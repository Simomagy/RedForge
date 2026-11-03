//! File drops on Wayland, which winit 0.30 reports only on X11 and Windows.
//!
//! A data device of our own, per seat, on the window's Wayland connection
//! (our own event queue on it, dispatched on a thread of its own) takes the
//! `text/uri-list` drags over the window, and hands the dropped files' paths
//! to the app.
//!
//! The thread never reads the socket: winit's event loop does, and libwayland
//! files our events into our queue, which the thread dispatches on a tick.
//! Reading as well (`blocking_dispatch`) would make it one of the display's
//! readers, waiting in `poll` with a read prepared, and eframe frees the
//! display at exit (winit's event loop lives in a thread local) while the
//! window may still get events: then reading would use a freed display, and
//! not reading would hang the other readers. The tick stops for good once the
//! window's surface is gone, which happens before the display is freed.

use std::ffi::{OsString, c_void};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use eframe::egui;
use percent_encoding::percent_decode;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use smithay_client_toolkit::data_device_manager::DataDeviceManagerState;
use smithay_client_toolkit::data_device_manager::data_device::{DataDevice, DataDeviceData, DataDeviceHandler};
use smithay_client_toolkit::data_device_manager::data_offer::{DataOfferData, DataOfferHandler, DragOffer};
use smithay_client_toolkit::globals::GlobalData;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::{delegate_registry, delegate_seat, registry_handlers};
use wayland_client::backend::{Backend, ObjectId, WaylandError};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_data_device::WlDataDevice;
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::WlDataOffer;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle, delegate_dispatch};

/// What a drag over the window does.
#[allow(dead_code)] // app.rs wires it in.
pub enum WaylandDrop {
    /// Files are being dragged over the window (true) or left it (false).
    Hover(bool),
    /// Files were dropped on it.
    Files(Vec<PathBuf>),
}

/// The mime type of file drags: a list of URIs.
const URI_LIST: &str = "text/uri-list";
/// How often the thread dispatches our events, and while a drag is over the
/// window or a drop is being read.
const IDLE_TICK: Duration = Duration::from_millis(50);
const BUSY_TICK: Duration = Duration::from_millis(10);

/// Starts taking drops on the window, when it's a Wayland one (else None).
/// Each event requests a repaint of `cc.egui_ctx`.
#[allow(dead_code)] // app.rs wires it in.
pub fn start(cc: &eframe::CreationContext) -> Option<Receiver<WaylandDrop>> {
    let display = cc.display_handle().map(|h| h.as_raw());
    let window = cc.window_handle().map(|h| h.as_raw());
    let (Ok(RawDisplayHandle::Wayland(display)), Ok(RawWindowHandle::Wayland(window))) = (display, window) else {
        log::debug!("Wayland file drops: not a Wayland window");
        return None;
    };
    match connect(display.display, window.surface, cc.egui_ctx.clone()) {
        Ok(events) => Some(events),
        Err(e) => {
            log::debug!("Wayland file drops unavailable: {e}");
            None
        }
    }
}

/// Our queue on winit's connection, a data device per seat, and the thread.
fn connect(
    display: NonNull<c_void>,
    surface: NonNull<c_void>,
    ctx: egui::Context,
) -> Result<Receiver<WaylandDrop>, String> {
    // SAFETY: the display is winit's, which outlives the window; the thread
    // stops using it once the window is gone (see the module's notes).
    let conn = Connection::from_backend(unsafe { Backend::from_foreign_display(display.as_ptr().cast()) });
    // SAFETY: the window's wl_surface, alive while it is (and the id only
    // ever compared, or checked for being alive, after).
    let surface = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }
        .map_err(|_| "the window handle isn't a wl_surface")?;
    // Watching the surface's life needs it managed by wayland-rs (winit's).
    conn.get_object_data(surface.clone()).map_err(|_| "the window's surface isn't winit's")?;

    let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("the registry: {e}"))?;
    let qh = queue.handle();
    let manager = DataDeviceManagerState::bind(&globals, &qh).map_err(|e| format!("no data device manager: {e}"))?;
    let (events, receiver) = mpsc::channel();
    let (read_done, reads) = mpsc::channel();
    let mut state = State {
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        manager,
        devices: Vec::new(),
        surface,
        events,
        ctx,
        hovering: false,
        reading: 0,
        read_done,
        closed: false,
    };
    for seat in state.seats.seats() {
        state.add_device(&qh, &seat);
    }
    conn.flush().map_err(|e| format!("flushing: {e}"))?;
    std::thread::Builder::new()
        .name("wayland-drop".into())
        .spawn(move || run(conn, queue, state, reads))
        .map_err(|e| format!("no thread: {e}"))?;
    Ok(receiver)
}

/// Dispatches our events on a tick until the window is gone, finishing the
/// drops whose data was read.
fn run(conn: Connection, mut queue: EventQueue<State>, mut state: State, reads: Receiver<(DragOffer, bool)>) {
    loop {
        // winit destroys the surface before freeing the display: from now on
        // nothing of it may be touched, so leave our objects to the exit.
        if state.surface.as_ptr().is_null() {
            std::mem::forget((state, queue, conn));
            return;
        }
        while let Ok((offer, read)) = reads.try_recv() {
            // A finish tells the source the drop succeeded; without, it's cancelled.
            if read {
                offer.finish();
            }
            offer.destroy();
            state.reading -= 1;
        }
        let dispatched =
            queue.dispatch_pending(&mut state).map_err(|e| e.to_string()).and_then(|_| match conn.flush() {
                // The socket is full: the next tick sends the rest.
                Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
                flushed => flushed.map_err(|e| e.to_string()),
            });
        // Stopping cleans up, the window (so the display) still being there.
        if let Err(e) = dispatched {
            log::debug!("Wayland file drops stopped: {e}");
            return;
        }
        // The app dropped its receiver.
        if state.closed {
            return;
        }
        std::thread::sleep(if state.hovering || state.reading > 0 { BUSY_TICK } else { IDLE_TICK });
    }
}

struct State {
    registry: RegistryState,
    seats: SeatState,
    manager: DataDeviceManagerState,
    /// A data device per seat.
    devices: Vec<DataDevice>,
    /// The window's wl_surface.
    surface: ObjectId,
    events: Sender<WaylandDrop>,
    ctx: egui::Context,
    /// Whether files are being dragged over the window.
    hovering: bool,
    /// The drops whose data is being read, then sent back on `read_done`
    /// with whether it was.
    reading: usize,
    read_done: Sender<(DragOffer, bool)>,
    /// The app dropped its receiver.
    closed: bool,
}

impl State {
    fn add_device(&mut self, qh: &QueueHandle<Self>, seat: &WlSeat) {
        if !self.devices.iter().any(|d| d.data().seat() == seat) {
            self.devices.push(self.manager.get_data_device(qh, seat));
        }
    }

    fn send(&mut self, event: WaylandDrop) {
        self.closed |= self.events.send(event).is_err();
        self.ctx.request_repaint();
    }

    fn hover(&mut self, hovering: bool) {
        if self.hovering != hovering {
            self.hovering = hovering;
            self.send(WaylandDrop::Hover(hovering));
        }
    }

    /// Whether a drag is files over the window.
    fn takes(&self, offer: &DragOffer) -> bool {
        offer.surface.id() == self.surface && offer.with_mime_types(|types| types.iter().any(|t| t == URI_LIST))
    }

    /// Reads a dropped list of files on a thread of its own (the source writes
    /// it once the compositor passed our request on).
    fn receive(&mut self, conn: &Connection, offer: DragOffer) {
        let pipe = match offer.receive(URI_LIST.into()) {
            Ok(pipe) => pipe,
            Err(e) => {
                log::debug!("Wayland file drop: no pipe for the data: {e}");
                offer.destroy();
                return;
            }
        };
        if let Err(e) = conn.flush() {
            log::debug!("Wayland file drop: flushing: {e}");
        }
        let mut pipe = File::from(OwnedFd::from(pipe));
        let (events, ctx, done) = (self.events.clone(), self.ctx.clone(), self.read_done.clone());
        let reading = offer.clone();
        let spawned = std::thread::Builder::new().name("wayland-drop-read".into()).spawn(move || {
            let mut data = Vec::new();
            let read = match pipe.read_to_end(&mut data) {
                Ok(_) => true,
                Err(e) => {
                    log::debug!("Wayland file drop: reading the data: {e}");
                    false
                }
            };
            let files = parse_uri_list(&data);
            if read && !files.is_empty() {
                let _ = events.send(WaylandDrop::Files(files));
                ctx.request_repaint();
            }
            let _ = done.send((reading, read));
        });
        match spawned {
            Ok(_) => self.reading += 1,
            Err(e) => {
                log::debug!("Wayland file drop: no thread to read it: {e}");
                offer.destroy();
            }
        }
    }
}

/// The drag on a data device, if any (none for one inside a client).
fn drag_offer(device: &WlDataDevice) -> Option<DragOffer> {
    device.data::<DataDeviceData>()?.drag_offer()
}

impl DataDeviceHandler for State {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, device: &WlDataDevice, _: f64, _: f64, _: &WlSurface) {
        let Some(offer) = drag_offer(device) else { return };
        if self.takes(&offer) {
            offer.accept_mime_type(offer.serial, Some(URI_LIST.into()));
            // Copy only: the app opens the files, it doesn't take them away.
            offer.set_actions(DndAction::Copy, DndAction::Copy);
            self.hover(true);
        }
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {
        self.hover(false);
    }

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}

    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn drop_performed(&mut self, conn: &Connection, _: &QueueHandle<Self>, device: &WlDataDevice) {
        let Some(offer) = drag_offer(device) else { return };
        if self.takes(&offer) {
            self.hover(false);
            self.receive(conn, offer);
        }
    }
}

impl DataOfferHandler for State {
    // The actions are set on enter, and the compositor settles on copy.
    fn source_actions(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &mut DragOffer, _: DndAction) {}

    fn selected_action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &mut DragOffer, _: DndAction) {}
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: WlSeat) {
        self.add_device(qh, &seat);
    }

    fn new_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, _: Capability) {}

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, _: Capability) {}

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: WlSeat) {
        self.devices.retain(|d| d.data().seat() != &seat);
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![SeatState];
}

delegate_registry!(State);
delegate_seat!(State);
// The data device's dispatching but for data sources (`delegate_data_device!`
// has them too): we only take drops.
delegate_dispatch!(State: [WlDataDeviceManager: GlobalData] => DataDeviceManagerState);
delegate_dispatch!(State: [WlDataDevice: DataDeviceData] => DataDeviceManagerState);
delegate_dispatch!(State: [WlDataOffer: DataOfferData] => DataDeviceManagerState);

/// The local files in a `text/uri-list` (RFC 2483): a URI a line, the lines
/// ended by CRLF (or just LF), `#` starting a comment line. URIs but local
/// `file:` ones are skipped.
fn parse_uri_list(data: &[u8]) -> Vec<PathBuf> {
    data.split(|&b| b == b'\n').map(trim).filter(|l| !l.is_empty() && l[0] != b'#').filter_map(file_path).collect()
}

/// A line without its surrounding whitespace (the CR of CRLF among it) and
/// NULs (which some sources end the list with).
fn trim(line: &[u8]) -> &[u8] {
    let junk = |b: &u8| b.is_ascii_whitespace() || *b == 0;
    let start = line.iter().position(|b| !junk(b)).unwrap_or(line.len());
    let end = line.iter().rposition(|b| !junk(b)).map_or(start, |i| i + 1);
    &line[start..end]
}

/// The path of a `file:` URI on this machine (no host, or localhost).
fn file_path(uri: &[u8]) -> Option<PathBuf> {
    const SCHEME: &[u8] = b"file://";
    if !uri.get(..SCHEME.len())?.eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let rest = &uri[SCHEME.len()..];
    let slash = rest.iter().position(|&b| b == b'/')?;
    let host = &rest[..slash];
    if !host.is_empty() && !host.eq_ignore_ascii_case(b"localhost") {
        return None;
    }
    // A query or fragment isn't the path's (a `?` or `#` in a name is escaped).
    let path = &rest[slash..];
    let path = &path[..path.iter().position(|&b| b == b'?' || b == b'#').unwrap_or(path.len())];
    Some(PathBuf::from(OsString::from_vec(percent_decode(path).collect())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists() {
        // Several files, CRLF ended, a comment, an escaped space.
        let list = b"# Dragged from Dolphin\r\nfile:///home/me/My%20Maps/valentine.rfproj\r\nfile:///tmp/a.rfproj\r\n";
        assert_eq!(
            parse_uri_list(list),
            [PathBuf::from("/home/me/My Maps/valentine.rfproj"), PathBuf::from("/tmp/a.rfproj")]
        );

        // LF only, no last newline, an empty line, localhost, the scheme's
        // case, escaped UTF-8, a fragment, a NUL at the end.
        let list = b"file://localhost/srv/x.ymap\n\nFILE:///caf%C3%A9/b.xml#frag\nfile:///c%23d.ytyp\0";
        assert_eq!(
            parse_uri_list(list),
            [PathBuf::from("/srv/x.ymap"), PathBuf::from("/café/b.xml"), PathBuf::from("/c#d.ytyp")]
        );

        // Others than local files are skipped.
        let list = b"https://example.com/a.rfproj\r\nfile://otherhost/b.rfproj\r\nfile:relative\r\n#file:///c\r\n";
        assert!(parse_uri_list(list).is_empty());

        // Names needn't be UTF-8.
        assert_eq!(parse_uri_list(b"file:///x%FF"), [PathBuf::from(OsString::from_vec(b"/x\xFF".to_vec()))]);

        assert!(parse_uri_list(b"").is_empty());
    }
}
