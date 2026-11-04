//! A collision's parts: what the C#'s `CreateEditablePartsPiece` makes of a
//! `Collider` (an `Rsc8Bounds`), one part per bound with a shape. The parts
//! are in the bounds' own space (the world's for a ybn, the model's for a
//! drawable), and their meshes are still on the CPU, so this runs off the UI
//! thread.

use glam::{Mat4, Vec3};
use rf_formats::formats::ybn::{
    BOUND_FLAG_NAMES, BoundGeometry, Bounds, BoundsShape, BoundsType, PolygonKind, flags_to_string,
};
use rf_game::materials::Materials;

use super::super::PartInfo;
use super::super::shapes::Geometry;

/// A part's mesh before it's on the GPU.
pub enum Shape {
    /// The shared unit box (-0.5 to 0.5), through the part's transform.
    UnitBox,
    /// The shared unit sphere (radius 1), through the part's transform.
    UnitSphere,
    Mesh(Geometry),
}

/// One part of a collision, in the space of its bounds.
pub struct Part {
    pub shape: Shape,
    pub transform: Mat4,
    /// Straight RGBA, multiplying the mesh's.
    pub colour: [u8; 4],
    /// A sphere around it, in the space of its bounds.
    pub centre: Vec3,
    pub radius: f32,
    /// What Properties shows (without the owner).
    pub info: PartInfo,
}

const WHITE: [u8; 4] = [255; 4];

/// The parts of a collision, in the order the C# flattens them: a
/// composite's children (each placed by its matrix) in turn.
pub fn parts(root: &Bounds, materials: &Materials) -> Vec<Part> {
    let mut out = Vec::new();
    add(root, Mat4::IDENTITY, None, materials, &mut out);
    out
}

fn v3(v: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", v[0], v[1], v[2])
}

/// The largest scale of a transform's axes.
pub fn max_scale(m: &Mat4) -> f32 {
    m.x_axis.truncate().length().max(m.y_axis.truncate().length()).max(m.z_axis.truncate().length())
}

/// Where a composite puts a child, as the C#'s `Matrix3x4` of it: without
/// the matrix's 4th column (M14, M24, M34), which holds other values in some
/// models' bounds and would make the transform projective.
pub fn child_transform(m: &[f32; 16]) -> Mat4 {
    let mut m = *m;
    (m[3], m[7], m[11], m[15]) = (0.0, 0.0, 0.0, 1.0);
    Mat4::from_cols_array(&m)
}

fn add(b: &Bounds, parent: Mat4, index: Option<usize>, materials: &Materials, out: &mut Vec<Part>) {
    let place = parent * child_transform(&b.transform);
    if let BoundsShape::Composite(c) = &b.shape {
        for (i, child) in c.children.iter().flatten().enumerate() {
            if let Some(child) = child {
                add(child, place, Some(i), materials, out);
            }
        }
        return;
    }
    let colour = materials.colour(b.material.hash());
    let (min, max) = (Vec3::from(b.box_min), Vec3::from(b.box_max));
    let centre = Vec3::from(b.box_center);
    // The capsule's and cylinder's axis is Y (`Shape.CreateCapsule`); a disc
    // is a cylinder about X, as wheels turn.
    let tube = |radius: f32, length: f32, axis: Vec3| {
        let half = axis * (length * 0.5);
        (centre - half, centre + half, radius)
    };
    let (shape, transform, colour) = match (&b.shape, b.bounds_type) {
        (BoundsShape::Basic, BoundsType::Sphere) => {
            let m = Mat4::from_translation(b.sphere_center.into()) * Mat4::from_scale(Vec3::splat(b.sphere_radius));
            (Shape::UnitSphere, place * m, colour)
        }
        (BoundsShape::Basic, BoundsType::Box | BoundsType::Plane) => {
            let m = Mat4::from_translation(centre) * Mat4::from_scale(max - min);
            (Shape::UnitBox, place * m, colour)
        }
        (BoundsShape::Capsule(_), _) => {
            let (a, z, r) = tube(b.margin, (b.sphere_radius - b.margin) * 2.0, Vec3::Y);
            let mut g = Geometry::default();
            g.add_capsule(a, z, r, WHITE);
            (Shape::Mesh(g), place, colour)
        }
        (BoundsShape::Cylinder(_), _) => {
            let extent = (max - min).abs();
            let (a, z, r) = tube(extent.x * 0.5, extent.y, Vec3::Y);
            let mut g = Geometry::default();
            g.add_cylinder(a, z, r, WHITE);
            (Shape::Mesh(g), place, colour)
        }
        (BoundsShape::Disc(_), _) => {
            let (a, z, r) = tube(b.sphere_radius, b.margin * 2.0, Vec3::X);
            let mut g = Geometry::default();
            g.add_cylinder(a, z, r, WHITE);
            (Shape::Mesh(g), place, colour)
        }
        (BoundsShape::Geometry(g) | BoundsShape::GeometryBvh(g, _), _) => {
            let mesh = geometry_mesh(g, materials);
            if mesh.is_empty() {
                return;
            }
            (Shape::Mesh(mesh), place, WHITE)
        }
        // Tapered capsules, triangles, morph geometries, rings and cloth have
        // no part shape in the C#.
        _ => return,
    };
    let (centre, radius) = match &shape {
        Shape::UnitBox => (transform.w_axis.truncate(), 0.87 * max_scale(&transform)),
        Shape::UnitSphere => (transform.w_axis.truncate(), max_scale(&transform)),
        Shape::Mesh(g) => {
            let (lo, hi) = g.vertices.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), v| {
                let p = Vec3::from(v.position);
                (lo.min(p), hi.max(p))
            });
            (transform.transform_point3((lo + hi) * 0.5), (hi - lo).length() * 0.5 * max_scale(&transform))
        }
    };
    let info = info(b, index, &shape, materials);
    out.push(Part { shape, transform, colour, centre, radius, info });
}

/// `Rsc8BoundGeometry.CreateMesh`: the triangles, and the spheres, capsules,
/// boxes and cylinders among the polygons, each in its material's colour
/// (white past the materials).
pub fn geometry_mesh(g: &BoundGeometry, materials: &Materials) -> Geometry {
    let mut mesh = Geometry::default();
    let vertices = g.vertices.as_deref().unwrap_or_default();
    let offset = Vec3::from(g.center_geom);
    let v = |i: i64| usize::try_from(i).ok().and_then(|i| vertices.get(i)).map(|p| Vec3::from(*p) + offset);
    let colours: Vec<[u8; 4]> = g.materials.iter().flatten().map(|m| materials.colour(m.data1.hash())).collect();
    for p in &g.polygons {
        let colour = usize::try_from(p.material_index).ok().and_then(|i| colours.get(i)).copied().unwrap_or(WHITE);
        match p.kind {
            PolygonKind::Triangle(t) => {
                let ids = [t.vert_index1, t.vert_index2, t.vert_index3].map(|i| v(i as i64));
                if let [Some(a), Some(b), Some(c)] = ids {
                    mesh.add_triangle(a, b, c, colour);
                }
            }
            PolygonKind::Sphere(s) => {
                if let Some(c) = v(s.vert_index as i64) {
                    mesh.add_sphere(c, s.sphere_radius, colour);
                }
            }
            PolygonKind::Capsule(c) => {
                if let (Some(a), Some(b)) = (v(c.vert_index1 as i64), v(c.vert_index2 as i64)) {
                    mesh.add_capsule(a, b, c.capsule_radius, colour);
                }
            }
            PolygonKind::Cylinder(c) => {
                if let (Some(a), Some(b)) = (v(c.vert_index1 as i64), v(c.vert_index2 as i64)) {
                    mesh.add_cylinder(a, b, c.capsule_radius, colour);
                }
            }
            PolygonKind::Box(bx) => {
                let ids = [bx.vert_index1, bx.vert_index2, bx.vert_index3, bx.vert_index4].map(|i| v(i as i64));
                if let [Some(v1), Some(v2), Some(v3), Some(v4)] = ids {
                    mesh.add_box(poly_box(v1, v2, v3, v4), colour);
                }
            }
            PolygonKind::Other(_) => {}
        }
    }
    mesh
}

/// A box polygon's unit box transform, from the 4 corners it's stored as (no
/// two on the same edge).
pub fn poly_box(v1: Vec3, v2: Vec3, v3: Vec3, v4: Vec3) -> Mat4 {
    let x = (v1 + v2) - (v3 + v4);
    let y = (v1 + v3) - (v2 + v4);
    let z = (v1 + v4) - (v2 + v3);
    let t = (v1 + v2 + v3 + v4) * 0.25;
    let dx = x.normalize_or(Vec3::X);
    let dy = y.normalize_or(Vec3::Y);
    let dz = dx.cross(dy).normalize_or(Vec3::Z);
    Mat4::from_cols(
        (dx * x.length() * 0.5).extend(0.0),
        (dy * y.length() * 0.5).extend(0.0),
        (dz * z.length() * 0.5).extend(0.0),
        t.extend(1.0),
    )
}

/// The C#'s `PartShape` of a bound's part, which its selection shows.
fn shape_name(shape: &Shape, t: BoundsType) -> &'static str {
    match (shape, t) {
        (Shape::UnitBox, _) => "Box",
        (Shape::UnitSphere, _) => "Sphere",
        (_, BoundsType::Capsule) => "Capsule",
        (_, BoundsType::Cylinder | BoundsType::Disc) => "Cylinder",
        _ => "Mesh",
    }
}

fn info(b: &Bounds, index: Option<usize>, shape: &Shape, materials: &Materials) -> PartInfo {
    let t = b.bounds_type;
    let mut fields = vec![("Type".to_owned(), t.name().to_owned())];
    let mut field = |k: &str, v: String| fields.push((k.to_owned(), v));
    match &b.shape {
        BoundsShape::Geometry(g) | BoundsShape::GeometryBvh(g, _) => {
            let names: Vec<String> = g.materials.iter().flatten().map(|m| materials.name(m.data1.hash())).collect();
            field("Materials", names.join(", "));
            field("Vertices", g.vertices.as_ref().map_or(0, Vec::len).to_string());
            field("Polygons", g.polygons.len().to_string());
            field("CenterGeom", v3(g.center_geom));
        }
        _ => {
            field("Material", materials.name(b.material.hash()));
        }
    }
    field("BoxMin", v3(b.box_min));
    field("BoxMax", v3(b.box_max));
    field("BoxCenter", v3(b.box_center));
    field("SphereCenter", v3(b.sphere_center));
    field("SphereRadius", format!("{:.3}", b.sphere_radius));
    field("Margin", format!("{:.3}", b.margin));
    match &b.shape {
        BoundsShape::Capsule(c) => field("HalfHeight", format!("{:.3}", c.half_height)),
        BoundsShape::Cylinder(c) => field("HalfHeight", format!("{:.3}", c.half_height)),
        _ => {}
    }
    field("Mass", format!("{}", b.mass));
    if index.is_some() {
        let p = &b.transform[12..15];
        field("Position", v3([p[0], p[1], p[2]]));
        let f = b.composite_flags1;
        field("TypeFlags", flags_to_string(f.type_flags, &BOUND_FLAG_NAMES));
        field("IncludeFlags", flags_to_string(f.include_flags, &BOUND_FLAG_NAMES));
    }
    let name = match index {
        Some(i) => format!("{} {i}", t.name()),
        None => t.name().to_owned(),
    };
    PartInfo { type_name: shape_name(shape, t).to_owned(), name, fields, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use rf_formats::formats::ybn::{BoundComposite, BoundPolygon, BoundsShape, PolyBox, PolyTriangle};

    use super::*;

    fn bound(t: BoundsType) -> Bounds {
        Bounds::new(t)
    }

    #[test]
    fn composite_children_are_parts() {
        let mut sphere = bound(BoundsType::Sphere);
        sphere.sphere_center = [1.0, 0.0, 0.0];
        sphere.sphere_radius = 2.0;
        sphere.transform[12..15].copy_from_slice(&[10.0, 0.0, 0.0]);
        let mut cube = bound(BoundsType::Box);
        cube.box_min = [-1.0, -2.0, -3.0];
        cube.box_max = [1.0, 2.0, 3.0];
        let tapered = bound(BoundsType::TaperedCapsule);
        let mut geom = Box::<BoundGeometry>::default();
        geom.vertices = Some(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 1.0]]);
        geom.center_geom = [0.0, 0.0, 5.0];
        let tri = PolyTriangle { vert_index1: 0, vert_index2: 1, vert_index3: 2, ..Default::default() };
        let bx = PolyBox { vert_index1: 0, vert_index2: 1, vert_index3: 2, vert_index4: 3, ..Default::default() };
        geom.polygons = vec![
            BoundPolygon { kind: PolygonKind::Triangle(tri), material_index: 0 },
            BoundPolygon { kind: PolygonKind::Box(bx), material_index: 0 },
        ];
        let mut bvh = bound(BoundsType::GeometryBvh);
        bvh.shape = BoundsShape::GeometryBvh(geom, Default::default());
        let mut root = bound(BoundsType::Composite);
        root.shape = BoundsShape::Composite(BoundComposite {
            children: Some(vec![Some(sphere), None, Some(cube), Some(tapered), Some(bvh)]),
            ..Default::default()
        });

        let parts = parts(&root, &Materials::default());
        assert_eq!(parts.len(), 3);
        let s = &parts[0];
        assert!(matches!(s.shape, Shape::UnitSphere));
        assert_eq!(s.centre, Vec3::new(11.0, 0.0, 0.0));
        assert_eq!(s.radius, 2.0);
        assert_eq!(s.info.name, "Sphere 0");
        assert_eq!(s.colour, rf_game::materials::UNKNOWN_COLOUR);
        let b = &parts[1];
        assert!(matches!(b.shape, Shape::UnitBox));
        assert_eq!(b.transform.transform_point3(Vec3::splat(0.5)), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(b.info.name, "Box 2");
        let m = &parts[2];
        let Shape::Mesh(g) = &m.shape else { panic!("a mesh") };
        // The triangle, then the box's 12.
        assert_eq!(g.indices.len(), 3 + 36);
        assert_eq!(g.vertices[0].position, [0.0, 0.0, 5.0]);
        // No materials: white.
        assert_eq!(g.vertices[0].colour, WHITE);
        assert_eq!((m.info.type_name.as_str(), m.info.name.as_str()), ("Mesh", "GeometryBVH 4"));
    }

    #[test]
    fn child_matrices_are_affine() {
        let mut cube = bound(BoundsType::Box);
        cube.box_max = [1.0; 3];
        cube.box_min = [-1.0; 3];
        // As in some fences' bounds: values in M14, M24 and M34.
        cube.transform = [0.0, 1.0, 0.0, 4.0, 0.0, 0.0, 1.0, -3.0, 1.0, 0.0, 0.0, 0.0, 5.0, 0.0, 0.0, 1.0];
        let mut root = bound(BoundsType::Composite);
        root.shape = BoundsShape::Composite(BoundComposite { children: Some(vec![Some(cube)]), ..Default::default() });
        let parts = parts(&root, &Materials::default());
        let t = parts[0].transform;
        assert_eq!(t.row(3), glam::Vec4::W);
        assert_eq!(t * glam::Vec4::new(0.5, 0.5, 0.5, 1.0), glam::Vec4::new(6.0, 1.0, 1.0, 1.0));
    }

    #[test]
    fn box_polygons_from_corners() {
        // Alternate corners of a 2 x 4 x 6 box about (1, 1, 1).
        let c = Vec3::ONE;
        let corner = |x: f32, y: f32, z: f32| c + Vec3::new(x, y * 2.0, z * 3.0);
        let m =
            poly_box(corner(1.0, 1.0, 1.0), corner(1.0, -1.0, -1.0), corner(-1.0, 1.0, -1.0), corner(-1.0, -1.0, 1.0));
        let p = m.transform_point3(Vec3::splat(0.5));
        assert!((p - corner(1.0, 1.0, 1.0)).length() < 1e-5, "{p}");
        assert!((m.transform_point3(Vec3::ZERO) - c).length() < 1e-5);
    }
}
