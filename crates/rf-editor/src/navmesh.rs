//! Navmeshes being edited (ynv): their polygons' attributes, and areas
//! blocked off.
//!
//! A navmesh is opened when one of its polygons is selected, and becomes the
//! project's when it's changed, as ymaps do. Edits keep its geometry and its
//! lists as the game has them: only the polygons' attributes and the edges'
//! neighbours change, in place, so polygon indices stay valid across edits
//! and undo.
//!
//! **Blocking** an area makes its polygons an island: every edge between
//! them and the rest is cut, both ways, as at the navmesh's border, the
//! special links (climbs, drops, jumps) between them and the rest come out
//! of the polygons' lists, and they are switched off for peds. In game no path goes in or out (tested in RedM:
//! a path that went straight through went around). Switching polygons off for
//! peds alone doesn't stop the paths scripts ask for. Unblocking puts back the
//! game's edges.
//!
//! Each navmesh is a grid cell of its own, and no edge goes into another: the
//! cells join through their border polygons, so blocking works cell by cell.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rf_formats::formats::ynv::{self, Navmesh, NavmeshEdge, NavmeshPolygon, NavmeshSpecialLink, poly_flags};
use rf_formats::rpf8::FileRef;

use crate::{DocKey, Editor, Project, Root, autosave};

/// The polygon an edge has across it when it has none.
pub const NO_POLYGON: u32 = 0x7FFF;

/// The area ID of "no navmesh", which edges without a neighbour point at.
const NO_AREA: u32 = 0xFFFF;

/// A navmesh open in the editor.
pub struct NavmeshDoc {
    /// Its file name's hash, as the game streams it.
    pub name: u32,
    /// Lowercase, without the extension: `navmesh[312][333]`.
    pub file_name: String,
    /// Its data now.
    pub root: Arc<Navmesh>,
    /// The game's, for unblocking; `None` when the game has no navmesh of
    /// this name.
    original: Option<Arc<Navmesh>>,
    /// As opened: the game's, or the project's file.
    opened: Arc<Navmesh>,
    /// As last saved.
    saved: Option<Arc<Navmesh>>,
    /// Where it was last saved or loaded from.
    pub path: Option<PathBuf>,
    pub in_project: bool,
}

impl NavmeshDoc {
    /// Whether it differs from the game's.
    pub fn is_edited(&self) -> bool {
        self.original.as_ref().is_none_or(|o| !Arc::ptr_eq(o, &self.root))
    }

    pub fn is_dirty(&self) -> bool {
        !Arc::ptr_eq(self.saved.as_ref().unwrap_or(&self.opened), &self.root)
    }

    /// Whether the game has this navmesh, laid out as this one: its edges can
    /// be put back.
    pub fn can_unblock(&self) -> bool {
        self.original.as_ref().is_some_and(|o| same_layout(o, &self.root))
    }
}

/// A polygon of a navmesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PolyRef {
    /// The navmesh's name hash.
    pub navmesh: u32,
    pub index: u32,
}

/// The polygons by navmesh, each list sorted.
fn by_navmesh(polys: &[PolyRef]) -> Vec<(u32, Vec<u32>)> {
    let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in polys {
        map.entry(p.navmesh).or_default().push(p.index);
    }
    let mut out: Vec<(u32, Vec<u32>)> = map.into_iter().collect();
    for (_, list) in &mut out {
        list.sort_unstable();
        list.dedup();
    }
    out.sort_by_key(|(n, _)| *n);
    out
}

/// Whether two navmeshes have the same polygons with the same corners.
fn same_layout(a: &Navmesh, b: &Navmesh) -> bool {
    match (a.polygons(), b.polygons()) {
        (Some(pa), Some(pb)) => {
            pa.len() == pb.len()
                && pa.iter().zip(&pb).all(|(x, y)| x.index_id == y.index_id && x.index_count() == y.index_count())
                && a.indices() == b.indices()
        }
        _ => false,
    }
}

/// The index in `adj_area_ids` of the navmesh itself.
fn own_area(nm: &Navmesh) -> Option<u32> {
    let n = (nm.adj_area_ids_count as usize).min(32);
    nm.adj_area_ids[..n].iter().position(|&a| a == nm.area_id).map(|i| i as u32)
}

/// The polygon of the same navmesh across an edge, if there's one.
fn across(e: &NavmeshEdge, own: Option<u32>) -> Option<u32> {
    (Some(e.area_id1()) == own && e.poly_id1() != NO_POLYGON).then(|| e.poly_id1())
}

/// The polygons of the same navmesh next to `index`, by its edges.
pub fn neighbours(nm: &Navmesh, polygons: &[NavmeshPolygon], edges: &[NavmeshEdge], index: u32) -> Vec<u32> {
    let own = own_area(nm);
    let Some(p) = polygons.get(index as usize) else { return Vec::new() };
    let start = p.index_id as usize;
    let mut out: Vec<u32> = (start..start + p.index_count() as usize)
        .filter_map(|i| edges.get(i))
        .filter_map(|e| across(e, own))
        .filter(|&q| (q as usize) < polygons.len())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// An edge with nothing across it.
fn cut(mut e: NavmeshEdge, none: u32) -> NavmeshEdge {
    e.set_area_id1(none);
    e.set_area_id2(none);
    e.set_poly_id1(NO_POLYGON);
    e.set_poly_id2(NO_POLYGON);
    e.set_adjacency_type(0);
    e.set_space_beyond_edge(0);
    e
}

/// The polygon at each end of a special link, when that end is in this
/// navmesh.
fn link_ends(nm: &Navmesh, l: &NavmeshSpecialLink) -> (Option<u32>, Option<u32>) {
    let own = |a: u16| a as u32 == nm.area_id;
    (own(l.area_id1).then_some(l.poly_id1 as u32), own(l.area_id2).then_some(l.poly_id2 as u32))
}

/// The other end of a special link from polygon `p`: `Some(None)` when it's
/// in another navmesh, `None` when the link doesn't start or end at `p`.
fn other_end(nm: &Navmesh, l: &NavmeshSpecialLink, p: u32) -> Option<Option<u32>> {
    match link_ends(nm, l) {
        (Some(a), b) if a == p => Some(b),
        (a, Some(b)) if b == p => Some(a),
        _ => None,
    }
}

/// A polygon's range in the special link indices, if it has one there.
fn link_range(p: &NavmeshPolygon, len: usize) -> Option<std::ops::Range<usize>> {
    let (start, count) = (p.link_id() as usize, p.link_count() as usize);
    (count > 0 && start + count <= len).then_some(start..start + count)
}

/// Takes out of the polygons' lists the special links between `inside` and
/// the rest (another navmesh included): each list is reordered so that they
/// come last, and its count leaves them out, so nothing is renumbered.
/// Returns how many came out.
fn cut_links(nm: &mut Navmesh, inside: &HashSet<u32>) -> usize {
    let (Some(links), Some(mut inds), Some(polygons)) =
        (nm.special_links.clone(), nm.special_links_inds.clone(), nm.polygons())
    else {
        return 0;
    };
    let mut removed = 0;
    let mut counts = Vec::new();
    for (p, poly) in polygons.iter().enumerate() {
        let p = p as u32;
        let Some(range) = link_range(poly, inds.len()) else { continue };
        let crosses = |l: &u16| {
            let Some(other) = links.get(*l as usize).and_then(|link| other_end(nm, link, p)) else { return false };
            inside.contains(&p) != other.is_some_and(|o| inside.contains(&o))
        };
        let (cut, keep): (Vec<u16>, Vec<u16>) = inds[range.clone()].iter().partition(|l| crosses(l));
        if cut.is_empty() {
            continue;
        }
        removed += cut.len();
        for (slot, l) in inds[range].iter_mut().zip(keep.iter().chain(&cut)) {
            *slot = *l;
        }
        counts.push((p, keep.len()));
    }
    nm.special_links_inds = Some(inds);
    let polys = nm.polygons.as_mut().expect("polygons");
    for (p, n) in counts {
        if let Some(x) = polys.get_mut(p as usize) {
            x.set_link_count(n as u8);
        }
    }
    removed
}

/// Puts back in the polygons' lists the game's special links that `set`
/// starts or ends, unless their other end is still blocked off (switched off
/// for peds where the game has it on). Returns how many came back.
fn restore_links(nm: &mut Navmesh, original: &Navmesh, set: &HashSet<u32>) -> usize {
    let (Some(links), Some(orig_inds), Some(orig_polys), Some(mut inds), Some(polygons)) = (
        original.special_links.clone(),
        original.special_links_inds.clone(),
        original.polygons(),
        nm.special_links_inds.clone(),
        nm.polygons(),
    ) else {
        return 0;
    };
    if inds.len() != orig_inds.len() || polygons.len() != orig_polys.len() {
        return 0;
    }
    let off = |p: &NavmeshPolygon| p.flags() & poly_flags::SWITCHED_OFF_FOR_PEDS != 0;
    let blocked = |x: u32| off(&polygons[x as usize]) && !off(&orig_polys[x as usize]);
    let touches = |l: u16| {
        links.get(l as usize).is_some_and(|link| {
            let (a, b) = link_ends(original, link);
            a.is_some_and(|a| set.contains(&a)) || b.is_some_and(|b| set.contains(&b))
        })
    };
    let mut restored = 0;
    let mut counts = Vec::new();
    for (p, op) in orig_polys.iter().enumerate() {
        let p = p as u32;
        let Some(range) = link_range(op, orig_inds.len()) else { continue };
        let original_list = &orig_inds[range.clone()];
        if !set.contains(&p) && !original_list.iter().any(|&l| touches(l)) {
            continue;
        }
        let now = polygons[p as usize].link_count() as usize;
        let active: HashSet<u16> = inds[range.start..range.start + now.min(range.len())].iter().copied().collect();
        let back = |l: u16| {
            touches(l)
                && !blocked(p)
                && links
                    .get(l as usize)
                    .and_then(|link| other_end(original, link, p))
                    .is_some_and(|o| o.is_none_or(|o| !blocked(o)))
        };
        let (wanted, rest): (Vec<u16>, Vec<u16>) = original_list.iter().partition(|&&l| active.contains(&l) || back(l));
        if wanted.len() == now {
            continue;
        }
        restored += wanted.len().saturating_sub(now);
        for (slot, l) in inds[range].iter_mut().zip(wanted.iter().chain(&rest)) {
            *slot = *l;
        }
        counts.push((p, wanted.len()));
    }
    nm.special_links_inds = Some(inds);
    let polys = nm.polygons.as_mut().expect("polygons");
    for (p, n) in counts {
        if let Some(x) = polys.get_mut(p as usize) {
            x.set_link_count(n as u8);
        }
    }
    restored
}

/// What blocking or unblocking changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Blocked {
    /// Edges cut or put back (both ways count).
    pub edges: usize,
    /// Special links taken out of polygons' lists or put back (both ends
    /// count).
    pub links: usize,
}

/// Makes `set` an island (see the module's notes).
fn block(nm: &mut Navmesh, set: &[u32]) -> Blocked {
    let n = nm.polygons.as_ref().map_or(0, |p| p.item_count as usize);
    let inside: HashSet<u32> = set.iter().copied().filter(|&i| (i as usize) < n).collect();
    let edges = cut_edges(nm, &inside);
    let links = cut_links(nm, &inside);
    Blocked { edges, links }
}

/// Cuts the edges between `inside` and the rest, and switches `inside` off
/// for peds; returns how many edges were cut.
fn cut_edges(nm: &mut Navmesh, inside: &HashSet<u32>) -> usize {
    let (Some(polygons), Some(edges)) = (nm.polygons(), nm.edges()) else { return 0 };
    let own = own_area(nm);
    let none = nm.ensure_adj_area_id(NO_AREA);
    let mut changed: Vec<(usize, NavmeshEdge)> = Vec::new();
    let corners = |p: &NavmeshPolygon| p.index_id as usize..(p.index_id + p.index_count()) as usize;
    for &i in inside {
        let p = &polygons[i as usize];
        for k in corners(p) {
            let Some(e) = edges.get(k) else { continue };
            let Some(q) = across(e, own).filter(|q| !inside.contains(q)) else { continue };
            changed.push((k, cut(*e, none)));
            // And the neighbour's edges back.
            if let Some(qp) = polygons.get(q as usize) {
                for kq in corners(qp) {
                    if let Some(eq) = edges.get(kq)
                        && across(eq, own) == Some(i)
                    {
                        changed.push((kq, cut(*eq, none)));
                    }
                }
            }
        }
    }
    let list = nm.edges.as_mut().expect("edges");
    for (k, e) in &changed {
        if let Some(slot) = list.get_mut(*k) {
            *slot = *e;
        }
    }
    let polys = nm.polygons.as_mut().expect("polygons");
    for &i in inside {
        if let Some(p) = polys.get_mut(i as usize) {
            p.set_flags(p.flags() | poly_flags::SWITCHED_OFF_FOR_PEDS);
        }
    }
    changed.len()
}

/// Puts back the game's edges of `set` (and their neighbours' back to them),
/// their "switched off for peds" as the game has it, and their special links.
fn unblock(nm: &mut Navmesh, original: &Navmesh, set: &[u32]) -> Blocked {
    let edges = restore_edges(nm, original, set);
    let set: HashSet<u32> = set.iter().copied().collect();
    let links = restore_links(nm, original, &set);
    Blocked { edges, links }
}

/// The edges and flag part of [`unblock`]; returns how many edges changed.
fn restore_edges(nm: &mut Navmesh, original: &Navmesh, set: &[u32]) -> usize {
    let (Some(polygons), Some(edges), Some(orig_edges), Some(orig_polys)) =
        (nm.polygons(), nm.edges(), original.edges(), original.polygons())
    else {
        return 0;
    };
    let own = own_area(original);
    let mut changed: Vec<(usize, NavmeshEdge)> = Vec::new();
    let corners = |p: &NavmeshPolygon| p.index_id as usize..(p.index_id + p.index_count()) as usize;
    // The edges refer to the areas by index: the original's indices, in this
    // navmesh's list.
    let mut area_map = HashMap::new();
    let mut remap = |nm: &mut Navmesh, mut e: NavmeshEdge| {
        let area = original.adj_area_ids.get(e.area_id1() as usize).copied().unwrap_or(NO_AREA);
        let i = *area_map.entry(area).or_insert_with(|| nm.ensure_adj_area_id(area));
        e.set_area_id1(i);
        e.set_area_id2(i);
        e
    };
    for &i in set {
        let Some(p) = polygons.get(i as usize) else { continue };
        for k in corners(p) {
            let (Some(cur), Some(org)) = (edges.get(k), orig_edges.get(k)) else { continue };
            let org_fixed = remap(nm, *org);
            if *cur != org_fixed {
                changed.push((k, org_fixed));
            }
            // The neighbour's edges that the game has pointing here.
            let Some(q) = across(org, own) else { continue };
            let Some(qp) = orig_polys.get(q as usize) else { continue };
            for kq in corners(qp) {
                let (Some(cq), Some(oq)) = (edges.get(kq), orig_edges.get(kq)) else { continue };
                if across(oq, own) == Some(i) {
                    let oq_fixed = remap(nm, *oq);
                    if *cq != oq_fixed {
                        changed.push((kq, oq_fixed));
                    }
                }
            }
        }
    }
    let list = nm.edges.as_mut().expect("edges");
    for (k, e) in &changed {
        if let Some(slot) = list.get_mut(*k) {
            *slot = *e;
        }
    }
    let polys = nm.polygons.as_mut().expect("polygons");
    for &i in set {
        let off = orig_polys.get(i as usize).is_some_and(|p| p.flags() & poly_flags::SWITCHED_OFF_FOR_PEDS != 0);
        if let Some(p) = polys.get_mut(i as usize) {
            let f = p.flags() & !poly_flags::SWITCHED_OFF_FOR_PEDS;
            p.set_flags(if off { f | poly_flags::SWITCHED_OFF_FOR_PEDS } else { f });
        }
    }
    changed.len()
}

/// Reads a loose ynv.
fn read(bytes: &[u8], shown: &str) -> Result<Navmesh, String> {
    ynv::read(bytes).map_err(|e| format!("{shown}: {e}"))
}

impl Editor {
    /// A navmesh open in the editor.
    pub fn navmesh(&self, name: u32) -> Option<&NavmeshDoc> {
        self.navmeshes.get(&name)
    }

    /// The navmeshes open, edited or not.
    pub fn navmeshes(&self) -> impl Iterator<Item = &NavmeshDoc> {
        self.navmeshes.values()
    }

    /// The game's navmesh of a name, as it has it.
    fn game_navmesh(&self, name: u32) -> Option<(FileRef, Arc<Navmesh>)> {
        let file = self.base_data.stream_entry("ynv", name)?;
        let bytes = self.fs.extract_loose(file).ok()?;
        Some((file, Arc::new(ynv::read(&bytes).ok()?)))
    }

    /// Opens a navmesh by its name hash: the project's, or the game's.
    pub fn open_navmesh(&mut self, name: u32) -> Result<&NavmeshDoc, String> {
        if !self.navmeshes.contains_key(&name) {
            let file = self.data.stream_entry("ynv", name).ok_or("no navmesh of that name")?;
            let path = self.fs.name(file);
            let file_name = path.rsplit_once('.').map_or(path.as_str(), |(s, _)| s).to_lowercase();
            let original = self.game_navmesh(name);
            let opened = match &original {
                Some((f, nm)) if *f == file => nm.clone(),
                _ => {
                    let bytes = self.fs.extract_loose(file).map_err(|e| format!("{file_name}: {e}"))?;
                    Arc::new(read(&bytes, &file_name)?)
                }
            };
            let doc = NavmeshDoc {
                name,
                file_name,
                root: opened.clone(),
                original: original.map(|(_, nm)| nm),
                opened,
                saved: None,
                path: None,
                in_project: false,
            };
            self.navmeshes.insert(name, doc);
        }
        Ok(&self.navmeshes[&name])
    }

    /// A navmesh from disk, in place of the game's of the same name.
    pub(crate) fn load_navmesh_file(&mut self, path: &Path) -> Result<(), String> {
        let shown = path.display().to_string();
        let bytes = std::fs::read(path).map_err(|e| format!("{shown}: {e}"))?;
        let root = Arc::new(read(&bytes, &shown)?);
        let file_name = path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
        let name = self.hashes.ensure(&file_name);
        let doc = NavmeshDoc {
            name,
            file_name,
            root: root.clone(),
            original: self.game_navmesh(name).map(|(_, nm)| nm),
            opened: root.clone(),
            saved: Some(root),
            path: Some(path.to_owned()),
            in_project: true,
        };
        self.navmeshes.insert(name, doc);
        Ok(())
    }

    /// A navmesh as it's saved.
    pub(crate) fn navmesh_bytes(&self, name: u32) -> Result<Vec<u8>, String> {
        let doc = self.navmeshes.get(&name).ok_or("no such navmesh")?;
        Ok(ynv::write(&doc.root))
    }

    /// Writes an edited navmesh.
    pub fn save_navmesh(&mut self, name: u32, path: &Path) -> Result<(), String> {
        self.project.get_or_insert_with(Project::new).add_file(path);
        let bytes = self.navmesh_bytes(name)?;
        autosave::write_safely(path, &bytes)?;
        let doc = self.navmeshes.get_mut(&name).unwrap();
        doc.saved = Some(doc.root.clone());
        doc.path = Some(path.to_owned());
        Ok(())
    }

    pub(crate) fn apply_navmesh(&mut self, name: u32, root: Arc<Navmesh>) {
        if let Some(doc) = self.navmeshes.get_mut(&name) {
            doc.root = root;
        }
    }

    /// Changes the navmeshes of `polys`, each with its polygons' indices: one
    /// step (none when nothing changed).
    fn edit_navmeshes(
        &mut self,
        label: &str,
        polys: &[PolyRef],
        mut f: impl FnMut(&mut Navmesh, Option<&Navmesh>, &[u32]) -> bool,
    ) -> Result<(), String> {
        let mut roots = Vec::new();
        for (name, list) in by_navmesh(polys) {
            self.open_navmesh(name)?;
            let doc = &self.navmeshes[&name];
            let mut nm = (*doc.root).clone();
            if f(&mut nm, doc.original.as_deref(), &list) {
                roots.push((DocKey::Navmesh(name), Root::Navmesh(Arc::new(nm))));
            }
        }
        if !roots.is_empty() {
            self.commit_roots(label, roots);
        }
        Ok(())
    }

    /// A polygon of an open navmesh, as it is now.
    pub fn navmesh_polygon(&self, r: PolyRef) -> Option<NavmeshPolygon> {
        let polys = self.navmeshes.get(&r.navmesh)?.root.polygons.as_ref()?;
        polys.all_items()?.get(r.index as usize).copied()
    }

    /// A polygon's corners now, in the world.
    pub fn navmesh_polygon_corners(&self, r: PolyRef) -> Option<Vec<glam::Vec3>> {
        let nm = &self.navmeshes.get(&r.navmesh)?.root;
        let (v, i) = (nm.vertex_positions()?, nm.indices()?);
        let p = nm.polygons.as_ref()?.all_items()?.get(r.index as usize).copied()?;
        Some((0..p.index_count() as usize).map(|k| glam::Vec3::from(p.vertex(k, &v, &i))).collect())
    }

    /// How many polygons an open navmesh has.
    pub fn navmesh_polygon_count(&self, name: u32) -> usize {
        self.navmeshes.get(&name).and_then(|d| d.root.polygons.as_ref()).map_or(0, |p| p.item_count as usize)
    }

    /// The polygons of the same navmesh next to `polys` (not in them).
    pub fn navmesh_neighbours(&self, polys: &[PolyRef]) -> Vec<PolyRef> {
        let mut out = Vec::new();
        let set: HashSet<PolyRef> = polys.iter().copied().collect();
        for (name, list) in by_navmesh(polys) {
            let Some(doc) = self.navmeshes.get(&name) else { continue };
            let (Some(p), Some(e)) = (doc.root.polygons(), doc.root.edges()) else { continue };
            for i in list {
                for q in neighbours(&doc.root, &p, &e, i) {
                    let r = PolyRef { navmesh: name, index: q };
                    if !set.contains(&r) && !out.contains(&r) {
                        out.push(r);
                    }
                }
            }
        }
        out
    }

    /// The polygons of an open navmesh with every bit of `flags`.
    pub fn navmesh_polygons_with(&self, name: u32, flags: u64) -> Vec<PolyRef> {
        let Some(polys) = self.navmeshes.get(&name).and_then(|d| d.root.polygons()) else { return Vec::new() };
        let both = |p: &NavmeshPolygon| p.index_count() >= 3 && p.flags() & flags == flags;
        polys
            .iter()
            .enumerate()
            .filter(|(_, p)| both(p))
            .map(|(i, _)| PolyRef { navmesh: name, index: i as u32 })
            .collect()
    }

    /// Sets the bits of `set` and clears those of `clear` on the polygons'
    /// flags: one step.
    pub fn set_polygon_flags(&mut self, polys: &[PolyRef], set: u64, clear: u64) -> Result<(), String> {
        let label = if polys.len() == 1 { "Polygon flags" } else { "Polygons' flags" };
        self.edit_navmeshes(label, polys, |nm, _, list| {
            let Some(l) = nm.polygons.as_mut() else { return false };
            let mut changed = false;
            for &i in list {
                if let Some(p) = l.get_mut(i as usize) {
                    let f = (p.flags() | set) & !clear;
                    changed |= f != p.flags();
                    p.set_flags(f);
                }
            }
            changed
        })
    }

    /// Sets the polygons' audio properties (0-15) or water depth (0-7): one
    /// step.
    pub fn set_polygon_values(
        &mut self,
        polys: &[PolyRef],
        audio: Option<u8>,
        water_depth: Option<u8>,
    ) -> Result<(), String> {
        let label = if audio.is_some() { "Polygon audio" } else { "Polygon water depth" };
        self.edit_navmeshes(label, polys, |nm, _, list| {
            let Some(l) = nm.polygons.as_mut() else { return false };
            let before: Vec<NavmeshPolygon> = list.iter().filter_map(|&i| l.get_mut(i as usize).map(|p| *p)).collect();
            for &i in list {
                if let Some(p) = l.get_mut(i as usize) {
                    if let Some(a) = audio {
                        p.set_audio_properties(a.min(15));
                    }
                    if let Some(w) = water_depth {
                        p.set_water_depth(w.min(7));
                    }
                }
            }
            let after: Vec<NavmeshPolygon> = list.iter().filter_map(|&i| l.get_mut(i as usize).map(|p| *p)).collect();
            before != after
        })
    }

    /// Blocks the polygons off (see the module's notes): one step. Returns
    /// what was cut.
    pub fn block_polygons(&mut self, polys: &[PolyRef]) -> Result<Blocked, String> {
        let mut total = Blocked::default();
        self.edit_navmeshes("Block navmesh", polys, |nm, _, list| {
            let before = nm.polygons();
            let n = block(nm, list);
            total.edges += n.edges;
            total.links += n.links;
            n != Blocked::default() || nm.polygons() != before
        })?;
        Ok(total)
    }

    /// Puts back the game's edges and links around the polygons: one step.
    /// Returns what was put back.
    pub fn unblock_polygons(&mut self, polys: &[PolyRef]) -> Result<Blocked, String> {
        for (name, _) in by_navmesh(polys) {
            self.open_navmesh(name)?;
            let doc = &self.navmeshes[&name];
            if !doc.can_unblock() {
                return Err(format!("{}: the game has no navmesh laid out as this one", doc.file_name));
            }
        }
        let mut total = Blocked::default();
        self.edit_navmeshes("Unblock navmesh", polys, |nm, original, list| {
            let Some(original) = original else { return false };
            let before = nm.polygons();
            let n = unblock(nm, original, list);
            total.edges += n.edges;
            total.links += n.links;
            n != Blocked::default() || nm.polygons() != before
        })?;
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_formats::formats::ynv::NavmeshList;

    /// Three squares in a row, 0 | 1 | 2, each with four corners; edges
    /// between them, the rest without neighbours.
    fn strip() -> Navmesh {
        let mut nm = Navmesh { area_id: 7, ..Default::default() };
        nm.set_adj_area_ids_trimmed(Some(&[7, NO_AREA]));
        let mut polys = Vec::new();
        let mut edges = Vec::new();
        let edge = |area: u32, poly: u32| {
            let mut e = NavmeshEdge::default();
            e.set_area_id1(area);
            e.set_area_id2(area);
            e.set_poly_id1(poly);
            e.set_poly_id2(poly);
            e
        };
        for i in 0..3u32 {
            let mut p = NavmeshPolygon { index_id: i * 4, ..Default::default() };
            p.set_index_count(4);
            polys.push(p);
            // Corners: bottom, right, top, left.
            edges.push(edge(1, NO_POLYGON));
            edges.push(if i < 2 { edge(0, i + 1) } else { edge(1, NO_POLYGON) });
            edges.push(edge(1, NO_POLYGON));
            edges.push(if i > 0 { edge(0, i - 1) } else { edge(1, NO_POLYGON) });
        }
        nm.polygons = Some(NavmeshList::rebuild(&polys));
        nm.edges = Some(NavmeshList::rebuild(&edges));
        nm.indices = Some(NavmeshList::rebuild(&(0..12u16).collect::<Vec<_>>()));
        nm
    }

    #[test]
    fn blocking_makes_an_island_and_unblocking_puts_it_back() {
        let original = strip();
        let mut nm = original.clone();
        let (p, e) = (nm.polygons().unwrap(), nm.edges().unwrap());
        assert_eq!(neighbours(&nm, &p, &e, 1), [0, 2]);

        // The middle one: its two edges and the two back.
        assert_eq!(block(&mut nm, &[1]).edges, 4);
        let (p, e) = (nm.polygons().unwrap(), nm.edges().unwrap());
        assert!((0..3).all(|i| neighbours(&nm, &p, &e, i).is_empty()));
        assert_ne!(p[1].flags() & poly_flags::SWITCHED_OFF_FOR_PEDS, 0);
        assert_eq!(p[0].flags() & poly_flags::SWITCHED_OFF_FOR_PEDS, 0);
        // Nothing left to cut.
        assert_eq!(block(&mut nm, &[1]), Blocked::default());

        assert_eq!(unblock(&mut nm, &original, &[1]).edges, 4);
        assert_eq!(nm.edges(), original.edges());
        assert_eq!(nm.polygons(), original.polygons());
    }

    #[test]
    fn blocking_two_keeps_them_joined() {
        let mut nm = strip();
        // 0 and 1 together: only the edges to 2 go.
        assert_eq!(block(&mut nm, &[0, 1]).edges, 2);
        let (p, e) = (nm.polygons().unwrap(), nm.edges().unwrap());
        assert_eq!(neighbours(&nm, &p, &e, 0), [1]);
        assert_eq!(neighbours(&nm, &p, &e, 1), [0]);
        assert!(neighbours(&nm, &p, &e, 2).is_empty());
    }

    /// The strip with a climb from 0 to 2 and one from 2 to the next navmesh,
    /// each listed by the polygons at its ends, after a link of 0's own.
    fn strip_with_links() -> Navmesh {
        let mut nm = strip();
        let link = |a1: u16, p1: u16, a2: u16, p2: u16| NavmeshSpecialLink {
            area_id1: a1,
            poly_id1: p1,
            area_id2: a2,
            poly_id2: p2,
            ..Default::default()
        };
        // 0: a climb onto itself, then the one to 2; 2: the one from 0, then
        // the one to the next navmesh.
        nm.special_links = Some(vec![link(7, 0, 7, 0), link(7, 0, 7, 2), link(7, 2, 9, 4)]);
        nm.special_links_inds = Some(vec![0, 1, 1, 2]);
        let mut p = nm.polygons().unwrap();
        p[0].set_link_id(0);
        p[0].set_link_count(2);
        p[2].set_link_id(2);
        p[2].set_link_count(2);
        nm.polygons = Some(NavmeshList::rebuild(&p));
        nm
    }

    #[test]
    fn blocking_takes_the_links_out() {
        let original = strip_with_links();
        let mut nm = original.clone();
        // 2 blocked: the climb from 0 comes out of both lists, and 2's link to
        // the next navmesh out of its own.
        let b = block(&mut nm, &[2]);
        assert_eq!(b, Blocked { edges: 2, links: 3 });
        let p = nm.polygons().unwrap();
        assert_eq!((p[0].link_count(), p[2].link_count()), (1, 0));
        assert_eq!(&nm.special_links_inds.as_ref().unwrap()[..2], [0, 1]);
        assert_eq!(nm.special_links_inds.as_ref().unwrap().len(), 4);

        let back = unblock(&mut nm, &original, &[2]);
        assert_eq!(back, Blocked { edges: 2, links: 3 });
        assert_eq!(nm.polygons(), original.polygons());
        assert_eq!(nm.special_links_inds, original.special_links_inds);
    }

    #[test]
    fn unblocking_one_island_leaves_the_other() {
        let original = strip_with_links();
        let mut nm = original.clone();
        // 0 and 2 blocked apart: the climb between them stays out when only 0
        // is unblocked.
        block(&mut nm, &[0]);
        block(&mut nm, &[2]);
        unblock(&mut nm, &original, &[0]);
        let p = nm.polygons().unwrap();
        assert_eq!((p[0].link_count(), p[2].link_count()), (1, 0));
        unblock(&mut nm, &original, &[2]);
        assert_eq!(nm.polygons(), original.polygons());
    }

    #[test]
    fn layouts() {
        let a = strip();
        let mut b = strip();
        assert!(same_layout(&a, &b));
        let mut p = b.polygons().unwrap();
        p[2].set_index_count(3);
        b.polygons = Some(NavmeshList::rebuild(&p));
        assert!(!same_layout(&a, &b));
    }
}
