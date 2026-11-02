//! Named places to jump to, found from the ymaps' names: a town's ymaps start
//! with its prefix (`val_` for Valentine, `new_` and a district for Saint
//! Denis, whose working name was New Bordeaux...).

use crate::math::Vec3;
use crate::nodes::Node;

/// Places and the ymap name prefixes that make them.
const PLACES: &[(&str, &[&str])] = &[
    ("Valentine", &["val_"]),
    ("Saint Denis", &["new_com_", "new_gar_", "new_frn_", "new_pie_", "new_slm_", "new_mrk_", "new_wrh_"]),
    ("Blackwater", &["bla_"]),
    ("Rhodes", &["rho_"]),
    ("Strawberry", &["str_"]),
    ("Annesburg", &["ann_"]),
    ("Van Horn", &["van_"]),
    ("Emerald Ranch", &["eme_"]),
    ("Lagras", &["lag_01_"]),
    ("Armadillo", &["arm_"]),
    ("Tumbleweed", &["tum_"]),
    ("Benedict Point", &["ben_"]),
    ("MacFarlane's Ranch", &["mfr_"]),
    ("Thieves Landing", &["the_"]),
    ("Colter", &["col_"]),
    ("Wapiti", &["wap_"]),
    ("Riggs Station", &["rig_01_"]),
    ("Wallace Station", &["wal_train_"]),
    ("Manzanita Post", &["man_01_"]),
    ("Butcher Creek", &["but_01_"]),
    ("Sisika Penitentiary", &["sis_01_"]),
    ("Guarma", &["gua_"]),
];

/// How high above the middle of its ymaps a place's camera is.
const CAMERA_HEIGHT: f32 = 30.0;

/// Ymaps whose streaming box is wider than this are LOD maps of a whole
/// region, not the place itself.
const MAX_SIZE: f32 = 2000.0;

/// The places whose ymaps are there: the median of the centres of their
/// streaming boxes, a little above.
pub(crate) fn places(nodes: &[Node]) -> Vec<(String, Vec3)> {
    let mut out = Vec::new();
    for (name, prefixes) in PLACES {
        let mut centres: Vec<Vec3> = nodes
            .iter()
            .filter(|n| prefixes.iter().any(|p| n.file_name.starts_with(p)))
            .filter(|n| n.has_streaming_box() && (0..2).all(|i| n.streaming.max[i] - n.streaming.min[i] < MAX_SIZE))
            .map(|n| n.streaming.centre())
            .collect();
        if centres.is_empty() {
            continue;
        }
        let mut p = [0, 1, 2].map(|i| median(&mut centres, i));
        p[2] += CAMERA_HEIGHT;
        out.push((name.to_string(), p));
    }
    out
}

fn median(points: &mut [Vec3], axis: usize) -> f32 {
    points.sort_by(|a, b| a[axis].total_cmp(&b[axis]));
    let n = points.len();
    if n % 2 == 1 { points[n / 2][axis] } else { (points[n / 2 - 1][axis] + points[n / 2][axis]) * 0.5 }
}

#[cfg(test)]
mod tests {
    use rf_formats::rpf8::FileRef;

    use super::*;
    use crate::math::Aabb;

    fn node(name: &str, min: Vec3, max: Vec3) -> Node {
        let c = rf_game::MapDataCacheEntry { streaming_min: min, streaming_max: max, ..Default::default() };
        let mut n = Node::cached(FileRef { archive: 0, entry: 0 }, name.into(), &c);
        n.streaming = Aabb::new(min, max);
        n
    }

    #[test]
    fn places_from_node_names() {
        let nodes = vec![
            node("val_01_", [0.0, 0.0, 100.0], [100.0, 100.0, 120.0]),
            node("val_02_", [100.0, 0.0, 100.0], [200.0, 100.0, 120.0]),
            node("val_03_", [200.0, 0.0, 100.0], [300.0, 100.0, 120.0]),
            // Too big, and not there.
            node("val_lod", [-5000.0, -5000.0, 0.0], [5000.0, 5000.0, 0.0]),
            node("val_vfx", [-1.0; 3], [-1.0; 3]),
            node("valley", [9000.0; 3], [9100.0; 3]),
            node("new_com_01", [10.0, 10.0, 0.0], [20.0, 20.0, 0.0]),
        ];
        let p = places(&nodes);
        assert_eq!(p[0], ("Valentine".to_string(), [150.0, 50.0, 110.0 + CAMERA_HEIGHT]));
        assert_eq!(p[1], ("Saint Denis".to_string(), [15.0, 15.0, CAMERA_HEIGHT]));
        assert_eq!(p.len(), 2);
    }
}
