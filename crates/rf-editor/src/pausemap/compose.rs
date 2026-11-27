//! Game tiles made again from the game's pixels and the marks: the map pane
//! and the export both go through here, so what's shown is what the game
//! gets.

use super::families::{Family, Layer};
use super::library::FamilyData;
use super::linework::Bounds;
use super::marks::Mark;

/// The tiles a box (atlas pixels) touches, as (row, column). Tiles overlap
/// by two pixels (tile c covers `[c stride, c stride + tile)`), so a mark on
/// a seam goes in both.
pub fn tiles_in(f: &Family, b: &Bounds) -> Vec<(u32, u32)> {
    if b.is_empty() {
        return Vec::new();
    }
    let st = f.stride() as f64;
    let range = |lo: f64, hi: f64, count: u32| {
        let a = ((lo - (f.tile as f64 - 1.0)) / st).ceil().max(0.0);
        let b = (hi / st).floor().min(count as f64 - 1.0);
        (a as i64, b as i64)
    };
    let (c0, c1) = range(b.x0, b.x1, f.cols);
    let (r0, r1) = range(b.y0, b.y1, f.rows);
    let mut out = Vec::new();
    for r in r0..=r1 {
        for c in c0..=c1 {
            out.push((r as u32, c as u32));
        }
    }
    out
}

/// Where the game ships an empty stub, the text families still stream a full
/// tile in its place; the line families don't, so marks there can't show.
pub fn can_fill_stub(f: &Family) -> bool {
    f.layer == Layer::Text
}

/// The game's tile, its marks erased, then every mark drawn; `None` for a
/// stub that can't be filled.
pub fn compose_tile<'a>(
    data: &FamilyData,
    row: u32,
    col: u32,
    marks: impl IntoIterator<Item = &'a Mark>,
) -> Option<Vec<u8>> {
    let f = data.family;
    let px = (f.tile * f.tile) as usize;
    let mut tile = match data.tile(row, col) {
        Some(t) => t.to_vec(),
        None if can_fill_stub(f) => vec![data.stub_value[(row * f.cols + col) as usize]; px],
        None => return None,
    };
    let st = f.stride() as f64;
    let (tx, ty) = (col as f64 * st, row as f64 * st);
    let marks: Vec<&Mark> = marks.into_iter().collect();
    for m in marks.iter().filter(|m| m.erases()) {
        m.stamp(&mut tile, f.tile as usize, tx, ty);
    }
    for m in marks.iter().filter(|m| !m.erases()) {
        m.stamp(&mut tile, f.tile as usize, tx, ty);
    }
    Some(tile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pausemap::families::{FAMILIES, fit_of};
    use crate::pausemap::marks::{Assets, mark_for};
    use crate::pausemap::path::from_points;
    use crate::pausemap::project::{Common, EraseObject, LineObject, LineStyle, MapObject};
    use rf_formats::formats::ytd::TextureDictionary;

    /// One full line tile at 0_0, ink everywhere (40), next to a stub.
    fn data() -> FamilyData {
        let f = &FAMILIES[2];
        let mut index = vec![0u16; f.cells()];
        index[0] = 1;
        FamilyData {
            family: f,
            tiles: vec![40; (f.tile * f.tile) as usize],
            index,
            stub_value: vec![255; f.cells()],
            template: TextureDictionary::default(),
        }
    }

    fn map_point(px: f64, py: f64) -> [f64; 2] {
        let fit = fit_of(&FAMILIES[2]);
        [px / fit.sx - fit.ox, py / fit.sy - fit.oy]
    }

    #[test]
    fn erasers_take_the_games_marks_never_whats_drawn() {
        let (data, assets) = (data(), Assets::default());
        let f = data.family;
        let erase = MapObject::Erase(EraseObject {
            common: Common { id: "e".into(), ..Default::default() },
            target: Layer::Lines,
            radius: 40.0,
            points: vec![map_point(128.0, 128.0)],
        });
        let road = MapObject::Line(LineObject {
            common: Common { id: "r".into(), ..Default::default() },
            style: LineStyle::Road,
            width: 2.0,
            spacing: 0.0,
            closed: false,
            nodes: from_points(&[map_point(100.0, 128.0), map_point(160.0, 128.0)], false),
        });
        for order in [[&road, &erase], [&erase, &road]] {
            let marks: Vec<Mark> = order.iter().map(|o| mark_for(f, o, &assets).unwrap()).collect();
            let tile = compose_tile(&data, 0, 0, &marks).unwrap();
            let t = f.tile as usize;
            assert!(tile[128 * t + 128] < 64);
            assert_eq!(tile[78 * t + 128], 255);
            assert_eq!(tile[10 * t + 10], 40);
        }
    }

    #[test]
    fn line_stubs_stay_stubs() {
        assert!(compose_tile(&data(), 0, 1, &[]).is_none());
    }
}
