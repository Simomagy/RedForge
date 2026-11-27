//! The editing commands of the keyboard, the menus, the layer list and the
//! inspector, as functions from the objects (and selection) to the new
//! objects: each call is one undo step for the caller.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};

use super::doc::{ancestors, index_of, is_locked, with_descendants};
use super::geometry::{selection_bounds, translate};
use super::marks::Assets;
use super::path::{PathNode, Point};
use super::project::{GroupObject, MapObject, new_id};
use super::project_file::{from_data_url, to_data_url};

/// What a command leaves selected.
pub type Selected = Vec<String>;

/// The selection's top-level ids: members of a selected group are left to it.
pub fn roots(ids: &[String], objects: &[MapObject]) -> Vec<String> {
    let index = index_of(objects);
    let set: HashSet<&str> = ids.iter().map(String::as_str).collect();
    ids.iter()
        .filter(|id| index.get(id.as_str()).is_some_and(|o| !ancestors(o, &index).iter().any(|g| set.contains(g.id()))))
        .cloned()
        .collect()
}

pub fn delete(objects: &[MapObject], selection: &[String]) -> Vec<MapObject> {
    let doomed = with_descendants(selection.iter().map(String::as_str), objects);
    objects.iter().filter(|o| !doomed.contains(o.id())).cloned().collect()
}

/// Copies with new ids, moved by `(dx, dy)`; group links inside the set
/// follow the new ids, the others stay when `keep_group` says.
fn clone_all(items: &[MapObject], dx: f64, dy: f64, keep_group: impl Fn(&str) -> bool) -> Vec<MapObject> {
    let ids: HashMap<&str, String> = items.iter().map(|o| (o.id(), new_id())).collect();
    items
        .iter()
        .map(|o| {
            let mut c = o.clone();
            let common = c.common_mut();
            common.id = ids[o.id()].clone();
            common.group =
                common.group.take().and_then(|g| ids.get(g.as_str()).cloned().or_else(|| keep_group(&g).then_some(g)));
            translate(&mut c, dx, dy);
            c
        })
        .collect()
}

/// The selection copied, nudged; the copies (their top) are selected.
pub fn duplicate(objects: &[MapObject], selection: &[String], dx: f64, dy: f64) -> (Vec<MapObject>, Selected) {
    let all = with_descendants(selection.iter().map(String::as_str), objects);
    let items: Vec<MapObject> = objects.iter().filter(|o| all.contains(o.id())).cloned().collect();
    let copies = clone_all(&items, dx, dy, |_| true);
    let tops: HashSet<String> = roots(selection, objects).into_iter().collect();
    let selected =
        copies.iter().zip(&items).filter(|(_, o)| tops.contains(o.id())).map(|(c, _)| c.id().to_owned()).collect();
    let mut out = objects.to_vec();
    out.extend(copies);
    (out, selected)
}

/// The text the clipboard gets: the layers (groups' members too) and the
/// pictures their stamps use.
pub const CLIPBOARD_APP: &str = "rdr-map-editor/layers";

pub fn clipboard_text(
    objects: &[MapObject],
    selection: &[String],
    pictures: &BTreeMap<String, Arc<Vec<u8>>>,
) -> Option<String> {
    let all = with_descendants(selection.iter().map(String::as_str), objects);
    let items: Vec<&MapObject> = objects.iter().filter(|o| all.contains(o.id())).collect();
    if items.is_empty() {
        return None;
    }
    let mut images = serde_json::Map::new();
    for o in &items {
        if let MapObject::Stamp(s) = o
            && let Some(b) = pictures.get(&s.asset)
        {
            images.insert(s.asset.clone(), Value::String(to_data_url(b)));
        }
    }
    Some(json!({ "app": CLIPBOARD_APP, "objects": items, "images": images }).to_string())
}

/// Pasted layers, centred on `at` when given, otherwise nudged from where
/// they were; and the pictures they bring that aren't there yet.
pub struct Pasted {
    pub objects: Vec<MapObject>,
    pub selected: Selected,
    pub pictures: Vec<(String, Vec<u8>)>,
}

pub fn paste(objects: &[MapObject], text: &str, at: Option<Point>, assets: &Assets) -> Option<Pasted> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("app")?.as_str()? != CLIPBOARD_APP {
        return None;
    }
    let items: Vec<MapObject> =
        v.get("objects")?.as_array()?.iter().filter_map(|o| serde_json::from_value(o.clone()).ok()).collect();
    if items.is_empty() {
        return None;
    }
    let pictures: Vec<(String, Vec<u8>)> = v
        .get("images")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(id, url)| Some((id.clone(), from_data_url(url.as_str()?)?.1))).collect())
        .unwrap_or_default();
    let ids: Vec<String> = items.iter().map(|o| o.id().to_owned()).collect();
    let (dx, dy) = match (at, selection_bounds(&ids, &items, assets)) {
        (Some(at), Some(b)) => (at[0] - (b.x0 + b.x1) / 2.0, at[1] - (b.y0 + b.y1) / 2.0),
        _ => (12.0, 12.0),
    };
    let inside: HashSet<&str> = items.iter().map(MapObject::id).collect();
    let copies = clone_all(&items, dx, dy, |g| inside.contains(g));
    let copy_ids: HashSet<&str> = copies.iter().map(MapObject::id).collect();
    let selected = copies
        .iter()
        .filter(|c| c.common().group.as_deref().is_none_or(|g| !copy_ids.contains(g)))
        .map(|c| c.id().to_owned())
        .collect();
    let mut out = objects.to_vec();
    out.extend(copies);
    Some(Pasted { objects: out, selected, pictures })
}

/// The selection in a new group, where its top member was.
pub fn group(objects: &[MapObject], selection: &[String]) -> Option<(Vec<MapObject>, Selected)> {
    let ids = roots(selection, objects);
    let index = index_of(objects);
    let parent = index.get(ids.last()?.as_str())?.common().group.clone();
    let id = new_id();
    let mut out = objects.to_vec();
    let top = out.iter().rposition(|o| ids.iter().any(|i| i == o.id()))?;
    let g = MapObject::Group(GroupObject {
        common: super::project::Common { id: id.clone(), group: parent, ..Default::default() },
        collapsed: false,
    });
    out.insert(top + 1, g);
    for o in &mut out {
        if ids.iter().any(|i| i == o.id()) {
            o.common_mut().group = Some(id.clone());
        }
    }
    Some((out, vec![id]))
}

/// The selected groups taken apart: their members go to the group above.
pub fn ungroup(objects: &[MapObject], selection: &[String]) -> Option<(Vec<MapObject>, Selected)> {
    let groups: Vec<&MapObject> =
        objects.iter().filter(|o| o.is_group() && selection.iter().any(|s| s == o.id())).collect();
    if groups.is_empty() {
        return None;
    }
    let parents: HashMap<String, Option<String>> =
        groups.iter().map(|g| (g.id().to_owned(), g.common().group.clone())).collect();
    let mut freed = Vec::new();
    let mut out: Vec<MapObject> = objects.iter().filter(|o| !parents.contains_key(o.id())).cloned().collect();
    for o in &mut out {
        let Some(g) = o.common().group.clone() else { continue };
        if let Some(parent) = parents.get(&g) {
            o.common_mut().group = parent.clone();
            freed.push(o.id().to_owned());
        }
    }
    Some((out, freed))
}

/// Locks the layers, or unlocks them when they're all locked.
pub fn toggle_lock(objects: &[MapObject], ids: &[String]) -> Vec<MapObject> {
    let lock = !ids.iter().all(|id| objects.iter().any(|o| o.id() == id && o.common().locked));
    objects
        .iter()
        .map(|o| {
            let mut o = o.clone();
            if ids.iter().any(|i| i == o.id()) {
                o.common_mut().locked = lock;
            }
            o
        })
        .collect()
}

pub fn toggle_hidden(objects: &[MapObject], ids: &[String]) -> Vec<MapObject> {
    let hide = !ids.iter().all(|id| objects.iter().any(|o| o.id() == id && o.common().hidden));
    objects
        .iter()
        .map(|o| {
            let mut o = o.clone();
            if ids.iter().any(|i| i == o.id()) {
                o.common_mut().hidden = hide;
            }
            o
        })
        .collect()
}

pub fn unlock_all(objects: &[MapObject]) -> Vec<MapObject> {
    objects
        .iter()
        .map(|o| {
            let mut o = o.clone();
            o.common_mut().locked = false;
            o
        })
        .collect()
}

pub fn show_all(objects: &[MapObject]) -> Vec<MapObject> {
    objects
        .iter()
        .map(|o| {
            let mut o = o.clone();
            o.common_mut().hidden = false;
            o
        })
        .collect()
}

/// Every top-level layer that isn't locked or hidden.
pub fn select_all(objects: &[MapObject]) -> Selected {
    let index = index_of(objects);
    objects
        .iter()
        .filter(|o| o.common().group.is_none() && !is_locked(o, &index) && !o.common().hidden)
        .map(|o| o.id().to_owned())
        .collect()
}

/// Moves the selection (or, editing one line's points, its chosen anchors).
pub fn nudge(objects: &[MapObject], selection: &[String], nodes: &[usize], dx: f64, dy: f64) -> Vec<MapObject> {
    let mut out = objects.to_vec();
    if let ([id], false) = (selection, nodes.is_empty()) {
        if let Some(MapObject::Line(l)) = out.iter_mut().find(|o| o.id() == id) {
            for &i in nodes {
                if let Some(n) = l.nodes.get_mut(i) {
                    n.p = [n.p[0] + dx, n.p[1] + dy];
                }
            }
        }
        return out;
    }
    let all = with_descendants(selection.iter().map(String::as_str), objects);
    for o in &mut out {
        if all.contains(o.id()) {
            translate(o, dx, dy);
        }
    }
    out
}

/// Where layers go in the list, as it shows them (top = last).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    Above,
    Below,
    /// In a group, as its top member.
    Into,
}

/// Moves layers above or below another, or into a group.
pub fn move_layers(objects: &[MapObject], ids: &[String], target: &str, at: Where) -> Option<Vec<MapObject>> {
    let moving = with_descendants(ids.iter().map(String::as_str), objects);
    if moving.contains(target) {
        return None;
    }
    let target_obj = objects.iter().find(|o| o.id() == target)?;
    let parent = if at == Where::Into { Some(target.to_owned()) } else { target_obj.common().group.clone() };
    let tops = roots(ids, objects);
    let taken: Vec<MapObject> = objects.iter().filter(|o| moving.contains(o.id())).cloned().collect();
    let mut out: Vec<MapObject> = objects.iter().filter(|o| !moving.contains(o.id())).cloned().collect();
    let ti = out.iter().position(|o| o.id() == target)?;
    let i = match at {
        Where::Below => ti,
        Where::Above => ti + 1,
        Where::Into => out.len(),
    };
    out.splice(i..i, taken);
    for o in &mut out {
        if tops.iter().any(|t| t == o.id()) {
            o.common_mut().group = parent.clone();
        }
    }
    Some(out)
}

fn edit_line(objects: &[MapObject], id: &str, f: impl FnOnce(&mut super::project::LineObject)) -> Vec<MapObject> {
    let mut out = objects.to_vec();
    if let Some(MapObject::Line(l)) = out.iter_mut().find(|o| o.id() == id) {
        f(l);
    }
    out
}

/// Takes anchors out of a line; a line left with fewer than two goes.
pub fn delete_nodes(objects: &[MapObject], id: &str, nodes: &[usize]) -> Vec<MapObject> {
    let Some(MapObject::Line(l)) = objects.iter().find(|o| o.id() == id) else { return objects.to_vec() };
    if l.nodes.len().saturating_sub(nodes.len()) < 2 {
        return objects.iter().filter(|o| o.id() != id).cloned().collect();
    }
    edit_line(objects, id, |l| {
        let mut i = 0;
        l.nodes.retain(|_| {
            i += 1;
            !nodes.contains(&(i - 1))
        });
        if l.nodes.len() < 3 {
            l.closed = false;
        }
    })
}

/// Corner: no handles. Smooth: handles along the neighbours' direction, a
/// third of the way to each (as Illustrator's Convert Anchor draws them).
pub fn convert_nodes(objects: &[MapObject], id: &str, nodes: &[usize], smooth: bool) -> Vec<MapObject> {
    edit_line(objects, id, |l| {
        let n = l.nodes.len();
        for &i in nodes {
            if i >= n {
                continue;
            }
            if !smooth {
                (l.nodes[i].inn, l.nodes[i].out) = (None, None);
                continue;
            }
            let prev = l.nodes[if i > 0 {
                i - 1
            } else if l.closed {
                n - 1
            } else {
                i
            }]
            .p;
            let next = l.nodes[if i < n - 1 {
                i + 1
            } else if l.closed {
                0
            } else {
                i
            }]
            .p;
            let p = l.nodes[i].p;
            let (mut tx, mut ty) = (next[0] - prev[0], next[1] - prev[1]);
            let len = tx.hypot(ty);
            let len = if len > 0.0 { len } else { 1.0 };
            (tx, ty) = (tx / len, ty / len);
            let d_prev = (p[0] - prev[0]).hypot(p[1] - prev[1]) / 3.0;
            let d_next = (next[0] - p[0]).hypot(next[1] - p[1]) / 3.0;
            l.nodes[i].inn = (d_prev > 0.0).then_some([-tx * d_prev, -ty * d_prev]);
            l.nodes[i].out = (d_next > 0.0).then_some([tx * d_next, ty * d_next]);
        }
    })
}

pub fn reverse_line(objects: &[MapObject], id: &str) -> Vec<MapObject> {
    edit_line(objects, id, |l| {
        l.nodes = l.nodes.iter().rev().map(|n| PathNode { p: n.p, inn: n.out, out: n.inn }).collect();
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pausemap::path::from_points;
    use crate::pausemap::project::{Common, LineObject, LineStyle};

    fn line(id: &str) -> MapObject {
        MapObject::Line(LineObject {
            common: Common { id: id.into(), ..Default::default() },
            style: LineStyle::Road,
            width: 1.0,
            spacing: 0.0,
            closed: false,
            nodes: from_points(&[[0.0, 0.0], [10.0, 0.0], [20.0, 5.0]], false),
        })
    }

    // The web app's commands tests.
    #[test]
    fn groups_come_and_go() {
        let objects = vec![line("a"), line("b"), line("c")];
        let (g, sel) = group(&objects, &["a".into(), "b".into()]).unwrap();
        assert_eq!(g.len(), 4);
        // The group takes the top member's place: after b.
        assert!(g[2].is_group() && g[2].id() == sel[0]);
        assert_eq!(g[0].common().group.as_deref(), Some(sel[0].as_str()));
        let (u, freed) = ungroup(&g, &sel).unwrap();
        assert_eq!(u.len(), 3);
        assert_eq!(freed.len(), 2);
        assert!(u.iter().all(|o| o.common().group.is_none()));
    }

    #[test]
    fn duplicates_and_paste_get_new_ids() {
        let objects = vec![line("a")];
        let (d, sel) = duplicate(&objects, &["a".into()], 12.0, 12.0);
        assert_eq!(d.len(), 2);
        assert_ne!(sel[0], "a");
        let text = clipboard_text(&objects, &["a".into()], &BTreeMap::new()).unwrap();
        let p = paste(&objects, &text, Some([100.0, 100.0]), &Assets::default()).unwrap();
        assert_eq!(p.objects.len(), 2);
        let MapObject::Line(l) = &p.objects[1] else { panic!() };
        // Centred on the point: the line's box was 0..20 x 0..5 (and half
        // its width).
        assert!((l.nodes[0].p[0] - 90.0).abs() < 1e-9);
        assert!(paste(&objects, "{}", None, &Assets::default()).is_none());
    }

    #[test]
    fn layers_move_above_below_and_into() {
        let objects = vec![line("a"), line("b"), line("c")];
        let ids = |v: &[MapObject]| v.iter().map(|o| o.id().to_owned()).collect::<Vec<_>>();
        let m = move_layers(&objects, &["a".into()], "c", Where::Above).unwrap();
        assert_eq!(ids(&m), vec!["b", "c", "a"]);
        let m = move_layers(&objects, &["c".into()], "a", Where::Below).unwrap();
        assert_eq!(ids(&m), vec!["c", "a", "b"]);
        let (g, sel) = group(&objects, &["b".into()]).unwrap();
        let m = move_layers(&g, &["a".into()], &sel[0], Where::Into).unwrap();
        assert_eq!(m.iter().find(|o| o.id() == "a").unwrap().common().group.as_deref(), Some(sel[0].as_str()));
    }

    #[test]
    fn anchors() {
        let objects = vec![line("a")];
        let d = delete_nodes(&objects, "a", &[1]);
        let MapObject::Line(l) = &d[0] else { panic!() };
        assert_eq!(l.nodes.len(), 2);
        assert!(delete_nodes(&objects, "a", &[0, 1]).is_empty());
        let s = convert_nodes(&objects, "a", &[1], true);
        let MapObject::Line(l) = &s[0] else { panic!() };
        assert!(l.nodes[1].inn.is_some() && l.nodes[1].out.is_some());
        let r = reverse_line(&objects, "a");
        let MapObject::Line(l) = &r[0] else { panic!() };
        assert_eq!(l.nodes[0].p, [20.0, 5.0]);
        let n = nudge(&objects, &["a".into()], &[0], 1.0, 1.0);
        let MapObject::Line(l) = &n[0] else { panic!() };
        assert_eq!((l.nodes[0].p, l.nodes[1].p), ([1.0, 1.0], [10.0, 0.0]));
    }
}
