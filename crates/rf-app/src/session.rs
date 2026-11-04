//! What's kept between runs: the panes' layout (in eframe's storage, with
//! the window's size), and in the settings the world map's camera, hour,
//! weather, speed and statistics, and the tools' mode, snaps and axes.

use std::path::{Path, PathBuf};

use egui_dock::DockState;
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::app::Tab;
use crate::map::MapState;
use crate::map::helpers::Mode;
use crate::map::tools::Tools;
use crate::settings::Settings;

/// The layout's key in eframe's storage.
pub const DOCK: &str = "dock";

/// A tab as kept: the panes, the world map, and the welcome page in place
/// of the other documents.
#[derive(Serialize, Deserialize)]
pub enum SavedTab {
    Welcome,
    World,
    Properties,
    Project,
    Hierarchy,
    GameFiles,
    Objects,
}

pub fn saved_dock(dock: &DockState<Tab>) -> DockState<SavedTab> {
    dock.map_tabs(|t| match t {
        Tab::Map(v) if v.is_world() => SavedTab::World,
        Tab::Properties => SavedTab::Properties,
        Tab::Project => SavedTab::Project,
        Tab::Hierarchy => SavedTab::Hierarchy,
        Tab::GameFiles => SavedTab::GameFiles,
        Tab::Objects => SavedTab::Objects,
        _ => SavedTab::Welcome,
    })
}

/// The layout kept, with one welcome page where the documents were, and
/// whether the world map was open; `None` when there's no place for
/// documents in it.
pub fn restore_dock(saved: &DockState<SavedTab>) -> Option<(DockState<Tab>, bool)> {
    let world = saved.iter_all_tabs().any(|(_, t)| matches!(t, SavedTab::World));
    let mut dock = saved.map_tabs(|t| match t {
        SavedTab::Welcome | SavedTab::World => Tab::Welcome,
        SavedTab::Properties => Tab::Properties,
        SavedTab::Project => Tab::Project,
        SavedTab::Hierarchy => Tab::Hierarchy,
        SavedTab::GameFiles => Tab::GameFiles,
        SavedTab::Objects => Tab::Objects,
    });
    let mut first = true;
    dock.retain_tabs(|t| !matches!(t, Tab::Welcome) || std::mem::take(&mut first));
    (!first).then_some((dock, world))
}

const CAMERA: &str = "Map.Camera";
/// The project open when the camera was kept: another one gets looked at.
const CAMERA_PROJECT: &str = "Map.CameraProject";
const HOUR: &str = "Map.Hour";
const WEATHER: &str = "Map.Weather";
const SPEED: &str = "Map.Speed";
const STATS: &str = "Map.Statistics";
const SCRIPTED: &str = "Map.ScriptedMaps";

pub fn remember_map(settings: &mut Settings, s: &MapState, project: Option<&Path>) {
    let p = s.position;
    settings.set_all([
        (CAMERA, format!("{:.2},{:.2},{:.2},{:.4},{:.4}", p.x, p.y, p.z, s.yaw, s.pitch)),
        (CAMERA_PROJECT, project.map(|p| p.display().to_string()).unwrap_or_default()),
        (HOUR, format!("{:.2}", s.hour)),
        (WEATHER, s.weather.clone()),
        (SPEED, format!("{:.1}", s.speed)),
        (STATS, s.stats.to_string()),
        (SCRIPTED, s.scripted.to_string()),
    ]);
}

/// The world map as it was last time; `None` the first time.
pub fn kept_map(settings: &Settings) -> Option<MapState> {
    let n: Vec<f32> = settings.get(CAMERA)?.split(',').filter_map(|n| n.parse().ok()).collect();
    let &[x, y, z, yaw, pitch] = n.as_slice() else { return None };
    let number = |k: &str, default: f32| settings.get(k).and_then(|v| v.parse().ok()).unwrap_or(default);
    Some(MapState {
        position: Vec3::new(x, y, z),
        yaw,
        pitch,
        hour: number(HOUR, 12.0),
        weather: settings.get(WEATHER).unwrap_or("sunny").to_owned(),
        speed: number(SPEED, 30.0),
        stats: settings.get(STATS) == Some("true"),
        scripted: settings.get(SCRIPTED) != Some("false"),
    })
}

/// The project open when the camera was kept.
pub fn camera_project(settings: &Settings) -> Option<PathBuf> {
    settings.get(CAMERA_PROJECT).map(PathBuf::from)
}

const MODE: &str = "Tools.Mode";
const SNAP_MOVE: &str = "Tools.SnapMove";
const SNAP_ANGLE: &str = "Tools.SnapAngle";
const LOCAL: &str = "Tools.LocalAxes";

pub fn restore_tools(tools: &mut Tools, settings: &Settings) {
    let number = |k: &str| settings.get(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| *v >= 0.0);
    // REDFORGE_MODE (for testing) wins.
    if std::env::var("REDFORGE_MODE").is_err()
        && let Some(m) = settings.get(MODE).and_then(|m| Mode::ALL.into_iter().find(|x| x.name() == m))
    {
        tools.set_mode(m);
    }
    tools.snap_move = number(SNAP_MOVE).unwrap_or(tools.snap_move);
    tools.snap_angle = number(SNAP_ANGLE).unwrap_or(tools.snap_angle);
    tools.local = settings.get(LOCAL) == Some("true");
}

/// The tools' choices, saved when they change.
pub fn remember_tools(tools: &Tools, settings: &mut Settings) {
    settings.set_all([
        (MODE, tools.mode.name().to_owned()),
        (SNAP_MOVE, tools.snap_move.to_string()),
        (SNAP_ANGLE, tools.snap_angle.to_string()),
        (LOCAL, tools.local.to_string()),
    ]);
}

/// The town nearest a point, to say where the camera was.
pub fn nearest_place(places: &[(String, [f32; 3])], p: Vec3) -> Option<&str> {
    places
        .iter()
        .min_by(|a, b| Vec3::from(a.1).distance_squared(p).total_cmp(&Vec3::from(b.1).distance_squared(p)))
        .map(|(n, _)| n.as_str())
}
