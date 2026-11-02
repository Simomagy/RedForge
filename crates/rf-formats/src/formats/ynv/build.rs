//! Building a navmesh's lists and quadtree from its polygons, as the C# does when
//! it reads a navmesh from XML (`Rsc8NavmeshBuilder`, `RebuildLists`,
//! `RebuildQuadTree`, `UpdateTotalBytes`).

use std::collections::HashMap;

use super::{
    Navmesh, NavmeshAabb, NavmeshEdge, NavmeshList, NavmeshPolygon, NavmeshQuadTree, NavmeshQuadTreeData,
    NavmeshVertex, poly_flags,
};

/// `Rsc8NavmeshBuilder`: collects the vertices, indices, edges and special link
/// indices of polygons given with their own vertices.
#[derive(Default)]
pub struct NavmeshBuilder {
    /// Equal positions share a vertex.
    vertex_dict: HashMap<[u32; 3], u16>,
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<u16>,
    pub edges: Vec<NavmeshEdge>,
    pub links_inds: Vec<u16>,
}

/// A `Vector3` as a key of a .NET `Dictionary`: `float.Equals` makes 0 and -0
/// equal, and all NaNs equal.
fn vec3_key(v: [f32; 3]) -> [u32; 3] {
    v.map(|f| {
        if f == 0.0 {
            0
        } else if f.is_nan() {
            0x7FC0_0000
        } else {
            f.to_bits()
        }
    })
}

/// .NET's `Math.Min(float, float)` and `Vector3.Min`: NaN if either is NaN, and
/// -0 below 0.
fn dotnet_min(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else if a == b {
        if a.is_sign_negative() { a } else { b }
    } else if a < b {
        a
    } else {
        b
    }
}

/// .NET's `Math.Max(float, float)` and `Vector3.Max`.
fn dotnet_max(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else if a == b {
        if a.is_sign_negative() { b } else { a }
    } else if a > b {
        a
    } else {
        b
    }
}

fn vmin(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [dotnet_min(a[0], b[0]), dotnet_min(a[1], b[1]), dotnet_min(a[2], b[2])]
}
fn vmax(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [dotnet_max(a[0], b[0]), dotnet_max(a[1], b[1]), dotnet_max(a[2], b[2])]
}

impl NavmeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// `BuildPolygon`: adds a polygon's vertices (and edges, one per vertex) and
    /// special links, and points the polygon at them. Nothing is done for a
    /// polygon without vertices.
    pub fn build_polygon(
        &mut self,
        poly: &mut NavmeshPolygon,
        verts: Option<&[[f32; 3]]>,
        edges: &[NavmeshEdge],
        links: Option<&[u16]>,
    ) {
        let Some(verts) = verts.filter(|v| !v.is_empty()) else { return };
        poly.index_id = self.indices.len() as u32;
        poly.set_index_count(verts.len() as u32);
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for (i, &v) in verts.iter().enumerate() {
            min = vmin(min, v);
            max = vmax(max, v);
            let next = self.vertices.len() as u16;
            let vi = *self.vertex_dict.entry(vec3_key(v)).or_insert_with(|| {
                self.vertices.push(v);
                next
            });
            self.indices.push(vi);
            self.edges.push(edges.get(i).copied().unwrap_or_default());
        }
        poly.cell_aabb = NavmeshAabb::new(min, max);
        poly.set_link_id(self.links_inds.len() as u32);
        poly.set_link_count(links.map_or(0, |l| l.len()) as u8);
        if let Some(links) = links {
            self.links_inds.extend_from_slice(links);
        }
    }
}

impl Navmesh {
    /// `RebuildLists`: the vertex, index, edge and polygon lists from flat
    /// arrays (vertices as positions in the navmesh's box), with each polygon's
    /// part ID set. Empty lists are left out.
    pub fn rebuild_lists(
        &mut self,
        vertices: &[[f32; 3]],
        indices: &[u16],
        edges: &[NavmeshEdge],
        polygons: Option<&[NavmeshPolygon]>,
    ) {
        let (min, size) = (self.aabb_min, self.extents3());
        let packed: Vec<NavmeshVertex> = vertices.iter().map(|&v| NavmeshVertex::new(v, min, size)).collect();
        let mut polys = NavmeshList::rebuild(polygons.unwrap_or_default());
        for (i, part) in polys.list_parts.iter_mut().flatten().enumerate() {
            for p in part.items.iter_mut().flatten() {
                p.set_part_id(i as u8);
            }
        }
        fn non_empty<T>(l: NavmeshList<T>) -> Option<NavmeshList<T>> {
            (l.item_count > 0).then_some(l)
        }
        self.indices = non_empty(NavmeshList::rebuild(indices));
        self.edges = non_empty(NavmeshList::rebuild(edges));
        self.vertices = non_empty(NavmeshList::rebuild(&packed));
        self.polygons = non_empty(polys);
    }

    /// `RebuildQuadTree`: the polygon lookup tree over the navmesh's box.
    pub fn rebuild_quad_tree(&mut self, polygons: &[NavmeshPolygon], vertices: &[[f32; 3]], indices: &[u16]) {
        let cx = TreeContext { polys: polygons, verts: vertices, inds: indices };
        self.quad_tree = Some(Box::new(cx.node(1, self.aabb_min, self.aabb_max, None)));
    }

    /// `UpdateTotalBytes`: the size of the quadtree, the lists and the special
    /// links. `special_links_count` is the one stored in the struct, which the C#
    /// only updates when writing the file (so it's 0 for a navmesh from XML).
    pub fn update_total_bytes(&mut self) {
        let mut n = 0u32;
        if let Some(t) = &self.quad_tree {
            for node in t.nodes() {
                n = n.wrapping_add(node.byte_count());
            }
        }
        n = n.wrapping_add(self.vertices.as_ref().map_or(0, |l| l.byte_count()));
        n = n.wrapping_add(self.indices.as_ref().map_or(0, |l| l.byte_count()));
        n = n.wrapping_add(self.edges.as_ref().map_or(0, |l| l.byte_count()));
        n = n.wrapping_add(self.polygons.as_ref().map_or(0, |l| l.byte_count()));
        n = n.wrapping_add(self.special_links_count.wrapping_mul(32));
        self.total_bytes = n;
    }
}

/// The polygons the quadtree is built over (`nm.CachedPolygons` and so on).
struct TreeContext<'a> {
    polys: &'a [NavmeshPolygon],
    verts: &'a [[f32; 3]],
    inds: &'a [u16],
}

/// The deepest level of the tree.
const MAX_DEPTH: i32 = 5;
/// The smallest node that can split.
const DIM_THRESHOLD: f32 = 10.0;

impl TreeContext<'_> {
    /// The `Rsc8NavmeshQuadTree(depth, min, max, nm, parentpolys)` constructor.
    fn node(&self, depth: i32, min: [f32; 3], max: [f32; 3], parent: Option<&[u16]>) -> NavmeshQuadTree {
        let mut t = NavmeshQuadTree {
            aabb_min: [min[0], min[1], min[2], 0.0],
            aabb_max: [max[0], max[1], max[2], 0.0],
            cell_aabb: NavmeshAabb::new(min, max),
            ..Default::default()
        };
        let cen = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5, (min[2] + max[2]) * 0.5];
        let siz = [max[0] - min[0], max[1] - min[1]];
        let dim = dotnet_max(siz[0], siz[1]);
        // The minimum number of polygons to split a node.
        let poly_threshold = (self.polys.len() / 100).max(50);

        let mut ids: Vec<u16> = Vec::new();
        match parent {
            Some(parent) => {
                // The parent's polygons that are in this node.
                for &i in parent {
                    let Some(poly) = self.polys.get(i as usize) else { continue };
                    if self.overlaps(poly, min, max) {
                        ids.push(i);
                    }
                }
            }
            None => {
                // The root has every polygon with an area.
                for (i, poly) in self.polys.iter().take(0xFFFF).enumerate() {
                    if poly.index_count() < 3 || poly.flags() & poly_flags::ZERO_AREA_STITCH_POLY_DLC != 0 {
                        continue;
                    }
                    ids.push(i as u16);
                }
            }
        }

        if ids.is_empty() {
            t.data = Some(NavmeshQuadTreeData::default());
            return t;
        }

        let mut split = depth < MAX_DEPTH && dim > DIM_THRESHOLD && ids.len() > poly_threshold;
        if split {
            // Clockwise from +X+Y (the Z values are arranged like this in the game files).
            let d = depth + 1;
            let subs = [
                self.node(d, [cen[0], cen[1], cen[2]], [max[0], max[1], max[2]], Some(&ids)),
                self.node(d, [cen[0], min[1], 0.0], [max[0], cen[1], 0.0], Some(&ids)),
                self.node(d, [min[0], min[1], min[2]], [cen[0], cen[1], cen[2]], Some(&ids)),
                self.node(d, [min[0], cen[1], 0.0], [cen[0], max[1], 0.0], Some(&ids)),
            ];
            // Cancel a split that leaves at most one child with polygons (only
            // when no child split).
            if subs.iter().all(|s| s.data.is_some()) {
                let empty = subs.iter().filter(|s| s.data.as_ref().unwrap().polygon_ids_count < 1).count();
                if empty >= 3 {
                    split = false;
                }
            }
            if split {
                t.sub_trees = subs.map(|s| Some(Box::new(s)));
            }
        }
        if !split {
            t.data = Some(NavmeshQuadTreeData {
                polygon_ids_count: ids.len() as u16,
                polygon_ids: Some(ids),
                ..Default::default()
            });
        }
        t
    }

    /// Whether a polygon is in a node (in X and Y): any of its vertices is, or
    /// its box overlaps the node's.
    fn overlaps(&self, poly: &NavmeshPolygon, min: [f32; 3], max: [f32; 3]) -> bool {
        let start = poly.index_id;
        let mut pmin = [f32::MAX; 3];
        let mut pmax = [f32::MIN; 3];
        for j in 0..poly.index_count() {
            let ii = start.wrapping_add(j) as usize;
            let Some(&vi) = self.inds.get(ii) else { continue };
            let Some(&v) = self.verts.get(vi as usize) else { continue };
            if v[0] >= min[0] && v[1] >= min[1] && v[0] <= max[0] && v[1] <= max[1] {
                return true;
            }
            pmin = vmin(pmin, v);
            pmax = vmax(pmax, v);
        }
        if pmin[0] > pmax[0] || pmin[1] > pmax[1] {
            return false; // No vertices.
        }
        if pmin[0] > max[0] || pmin[1] > max[1] {
            return false;
        }
        if pmax[0] < min[0] || pmax[1] < min[1] {
            return false;
        }
        true
    }
}
