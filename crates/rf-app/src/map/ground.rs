//! Drop to ground (End), as the C#'s `DropSelectionToGround`: the selection
//! goes straight down (or up) until its lowest point rests on what's under
//! its middle. What's under it is the game's static collisions (the terrain
//! and the buildings) and the streamed interiors', read from their ybns (and
//! kept a while, so that End again nearby doesn't read them again).

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec2, Vec3};
use rf_editor::{Editor, EntityId, Placement};
use rf_formats::formats::ybn::{self, Bounds, BoundsShape, BoundsType, PolygonKind};
use rf_formats::rpf8::FileRef;

use super::helpers::physics::bounds::{child_transform, poly_box};
use super::tools::{self, count};
use crate::game::Game;

/// Ybns read at most at once; past it they're read again.
const KEPT: usize = 64;
/// Boxes this wide hold the whole map, not what's under a point.
const MAP_WIDE: f32 = 3000.0;

#[derive(Default)]
pub struct Ground {
    /// The ybns' triangles, in their own space (the world's, or their
    /// interior's), by name hash.
    read: HashMap<u32, Arc<Vec<[Vec3; 3]>>>,
}

impl Ground {
    /// End: the selection straight down (or up) onto what's under its
    /// middle; what happened, to show.
    pub fn drop_selection(&mut self, game: &Game, editor: &mut Editor) -> Result<String, String> {
        let items: Vec<(EntityId, Placement)> = editor
            .selection()
            .iter()
            .filter(|id| editor.is_editable(**id))
            .filter_map(|id| Some((*id, editor.placement(*id)?)))
            .collect();
        if items.is_empty() {
            return Err("Select what to drop to the ground first".into());
        }
        // The selection's lowest and highest points, and its middle.
        let (mut low, mut high, mut middle) = (f32::MAX, f32::MIN, Vec3::ZERO);
        for (id, p) in &items {
            middle += p.position;
            let Some((m, min, max)) = tools::entity_box(editor, *id) else { continue };
            for i in 0..8 {
                let corner = Vec3::new(
                    if i & 1 != 0 { max.x } else { min.x },
                    if i & 2 != 0 { max.y } else { min.y },
                    if i & 4 != 0 { max.z } else { min.z },
                );
                let z = m.transform_point3(corner).z;
                (low, high) = (low.min(z), high.max(z));
            }
        }
        middle /= items.len() as f32;
        let from = Vec3::new(middle.x, middle.y, high + 1.0);
        let ground = self.below(game, &editor.world, from).ok_or("There's no ground or building under it")?;
        let rise = ground - low;
        if rise.abs() < 1e-4 {
            return Ok("It's on the ground already".into());
        }
        let moved: Vec<(EntityId, Placement)> =
            items.into_iter().map(|(id, p)| (id, Placement { position: p.position + Vec3::Z * rise, ..p })).collect();
        editor.set_placements("Drop to ground", &moved);
        let way = if rise < 0.0 { "down" } else { "up" };
        Ok(format!("Dropped {} {:.2} m {way}", count(moved.len(), "object"), rise.abs()))
    }

    /// The highest surface straight below `from` (within 2 km).
    fn below(&mut self, game: &Game, world: &rf_world::World, from: Vec3) -> Option<f32> {
        let data = game.data();
        let at = Vec2::new(from.x, from.y);
        let under = |min: Vec3, max: Vec3| {
            (min.x..=max.x).contains(&at.x)
                && (min.y..=max.y).contains(&at.y)
                && min.z <= from.z
                && max.z >= from.z - 2000.0
                && (max.x - min.x).min(max.y - min.y) < MAP_WIDE
        };
        // The ybns whose box is under the point, and where they're placed.
        let mut candidates: Vec<(u32, FileRef, Option<Mat4>)> = data
            .stream_entries("ybn")
            .filter(|(hash, _)| !data.interior_bounds_lookup.contains_key(hash))
            .filter(|(hash, _)| data.bounds_cache.get(hash).is_some_and(|b| under(b.min.into(), b.max.into())))
            .map(|(hash, file)| (hash, file, None))
            .collect();
        for (_, helpers) in world.map_helpers() {
            for interior in &helpers.interiors {
                let Some(files) = data.interiors.get(&interior.archetype).and_then(|i| i.bounds.as_ref()) else {
                    continue;
                };
                let place = Mat4::from_cols_array(&interior.transform);
                for &hash in files {
                    let near = data.bounds_cache.get(&hash).is_some_and(|b| {
                        let (min, max) = world_box(&place, b.min.into(), b.max.into());
                        under(min, max)
                    });
                    if near && let Some(file) = data.stream_entry("ybn", hash) {
                        candidates.push((hash, file, Some(place)));
                    }
                }
            }
        }
        let mut best: Option<f32> = None;
        for (hash, file, place) in candidates {
            let triangles = match self.read.get(&hash) {
                Some(t) => t.clone(),
                None => {
                    let t = Arc::new(triangles(game, file));
                    self.read.insert(hash, t.clone());
                    t
                }
            };
            for t in triangles.iter() {
                let t = match place {
                    Some(m) => t.map(|p| m.transform_point3(p)),
                    None => *t,
                };
                if let Some(z) = height_at(&t, at)
                    && z <= from.z
                    && best.is_none_or(|b| z > b)
                {
                    best = Some(z);
                }
            }
        }
        if self.read.len() > KEPT {
            self.read.clear();
        }
        best
    }
}

/// A ybn's triangles in the world's space: its geometries' (with their box
/// polygons) and its boxes.
fn triangles(game: &Game, file: FileRef) -> Vec<[Vec3; 3]> {
    let mut out = Vec::new();
    let read = game.fs.extract_loose(file).map_err(rf_formats::Error::from).and_then(|d| ybn::read(&d));
    match read {
        Ok(root) => add(&root, Mat4::IDENTITY, &mut out),
        Err(e) => log::warn!("{}: {e}", game.path(file)),
    }
    out
}

fn add(b: &Bounds, parent: Mat4, out: &mut Vec<[Vec3; 3]>) {
    let place = parent * child_transform(&b.transform);
    match &b.shape {
        BoundsShape::Composite(c) => {
            for child in c.children.iter().flatten().flatten() {
                add(child, place, out);
            }
        }
        BoundsShape::Geometry(g) | BoundsShape::GeometryBvh(g, _) => {
            let vertices = g.vertices.as_deref().unwrap_or_default();
            let offset = Vec3::from(g.center_geom);
            let v = |i: i64| {
                let p = usize::try_from(i).ok().and_then(|i| vertices.get(i))?;
                Some(place.transform_point3(Vec3::from(*p) + offset))
            };
            for p in &g.polygons {
                match p.kind {
                    PolygonKind::Triangle(t) => {
                        if let [Some(a), Some(b), Some(c)] =
                            [t.vert_index1, t.vert_index2, t.vert_index3].map(|i| v(i as i64))
                        {
                            out.push([a, b, c]);
                        }
                    }
                    PolygonKind::Box(bx) => {
                        let corners = [bx.vert_index1, bx.vert_index2, bx.vert_index3, bx.vert_index4];
                        // The box's corners are already placed: poly_box in the world.
                        if let [Some(a), Some(b), Some(c), Some(d)] = corners.map(|i| v(i as i64)) {
                            box_triangles(poly_box(a, b, c, d), out);
                        }
                    }
                    _ => {}
                }
            }
        }
        BoundsShape::Basic if matches!(b.bounds_type, BoundsType::Box) => {
            let (min, max) = (Vec3::from(b.box_min), Vec3::from(b.box_max));
            box_triangles(place * Mat4::from_translation((min + max) * 0.5) * Mat4::from_scale(max - min), out);
        }
        _ => {}
    }
}

/// The world box around a box through a transform.
fn world_box(m: &Mat4, min: Vec3, max: Vec3) -> (Vec3, Vec3) {
    (0..8).fold((Vec3::MAX, Vec3::MIN), |(lo, hi), i| {
        let corner = Vec3::new(
            if i & 1 != 0 { max.x } else { min.x },
            if i & 2 != 0 { max.y } else { min.y },
            if i & 4 != 0 { max.z } else { min.z },
        );
        let p = m.transform_point3(corner);
        (lo.min(p), hi.max(p))
    })
}

/// The 12 triangles of the unit box (-0.5 to 0.5) through a transform.
fn box_triangles(m: Mat4, out: &mut Vec<[Vec3; 3]>) {
    let c = |i: usize| {
        m.transform_point3(Vec3::new((i & 1) as f32 - 0.5, ((i >> 1) & 1) as f32 - 0.5, ((i >> 2) & 1) as f32 - 0.5))
    };
    for [a, b, cc, d] in [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]] {
        out.push([c(a), c(b), c(cc)]);
        out.push([c(a), c(cc), c(d)]);
    }
}

/// Where a vertical line through `at` meets a triangle, if it does.
fn height_at(t: &[Vec3; 3], at: Vec2) -> Option<f32> {
    let [a, b, c] = *t;
    let (e1, e2, p) = (b.truncate() - a.truncate(), c.truncate() - a.truncate(), at - a.truncate());
    let d = e1.perp_dot(e2);
    if d.abs() < 1e-9 {
        return None;
    }
    let u = p.perp_dot(e2) / d;
    let v = e1.perp_dot(p) / d;
    const EDGE: f32 = -1e-5;
    (u >= EDGE && v >= EDGE && u + v <= 1.0 - EDGE).then_some(a.z + u * (b.z - a.z) + v * (c.z - a.z))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights() {
        let t = [Vec3::new(0.0, 0.0, 1.0), Vec3::new(10.0, 0.0, 1.0), Vec3::new(0.0, 10.0, 11.0)];
        assert_eq!(height_at(&t, Vec2::new(0.0, 5.0)), Some(6.0));
        assert!((height_at(&t, Vec2::new(2.0, 2.0)).unwrap() - 3.0).abs() < 1e-5);
        assert_eq!(height_at(&t, Vec2::new(9.0, 9.0)), None);
        let mut boxes = Vec::new();
        box_triangles(Mat4::from_scale(Vec3::splat(2.0)), &mut boxes);
        let top = boxes.iter().filter_map(|t| height_at(t, Vec2::ZERO)).fold(f32::MIN, f32::max);
        assert!((top - 1.0).abs() < 1e-5);
    }
}
