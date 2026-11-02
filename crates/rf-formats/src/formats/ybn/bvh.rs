//! `Rsc8BVH`, the bounding volume hierarchy of BVH geometries and composites,
//! and `Rsc8BVHBuilder`, which the C# runs every time it writes one.

use bytemuck::{Pod, Zeroable};

use super::math::{self, Vec3};
use super::reader::{BoundsReader, Tag};
use crate::Result;
use crate::rsc8::{ArrayHeader32, BlockBuf, BlockId, Rsc8Writer};

/// A node: its box quantized relative to the BVH's centre, then either the
/// first item and the item count (leaves), or the number of nodes in its
/// subtree and 0.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct BvhNode {
    pub min: [i16; 3],
    pub max: [i16; 3],
    pub item_id: i16,
    pub item_count: i16,
}

/// `Rsc8BVH` (112 bytes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bvh {
    pub bounding_box_min: [f32; 4],
    pub bounding_box_max: [f32; 4],
    pub bounding_box_center: [f32; 4],
    pub quantum_inverse: [f32; 4],
    /// The box's half size / 32767.
    pub quantum: [f32; 4],
    /// All the nodes written (`nodes_capacity` of them when built here); the
    /// C# reads `nodes_count`.
    pub nodes: Option<Vec<BvhNode>>,
    pub nodes_count: u32,
    pub nodes_capacity: u32,
    pub unknown_60h: u64,
    pub unknown_68h: u64,
}

impl Bvh {
    pub(crate) fn read(br: &mut BoundsReader) -> Result<Self> {
        let r = &mut *br.r;
        let mut b = Bvh {
            bounding_box_min: r.vec4()?,
            bounding_box_max: r.vec4()?,
            bounding_box_center: r.vec4()?,
            quantum_inverse: r.vec4()?,
            quantum: r.vec4()?,
            ..Default::default()
        };
        let nodes = ArrayHeader32::read(r)?;
        b.nodes_count = nodes.count;
        b.nodes_capacity = nodes.capacity;
        b.unknown_60h = r.u64()?;
        b.unknown_68h = r.u64()?;
        b.nodes = br.array(Tag::BvhNode, nodes.ptr, nodes.count as usize)?;
        Ok(b)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let nodes = w.array(self.nodes.as_deref());
        let mut b = BlockBuf::with_capacity(112);
        b.vec4(self.bounding_box_min);
        b.vec4(self.bounding_box_max);
        b.vec4(self.bounding_box_center);
        b.vec4(self.quantum_inverse);
        b.vec4(self.quantum);
        b.ptr(nodes);
        b.u32(self.nodes_count);
        b.u32(self.nodes_capacity);
        b.u64(self.unknown_60h);
        b.u64(self.unknown_68h);
        w.add(b, false)
    }
}

/// An item to put in a BVH: a polygon or a composite's child, by its box.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BuildItem {
    pub min: Vec3,
    pub max: Vec3,
    pub index: i32,
}

struct BuildNode {
    children: Option<Vec<BuildNode>>,
    /// Indices into the items.
    items: Option<Vec<usize>>,
    min: Vec3,
    max: Vec3,
}

impl BuildNode {
    fn new(items: Vec<usize>) -> Self {
        Self { children: None, items: Some(items), min: [0.0; 3], max: [0.0; 3] }
    }

    fn total_nodes(&self) -> i32 {
        1 + self.children.iter().flatten().map(|c| c.total_nodes()).sum::<i32>()
    }

    fn total_items(&self) -> i32 {
        self.items.as_ref().map_or(0, |i| i.len() as i32)
            + self.children.iter().flatten().map(|c| c.total_items()).sum::<i32>()
    }

    fn update_min_max(&mut self, items: &[BuildItem]) {
        let mut min = math::splat(f32::MAX);
        let mut max = math::splat(f32::MIN);
        for &i in self.items.iter().flatten() {
            min = math::min(min, items[i].min);
            max = math::max(max, items[i].max);
        }
        for c in self.children.iter_mut().flatten() {
            c.update_min_max(items);
            min = math::min(min, c.min);
            max = math::max(max, c.max);
        }
        self.min = min;
        self.max = max;
    }

    /// `Rsc8BVHBuilderNode.Build`: splits the items in two around their mean
    /// centre, on the axis that splits them most evenly.
    fn build(&mut self, items: &[BuildItem], threshold: usize) {
        self.update_min_max(items);
        let Some(mine) = &self.items else { return };
        if mine.len() <= threshold {
            return;
        }
        let n = mine.len();
        let mut avgsum = [0.0f32; 3];
        for &i in mine {
            avgsum = math::add(avgsum, items[i].min);
            avgsum = math::add(avgsum, items[i].max);
        }
        let avg = math::scale(avgsum, 0.5 / n as f32);
        let centre = |i: usize| math::scale(math::add(items[i].min, items[i].max), 0.5);
        let mut counts = [0i32; 3];
        for &i in mine {
            let c = centre(i);
            for a in 0..3 {
                if c[a] < avg[a] {
                    counts[a] += 1;
                }
            }
        }
        let target = n as f32 / 2.0;
        let d = counts.map(|c| (target - c as f32).abs());
        let axis = if d[0] <= d[1] && d[0] <= d[2] {
            0
        } else if d[1] <= d[2] {
            1
        } else {
            2
        };
        let (mut l1, mut l2): (Vec<usize>, Vec<usize>) = mine.iter().partition(|&&i| centre(i)[axis] > avg[axis]);
        if l1.is_empty() || l2.is_empty() {
            let mut l3: Vec<usize> = l1.iter().chain(&l2).copied().collect();
            if l3.is_empty() {
                return;
            }
            math::dotnet_sort(&mut l3, &|&a: &usize, &b: &usize| {
                let c = math::compare_vec3(items[a].min, items[b].min);
                if c != 0 { c } else { math::compare_vec3(items[a].max, items[b].max) }
            });
            let half = l3.len() / 2;
            l2 = l3.split_off(half);
            l1 = l3;
        }
        self.items = None;
        let mut n1 = BuildNode::new(l1);
        n1.build(items, threshold);
        let mut n2 = BuildNode::new(l2);
        n2.build(items, threshold);
        // Sorted by item count, largest first (a 2-item .NET sort swaps only
        // when the second is strictly larger).
        self.children = Some(if n2.total_items() > n1.total_items() { vec![n2, n1] } else { vec![n1, n2] });
    }

    fn gather<'a>(&'a self, out: &mut Vec<&'a BuildNode>) {
        out.push(self);
        for c in self.children.iter().flatten() {
            c.gather(out);
        }
    }
}

/// `Vector3` to the node's shorts (`Rsc8BVHNode.Min`/`Max` setters).
fn quantize(v: f32, round: fn(f32) -> f32) -> i16 {
    // `FloatUtil.Clamp` (NaN stays NaN, then becomes 0 as a short).
    round(v).clamp(-32767.0, 32767.0) as i32 as i16
}

/// The default NaN of x64, which is .NET's `float.NaN`.
const DOTNET_NAN: f32 = f32::from_bits(0xFFC0_0000);

/// `Rsc8BVHBuilder.Build`. `items` may have holes (a composite's null
/// children), which count towards a composite's node capacity. Returns the
/// BVH, and the items' new order when `threshold > 1` (geometries regroup
/// their polygons by node); items' `index` is then their new position.
pub(crate) fn build(
    items: &mut [Option<BuildItem>],
    threshold: usize,
    force: Option<(Vec3, Vec3)>,
) -> (Bvh, Option<Vec<usize>>) {
    let present: Vec<usize> = (0..items.len()).filter(|&i| items[i].is_some()).collect();
    let mut arena: Vec<BuildItem> = present.iter().map(|&i| items[i].unwrap()).collect();
    let mut min = math::splat(f32::MAX);
    let mut max = math::splat(f32::MIN);
    for it in &arena {
        min = math::min(min, it.min);
        max = math::max(max, it.max);
    }
    if let Some((fmin, fmax)) = force {
        min = fmin;
        max = fmax;
    }
    let cen = math::scale(math::add(min, max), 0.5);
    let quantum = math::div_scalar(math::max(math::abs(math::sub(min, cen)), math::abs(math::sub(max, cen))), 32767.0);
    let qi = math::div(math::splat(1.0), quantum);
    let mut bvh = Bvh {
        bounding_box_min: [min[0], min[1], min[2], f32::from_bits(1)],
        bounding_box_max: [max[0], max[1], max[2], 0.0],
        bounding_box_center: [cen[0], cen[1], cen[2], 0.0],
        quantum_inverse: [qi[0], qi[1], qi[2], DOTNET_NAN],
        quantum: [quantum[0], quantum[1], quantum[2], 0.0],
        ..Default::default()
    };

    let mut root = BuildNode::new((0..arena.len()).collect());
    root.build(&arena, threshold);
    let mut nodes = Vec::new();
    root.gather(&mut nodes);

    let mut order = None;
    if threshold > 1 {
        let mut o = Vec::with_capacity(arena.len());
        for n in &nodes {
            for &i in n.items.iter().flatten() {
                arena[i].index = o.len() as i32;
                o.push(present[i]);
            }
        }
        for (pos, &src) in o.iter().enumerate() {
            if let Some(it) = &mut items[src] {
                it.index = pos as i32;
            }
        }
        order = Some(o);
    }

    let mut out = Vec::with_capacity(nodes.len());
    for n in &nodes {
        let id = n.items.as_ref().and_then(|v| v.first()).map_or(0, |&i| arena[i].index);
        let tn = n.total_nodes();
        let lo = math::mul(math::sub(n.min, cen), qi);
        let hi = math::mul(math::sub(n.max, cen), qi);
        out.push(BvhNode {
            min: lo.map(|v| quantize(v, f32::floor)),
            max: hi.map(|v| quantize(v, f32::ceil)),
            item_count: if tn <= 1 { n.total_items() as i16 } else { 0 },
            item_id: if tn <= 1 { id as i16 } else { tn as i16 },
        });
    }
    let count = out.len();
    if threshold <= 1 {
        // Composites: room for (children * 2) + 1 nodes, the rest empty.
        let capacity = items.len() * 2 + 1;
        while out.len() < capacity {
            out.push(BvhNode { item_id: 1, ..Default::default() });
        }
    }
    bvh.nodes_count = count as u32;
    bvh.nodes_capacity = out.len() as u32;
    bvh.nodes = Some(out);
    (bvh, order)
}
