//! A ymap's instanced props and grass (`instancedData`): RDR2's trees,
//! bushes, rocks and grass, which aren't entities but batches of instances
//! of one archetype. Read as the C#'s `RDR2PropBatch` and `RDR2GrassBatch`
//! read them, with the instances grouped by the batch's population grid
//! (`EntityBatchGrid`), whose cells each pick their LOD.
//!
//! These fields are hashed case-sensitively, unlike the entities'.

use rf_formats::formats::databag::DataBag;

use crate::math::{Aabb, Vec3};

const INSTANCED_DATA: u32 = 0x9920_DC29;
const PROP_INSTANCE_LIST: u32 = 0xD3AF_3360;
const GRASS_INSTANCE_LIST: u32 = 0x0F37_73DD;
const INSTANCE_LIST: u32 = 0x1C08_0BB9;
const INSTANCE_LOD_DIST: u32 = 0xC015_6D12;
const BATCH_AABB: u32 = 0x6ECE_BA6E;
const MIN: u32 = 0xFE30_E6F3;
const MAX: u32 = 0x6060_68C4;
const POPULATION_GRID_DATA: u32 = 0xAB13_0376;
const CELL_SIZE: u32 = 0x0C70_D409;
const GRID_SIZE_X: u32 = 0x6BC7_D2C4;
const GRID_SIZE_Y: u32 = 0x1E6D_3810;
const SCALE_RANGE: u32 = 0xA066_4EE6;
const ORIENT_TO_TERRAIN: u32 = 0xC72A_DEFA;
const IS_SLOD_LIST: u32 = 0x4579_099D;
const INST_MAT: u32 = 0x7EE6_CC43;
const POSITION: u32 = 0x0C92_8991;
const NORMAL_X: u32 = 0xBB0B_13F0;
const NORMAL_Y: u32 = 0x1051_BE7C;
const SCALE: u32 = 0x3CBA_3FE6;
const ROTATION_IDX: u32 = 0xBA36_796D;
const RCP_SCALE_XY: u32 = 0x95D3_A60C;
const RCP_SCALE_Z: u32 = 0xDD3A_8216;

/// One grass instance, as stored (`EntityBatchInstance2`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct GrassInstance {
    /// In the batch's box, 0-65535.
    position: [u16; 3],
    /// The terrain's normal's x and y, 0-255.
    normal: [u8; 2],
    scale: u8,
    /// Turned by this many 5° steps about Z.
    rotation: u8,
}

#[derive(Clone, Debug, PartialEq)]
enum Instances {
    /// Each a 3x4 matrix as stored: rows of the axes' images, with the
    /// position in the fourth column.
    Props(Vec<[f32; 12]>),
    Grass {
        instances: Vec<GrassInstance>,
        /// `ScaleRange`'s minimum and maximum.
        scale: [f32; 2],
        /// How much the grass leans with the slope.
        orient_to_terrain: f32,
    },
}

/// A cell of the batch's population grid that has instances.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatchCell {
    pub bounds: Aabb,
    /// Its instances, in [`InstanceBatch::transform`]'s order.
    pub start: u32,
    pub end: u32,
}

/// A batch of instances of one archetype.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceBatch {
    pub archetype: u32,
    pub grass: bool,
    /// Its place in its list (`PropInstanceList` or `GrassInstanceList`).
    pub list_index: u32,
    pub bounds: Aabb,
    /// How far its instances show (`InstanceLodDist`).
    pub instance_lod_dist: f32,
    /// How far from it the batch is streamed: the C#'s `LodDistMax`, the
    /// instance distance or a bit more than its radius.
    pub lod_dist_max: f32,
    pub cells: Vec<BatchCell>,
    instances: Instances,
    /// Per instance, its index in the batch's `InstanceList` (they're
    /// reordered by cell).
    source: Vec<u32>,
}

impl InstanceBatch {
    pub fn len(&self) -> usize {
        match &self.instances {
            Instances::Props(p) => p.len(),
            Instances::Grass { instances, .. } => instances.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// An instance's transform, model to world, column-major (as
    /// `glam::Mat4::from_cols_array`). Grass is turned and scaled as the
    /// C#'s instance shader does it, and sheared to follow the slope.
    pub fn transform(&self, i: usize) -> [f32; 16] {
        match &self.instances {
            Instances::Props(p) => prop_transform(&p[i]),
            Instances::Grass { instances, scale, orient_to_terrain } => {
                grass_transform(&self.bounds, *scale, *orient_to_terrain, &instances[i])
            }
        }
    }

    /// Moves an instance, as the C#'s `SetInstanceTransform`: a prop takes
    /// the transform as it is; grass its position (in the batch's box), its
    /// scale (in the scale range) and its turn about Z.
    pub fn set_transform(&mut self, i: usize, m: &[f32; 16]) {
        let bounds = self.bounds;
        match &mut self.instances {
            Instances::Props(p) => p[i] = prop_matrix(m),
            Instances::Grass { instances, scale, .. } => set_grass_transform(&bounds, *scale, &mut instances[i], m),
        }
    }

    /// An instance's index in the batch's `InstanceList`.
    pub fn source_index(&self, i: usize) -> u32 {
        self.source[i]
    }

    /// Where the instance of the `InstanceList` at `source` is.
    pub fn index_of(&self, source: u32) -> Option<usize> {
        self.source.iter().position(|&s| s == source)
    }

    /// The terrain's normal a grass instance is lit with instead of its
    /// own (the C#'s "use instance normals"); `None` for props.
    pub fn normal(&self, i: usize) -> Option<Vec3> {
        match &self.instances {
            Instances::Props(_) => None,
            Instances::Grass { instances, .. } => Some(grass_normal(&instances[i])),
        }
    }

    fn position(&self, i: usize) -> Vec3 {
        match &self.instances {
            Instances::Props(p) => [p[i][3], p[i][7], p[i][11]],
            Instances::Grass { instances, .. } => grass_position(&self.bounds, &instances[i]),
        }
    }

    fn read(b: &DataBag, grass: bool, list_index: u32) -> Option<Self> {
        let items = b.bags(INSTANCE_LIST)?;
        b.bag(BATCH_AABB)?;
        let bounds = batch_bounds(b);
        let instances = if grass {
            if b.bool(IS_SLOD_LIST) {
                return None;
            }
            let range = b.vec3(SCALE_RANGE);
            Instances::Grass {
                instances: items.iter().flatten().map(GrassInstance::read).collect(),
                scale: [range[0], range[1]],
                orient_to_terrain: b.f32(ORIENT_TO_TERRAIN),
            }
        } else {
            Instances::Props(items.iter().flatten().map(|i| i.matrix3x4(INST_MAT)).collect())
        };
        let instance_lod_dist = b.f32(INSTANCE_LOD_DIST);
        let size = [0, 1, 2].map(|k| bounds.max[k] - bounds.min[k]);
        let radius = (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt() * 0.5;
        let count = items.iter().flatten().count() as u32;
        let mut batch = InstanceBatch {
            archetype: b.hash(0xA023_A02C), // "archetypeName"
            grass,
            list_index,
            bounds,
            instance_lod_dist,
            lod_dist_max: instance_lod_dist.max(radius * 1.1),
            cells: Vec::new(),
            instances,
            source: (0..count).collect(),
        };
        if batch.is_empty() {
            return None;
        }
        let grid = b.bag(POPULATION_GRID_DATA);
        let cell_size = grid.map_or(0.0, |g| g.f32(CELL_SIZE));
        let (sx, sy, cs) = if cell_size > 0.0 {
            let g = grid.unwrap();
            (g.i8(GRID_SIZE_X).max(1) as usize, g.i8(GRID_SIZE_Y).max(1) as usize, cell_size)
        } else {
            auto_grid(size)
        };
        batch.group_by_cell(sx, sy, cs);
        Some(batch)
    }

    /// `EntityBatchGrid`: puts the instances in order of their cells, and
    /// keeps each cell's box and range.
    fn group_by_cell(&mut self, sx: usize, sy: usize, cs: f32) {
        let min = self.bounds.min;
        let cell_of = |p: Vec3| {
            let cx = (((p[0] - min[0]) / cs).max(0.0) as usize).min(sx - 1);
            let cy = (((p[1] - min[1]) / cs).max(0.0) as usize).min(sy - 1);
            cy * sx + cx
        };
        let n = self.len();
        let cells: Vec<usize> = (0..n).map(|i| cell_of(self.position(i))).collect();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| cells[i]);
        match &mut self.instances {
            Instances::Props(p) => *p = order.iter().map(|&i| p[i]).collect(),
            Instances::Grass { instances, .. } => *instances = order.iter().map(|&i| instances[i]).collect(),
        }
        self.source = order.iter().map(|&i| self.source[i]).collect();
        let mut start = 0;
        while start < n {
            let c = cells[order[start]];
            let end = (start..n).find(|&k| cells[order[k]] != c).unwrap_or(n);
            let (x, y) = ((c % sx) as f32, (c / sx) as f32);
            let cmin = [min[0] + x * cs, min[1] + y * cs, self.bounds.min[2]];
            let cmax = [cmin[0] + cs, cmin[1] + cs, self.bounds.max[2]];
            self.cells.push(BatchCell { bounds: Aabb::new(cmin, cmax), start: start as u32, end: end as u32 });
            start = end;
        }
    }
}

impl GrassInstance {
    fn read(i: &DataBag) -> Self {
        let p = i.embedded_array::<u16>(POSITION).unwrap_or_default();
        GrassInstance {
            position: [0, 1, 2].map(|k| p.get(k).copied().unwrap_or(0)),
            normal: [i.u8(NORMAL_X), i.u8(NORMAL_Y)],
            scale: i.u8(SCALE),
            rotation: i.u8(ROTATION_IDX),
        }
    }

    fn write(&self, i: &mut DataBag) {
        i.set_embedded_array(POSITION, &self.position);
        i.set_u8(SCALE, self.scale);
        i.set_u8(ROTATION_IDX, self.rotation);
    }
}

/// A prop instance's stored matrix as a transform.
fn prop_transform(m: &[f32; 12]) -> [f32; 16] {
    [m[0], m[1], m[2], 0.0, m[4], m[5], m[6], 0.0, m[8], m[9], m[10], 0.0, m[3], m[7], m[11], 1.0]
}

/// A transform as a prop instance's stored matrix.
fn prop_matrix(m: &[f32; 16]) -> [f32; 12] {
    [m[0], m[1], m[2], m[12], m[4], m[5], m[6], m[13], m[8], m[9], m[10], m[14]]
}

fn grass_position(bounds: &Aabb, g: &GrassInstance) -> Vec3 {
    let (min, max) = (bounds.min, bounds.max);
    [0, 1, 2].map(|k| min[k] + (max[k] - min[k]) * g.position[k] as f32 / 65535.0)
}

/// A grass instance's transform: turned and scaled as the C#'s instance
/// shader does it, and sheared to follow the slope.
fn grass_transform(bounds: &Aabb, scale: [f32; 2], orient: f32, g: &GrassInstance) -> [f32; 16] {
    let p = grass_position(bounds, g);
    let [nx, ny, _] = grass_normal(g);
    let s = scale[0] + g.scale as f32 / 255.0 * (scale[1] - scale[0]);
    let a = std::f32::consts::PI * (g.rotation % 72) as f32 / 36.0;
    let (sin, cos) = a.sin_cos();
    let (c0, c1) = ([s * cos, s * sin], [-s * sin, s * cos]);
    // z -= (n.xy · xy) × orient, after turning and scaling.
    let shear = |c: [f32; 2]| -orient * (nx * c[0] + ny * c[1]);
    [c0[0], c0[1], shear(c0), 0.0, c1[0], c1[1], shear(c1), 0.0, 0.0, 0.0, s, 0.0, p[0], p[1], p[2], 1.0]
}

/// Grass placed by a transform: its position (clamped to the batch's box),
/// its scale and its turn about Z, in 5° steps.
fn set_grass_transform(bounds: &Aabb, scale: [f32; 2], g: &mut GrassInstance, m: &[f32; 16]) {
    let range = (scale[1] - scale[0]).max(1e-6);
    let s = m[10].abs();
    g.scale = ((s - scale[0]) / range * 255.0).clamp(0.0, 255.0) as u8;
    g.position = [0, 1, 2].map(|k| {
        let size = (bounds.max[k] - bounds.min[k]).max(1e-6);
        ((m[12 + k] - bounds.min[k]) / size * 65535.0).clamp(0.0, 65535.0) as u16
    });
    let yaw = m[1].atan2(m[0]).rem_euclid(std::f32::consts::TAU);
    g.rotation = ((yaw / std::f32::consts::TAU * 72.0).round() as u32 % 72) as u8;
}

/// A batch's `InstanceList` item placed in the world (the transform its
/// batch draws it with).
pub fn instance_transform(batch: &DataBag, grass: bool, instance: &DataBag) -> [f32; 16] {
    if grass {
        let range = batch.vec3(SCALE_RANGE);
        grass_transform(
            &batch_bounds(batch),
            [range[0], range[1]],
            batch.f32(ORIENT_TO_TERRAIN),
            &GrassInstance::read(instance),
        )
    } else {
        prop_transform(&instance.matrix3x4(INST_MAT))
    }
}

/// Moves a batch's `InstanceList` item to a transform ([`InstanceBatch::set_transform`]'s
/// rules); a prop's reciprocal scales follow.
pub fn set_instance_transform(batch: &DataBag, grass: bool, instance: &mut DataBag, m: &[f32; 16]) {
    if grass {
        let range = batch.vec3(SCALE_RANGE);
        let mut g = GrassInstance::read(instance);
        set_grass_transform(&batch_bounds(batch), [range[0], range[1]], &mut g, m);
        g.write(instance);
    } else {
        instance.set_matrix3x4(INST_MAT, prop_matrix(m));
        let len = |c: usize| (m[c] * m[c] + m[c + 1] * m[c + 1] + m[c + 2] * m[c + 2]).sqrt().max(1e-6);
        instance.set_f32(RCP_SCALE_XY, 1.0 / len(0));
        instance.set_f32(RCP_SCALE_Z, 1.0 / len(8));
    }
}

/// A batch's `BatchAABB`.
pub fn batch_bounds(batch: &DataBag) -> Aabb {
    match batch.bag(BATCH_AABB) {
        Some(b) => {
            let (min, max) = (b.vec4(MIN), b.vec4(MAX));
            Aabb::new([min[0], min[1], min[2]], [max[0], max[1], max[2]])
        }
        None => Aabb::EMPTY,
    }
}

fn grass_normal(g: &GrassInstance) -> Vec3 {
    let x = g.normal[0] as f32 / 255.0 * 2.0 - 1.0;
    let y = g.normal[1] as f32 / 255.0 * 2.0 - 1.0;
    [x, y, (1.0 - x * x - y * y).max(0.0).sqrt()]
}

/// `InitGrid()` without a population grid: 32 cells along the longer side.
fn auto_grid(size: Vec3) -> (usize, usize, f32) {
    const MAX: f32 = 32.0;
    let (csx, csy) = (size[0] / MAX, size[1] / MAX);
    let cs = csx.max(csy).max(1e-3);
    if csx >= csy {
        (MAX as usize, (size[1] / cs) as usize + 1, cs)
    } else {
        ((size[0] / cs) as usize + 1, MAX as usize, cs)
    }
}

/// `InitInstances`: a `CMapData`'s prop and grass batches.
pub fn read_batches(map: &DataBag) -> Vec<InstanceBatch> {
    let Some(data) = map.bag(INSTANCED_DATA) else { return Vec::new() };
    let lists = [(PROP_INSTANCE_LIST, false), (GRASS_INSTANCE_LIST, true)];
    lists
        .iter()
        .flat_map(|&(list, grass)| {
            data.bags(list)
                .into_iter()
                .flatten()
                .enumerate()
                .filter_map(move |(i, b)| InstanceBatch::read(b.as_ref()?, grass, i as u32))
        })
        .collect()
}

/// A `CMapData`'s batch list (props or grass), for changing it.
pub fn batch_list_mut(map: &mut DataBag, grass: bool) -> Option<&mut Vec<Option<DataBag>>> {
    let list = if grass { GRASS_INSTANCE_LIST } else { PROP_INSTANCE_LIST };
    map.bag_mut(INSTANCED_DATA)?.bags_mut(list)
}

pub fn batch_list(map: &DataBag, grass: bool) -> Option<&[Option<DataBag>]> {
    let list = if grass { GRASS_INSTANCE_LIST } else { PROP_INSTANCE_LIST };
    map.bag(INSTANCED_DATA)?.bags(list)
}

/// A batch's `InstanceList`.
pub fn instance_list(batch: &DataBag) -> Option<&[Option<DataBag>]> {
    batch.bags(INSTANCE_LIST)
}

pub fn instance_list_mut(batch: &mut DataBag) -> Option<&mut Vec<Option<DataBag>>> {
    batch.bags_mut(INSTANCE_LIST)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grass(instances: Vec<GrassInstance>) -> InstanceBatch {
        InstanceBatch {
            archetype: 1,
            grass: true,
            bounds: Aabb::new([0.0, 0.0, 10.0], [100.0, 50.0, 20.0]),
            instance_lod_dist: 100.0,
            lod_dist_max: 100.0,
            cells: Vec::new(),
            source: (0..instances.len() as u32).collect(),
            instances: Instances::Grass { instances, scale: [1.0, 3.0], orient_to_terrain: 1.0 },
            list_index: 0,
        }
    }

    #[test]
    fn grass_is_placed_turned_and_leaned() {
        let flat = GrassInstance { position: [65535, 0, 32768], normal: [128, 128], scale: 255, rotation: 18 };
        let b = grass(vec![flat]);
        let m = b.transform(0);
        // At the box's far x, its near y, half way up; scale 3, turned 90°.
        assert!((m[12] - 100.0).abs() < 1e-3 && m[13].abs() < 1e-3 && (m[14] - 15.0).abs() < 1e-3);
        assert!(m[0].abs() < 1e-5 && (m[1] - 3.0).abs() < 1e-5 && (m[10] - 3.0).abs() < 1e-5);
        // On a slope rising along +x (normal leaning to -x), x moves z up.
        let slope = GrassInstance { normal: [64, 128], rotation: 0, ..flat };
        let m = grass(vec![slope]).transform(0);
        assert!(m[2] > 0.5, "{m:?}");
    }

    #[test]
    fn instances_go_by_cell() {
        let at = |x: u16, y: u16| GrassInstance { position: [x, y, 0], ..Default::default() };
        let mut b = grass(vec![at(60000, 60000), at(100, 100), at(60000, 60000), at(100, 40000)]);
        b.group_by_cell(4, 2, 25.0);
        let cells: Vec<(u32, u32)> = b.cells.iter().map(|c| (c.start, c.end)).collect();
        assert_eq!(cells, vec![(0, 1), (1, 2), (2, 4)]);
        assert_eq!(b.cells[0].bounds.min, [0.0, 0.0, 10.0]);
        assert_eq!(b.cells[2].bounds.min, [75.0, 25.0, 10.0]);
    }
}
