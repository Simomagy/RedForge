//! Properties of the navmesh polygons selected in "Navmesh" mode: their flags,
//! audio and water depth, blocking them off, and growing the selection.

use eframe::egui::{self, DragValue, RichText};
use rf_editor::{Editor, PolyRef};
use rf_formats::formats::ynv::{NavmeshPolygon, POLY_FLAG_NAMES, poly_flags};

use crate::map::helpers::navmesh::{group, polygon_key};
use crate::map::helpers::{HelperKey, Mode};
use crate::map::tools::Tools;

/// The flags by what they're about, by their names in the XML. The rest are
/// unknown.
const GROUPS: [(&str, &[&str]); 4] = [
    (
        "Surface",
        &["Pavement", "Road", "Boardwalk", "Muddy", "Vegetation", "Water", "Stairs", "SteepSlope", "Door", "Interior"],
    ),
    (
        "Peds",
        &[
            "SwitchedOffForPeds",
            "Isolated",
            "DangerKeepAway1",
            "DangerKeepAway2",
            "NetworkSpawnCandidate",
            "NearCarNode",
        ],
    ),
    ("Shelter", &["Sheltered", "Sheltered2", "Sheltered3", "Shelter4"]),
    // Set by the game's tools from the polygons' shapes: shown, not edited.
    ("Layout", &["Small", "Large", "ZeroAreaStitchPolyDLC", "LiesAlongEdgeOfMesh"]),
];

/// What a flag is for, where that's known.
fn tip(name: &str) -> &'static str {
    match name {
        "SwitchedOffForPeds" => {
            "Ambient peds keep off it. Paths that scripts ask for still go through: to keep everyone \
             out, use Block"
        }
        "Isolated" => "Not joined to the navmesh around it",
        "Pavement" => "Walkways: ambient peds keep to them in towns",
        "Road" => "Roads and streets",
        "Boardwalk" => "Wooden walkways in front of buildings",
        "Interior" => "Inside a building",
        "Door" => "A doorway",
        "Stairs" => "Steps and stairs",
        "SteepSlope" => "Too steep to walk on comfortably",
        "Water" => "Under water: the water depth says how deep",
        "NetworkSpawnCandidate" => "Players can spawn on it in multiplayer",
        "DangerKeepAway1" | "DangerKeepAway2" => "Peds avoid it (fire, danger)",
        "Small" | "Large" => "The polygon's size class, set by the game's tools",
        "ZeroAreaStitchPolyDLC" => "A two-corner polygon along the cell's edge, joining it to the next cell",
        "LiesAlongEdgeOfMesh" => "Along the edge of the navmesh",
        _ => "",
    }
}

/// "SwitchedOffForPeds" as "Switched off for peds".
pub fn pretty(name: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        let prev_lower = i > 0 && chars[i - 1].is_lowercase();
        let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
        let word = i > 0 && ((c.is_uppercase() && (prev_lower || next_lower)) || (c.is_ascii_digit() && !prev_digit));
        if word {
            out.push(' ');
        }
        // An acronym (DLC) stays as it is.
        let acronym = c.is_uppercase() && !next_lower && chars.get(i + 1).is_some_and(|n| n.is_uppercase());
        let acronym = acronym || (c.is_uppercase() && !next_lower && i > 0 && chars[i - 1].is_uppercase());
        out.push(if i > 0 && !acronym { c.to_ascii_lowercase() } else { c });
    }
    out
}

fn bit(name: &str) -> Option<u64> {
    POLY_FLAG_NAMES.iter().find(|(_, n)| *n == name).map(|(b, _)| *b)
}

/// The selected navmesh polygons.
pub fn selected(tools: &Tools) -> Vec<PolyRef> {
    if tools.mode != Mode::Navmesh {
        return Vec::new();
    }
    tools
        .helper_selection
        .iter()
        .filter(|k| k.group == group::POLYGON)
        .map(|k| PolyRef { navmesh: k.source, index: k.index })
        .collect()
}

fn keys(polys: &[PolyRef]) -> Vec<HelperKey> {
    polys.iter().map(|p| polygon_key(p.navmesh, p.index)).collect()
}

/// What's drawn over the navmeshes, and "colour by flag": shown with or
/// without polygons selected.
pub fn view_ui(ui: &mut egui::Ui, tools: &mut Tools) {
    use crate::map::helpers::navmesh::overlay;
    ui.horizontal_wrapped(|ui| {
        ui.label("Show");
        let shown = [
            (overlay::EDGES, "Edges", "Every polygon's outline (white)"),
            (
                overlay::OPEN_EDGES,
                "Open edges",
                "Edges with nothing across, red: walls, the navmesh's edge, blocked areas. In cyan the \
                 two-corner polygons that join the next cell",
            ),
            (overlay::NEIGHBOURS, "Neighbours", "Lines from the selected polygons to those across their edges (green)"),
            (overlay::LINKS, "Links", "Special links: ladders, jumps, climbs and drops between polygons (yellow)"),
        ];
        for (bit, name, tip) in shown {
            let mut on = tools.navmesh_overlays & bit != 0;
            if ui.checkbox(&mut on, name).on_hover_text(tip).changed() {
                tools.navmesh_overlays ^= bit;
            }
        }
    });
    highlight_ui(ui, tools);
}

/// The "colour by flag" choice.
fn highlight_ui(ui: &mut egui::Ui, tools: &mut Tools) {
    ui.horizontal(|ui| {
        ui.label("Colour by flag");
        let shown = match tools.navmesh_highlight {
            Some(b) => POLY_FLAG_NAMES.iter().find(|(v, _)| *v == b).map_or("?".into(), |(_, n)| pretty(n)),
            None => "None".into(),
        };
        egui::ComboBox::from_id_salt("navmesh-highlight").selected_text(shown).show_ui(ui, |ui| {
            ui.selectable_value(&mut tools.navmesh_highlight, None, "None");
            for (_, names) in GROUPS {
                for n in names {
                    if let Some(b) = bit(n) {
                        ui.selectable_value(&mut tools.navmesh_highlight, Some(b), pretty(n));
                    }
                }
            }
        });
    })
    .response
    .on_hover_text("Polygons with the flag in yellow, the others dark");
}

/// "Make from collisions": the navmesh of the box around the selected
/// polygons made again, and its settings.
fn make_ui(ui: &mut egui::Ui, editor: &mut Editor, tools: &mut Tools, polys: &[PolyRef], error: &mut Option<String>) {
    use rf_editor::navgen::stitch::Region;
    ui.add_space(4.0);
    let tip = "Make the navmesh of the box around the selected polygons (2 m wider) again, from the collisions \
               there: the ground, the buildings, the interiors and the models of the objects around (doors \
               and gates left out). The polygons around are cut to fit and joined to it, special links follow, \
               and the flags the collision doesn't say (pavement, road...) come from the polygons that were \
               there. The box may cross navmesh cells: each gets its part. It's shown first, to apply or drop. \
               For a custom interior or a new building: select the ground around it first.";
    ui.horizontal(|ui| {
        if ui.button("Make from collisions").on_hover_text(tip).clicked() {
            let corners: Vec<glam::Vec3> =
                polys.iter().filter_map(|p| editor.navmesh_polygon_corners(*p)).flatten().collect();
            if corners.is_empty() {
                return;
            }
            let (lo, hi) = corners.iter().fold((glam::Vec3::MAX, glam::Vec3::MIN), |(l, h), c| (l.min(*c), h.max(*c)));
            let region = Region {
                min: lo.truncate() - glam::Vec2::splat(2.0),
                max: hi.truncate() + glam::Vec2::splat(2.0),
                z_min: lo.z - 3.0,
                z_max: hi.z + 12.0,
            };
            match editor.make_navmesh(region, &tools.navgen) {
                Ok(made) => {
                    *error = None;
                    // The polygons shown are the preview's, numbered anew.
                    let names: Vec<u32> = made.names().collect();
                    tools.helper_selection.retain(|k| !names.contains(&k.source));
                    tools.navmesh_preview = Some(made);
                }
                Err(e) => *error = Some(e),
            }
        }
    });
    egui::CollapsingHeader::new("Making settings").id_salt("navgen-settings").show(ui, |ui| {
        let s = &mut tools.navgen;
        egui::Grid::new("navgen-grid").num_columns(2).show(ui, |ui| {
            let row = |ui: &mut egui::Ui,
                       label: &str,
                       tip: &str,
                       v: &mut f32,
                       range: std::ops::RangeInclusive<f32>,
                       unit: &str| {
                ui.label(label).on_hover_text(tip);
                ui.add(DragValue::new(v).range(range).speed(0.01).suffix(unit).max_decimals(2));
                ui.end_row();
            };
            row(
                ui,
                "Distance from walls",
                "How far from walls the navmesh stays",
                &mut s.agent_radius,
                0.1..=1.5,
                " m",
            );
            row(ui, "Headroom", "The height a ped needs over it", &mut s.agent_height, 0.5..=4.0, " m");
            row(ui, "Highest step", "The highest step peds walk up", &mut s.max_climb, 0.0..=2.0, " m");
            row(ui, "Steepest slope", "The steepest ground peds walk on", &mut s.max_slope, 10.0..=80.0, "°");
            row(ui, "Detail", "The voxels' size: smaller is finer and slower", &mut s.cell_size, 0.05..=0.5, " m");
        });
        if ui
            .small_button("As the game")
            .on_hover_text("The settings that came closest to the game's navmeshes (#6)")
            .clicked()
        {
            *s = Default::default();
        }
    });
}

/// Navmesh made from collisions and shown, not applied yet: what it does, and
/// Apply or Cancel.
pub fn preview_ui(ui: &mut egui::Ui, editor: &mut Editor, tools: &mut Tools, error: &mut Option<String>) {
    let Some(made) = &tools.navmesh_preview else { return };
    let g = &made.generated;
    let s = &g.stitched;
    ui.label(RichText::new("Navmesh made from collisions").strong());
    ui.label(RichText::new("Shown in place of the navmesh there: apply it or drop it.").weak());
    egui::Grid::new("navgen-preview").num_columns(2).show(ui, |ui| {
        let mut row = |k: &str, v: String| {
            ui.label(k);
            ui.label(v);
            ui.end_row();
        };
        row("Navmeshes", g.navmeshes.join(", "));
        row(
            "Box",
            format!(
                "{:.1}, {:.1} to {:.1}, {:.1}",
                made.region.min.x, made.region.min.y, made.region.max.x, made.region.max.y
            ),
        );
        row("Collision", format!("{} triangles", g.triangles));
        row("Polygons", format!("{} made, {} removed, {} cut to fit", s.added, s.removed, s.pieces));
        if s.borders > 0 {
            row("Cell borders", format!("{} border polygons made again", s.borders));
        }
        row("Special links", format!("{} moved", s.links_moved + g.neighbour_links));
    });
    let lost = s.links_dropped + g.neighbour_links_lost;
    if lost > 0 {
        ui.label(
            RichText::new(format!("{lost} special links found no polygon at their end any more"))
                .color(egui::Color32::from_rgb(255, 160, 40)),
        );
    }
    if !g.cells_without_navmesh.is_empty() {
        ui.label(
            RichText::new(format!("No navmesh in {}: nothing made there", g.cells_without_navmesh.join(", ")))
                .color(egui::Color32::from_rgb(255, 160, 40)),
        );
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Apply").on_hover_text("Put it in the navmeshes (one undo step)").clicked() {
            let made = tools.navmesh_preview.take().expect("shown");
            let region = made.region;
            match editor.apply_made_navmesh(made) {
                Ok(g) => {
                    *error = None;
                    tools.say(format!("{}: {} polygons made", g.navmeshes.join(", "), g.stitched.added));
                    // The polygons made, selected, to make again or edit.
                    let mut found = Vec::new();
                    for name in &g.navmeshes {
                        let n = rf_formats::hash::joaat(name);
                        for index in 0..editor.navmesh_polygon_count(n) {
                            let r = PolyRef { navmesh: n, index: index as u32 };
                            let Some(c) = editor.navmesh_polygon_corners(r) else { continue };
                            if c.len() < 3 {
                                continue;
                            }
                            let m = c.iter().sum::<glam::Vec3>() / c.len() as f32;
                            if m.x > region.min.x && m.x < region.max.x && m.y > region.min.y && m.y < region.max.y {
                                found.push(r);
                            }
                        }
                    }
                    tools.helper_selection = keys(&found);
                }
                Err(e) => *error = Some(e),
            }
        }
        if ui.button("Cancel").on_hover_text("Drop it: the navmesh stays as it was").clicked() {
            let made = tools.navmesh_preview.take().expect("shown");
            let names: Vec<u32> = made.names().collect();
            tools.helper_selection.retain(|k| !names.contains(&k.source));
            *error = None;
        }
    });
    if let Some(e) = error {
        ui.colored_label(egui::Color32::from_rgb(255, 120, 100), e.as_str());
    }
}

/// The selected polygons' properties. Returns false when none are selected.
pub fn ui(ui: &mut egui::Ui, editor: &mut Editor, tools: &mut Tools, error: &mut Option<String>) -> bool {
    let polys = selected(tools);
    if polys.is_empty() {
        return false;
    }
    let mut navmeshes: Vec<u32> = polys.iter().map(|p| p.navmesh).collect();
    navmeshes.sort_unstable();
    navmeshes.dedup();
    for n in &navmeshes {
        if let Err(e) = editor.open_navmesh(*n) {
            *error = Some(e);
            return true;
        }
    }
    let current: Vec<NavmeshPolygon> = polys.iter().filter_map(|r| editor.navmesh_polygon(*r)).collect();
    let names: Vec<String> = navmeshes.iter().filter_map(|n| editor.navmesh(*n)).map(|d| d.file_name.clone()).collect();

    ui.horizontal(|ui| {
        let what =
            if polys.len() == 1 { format!("Polygon {}", polys[0].index) } else { format!("{} polygons", polys.len()) };
        ui.label(RichText::new(what).strong());
        ui.label("in");
        ui.label(RichText::new(names.join(", ")).strong());
    });
    if navmeshes.iter().any(|n| editor.navmesh(*n).is_some_and(|d| d.is_edited())) {
        ui.label(RichText::new("navmesh edited").color(egui::Color32::from_rgb(255, 160, 40)));
    }

    // Blocking.
    let all = |flag: u64| current.iter().all(|p| p.flags() & flag != 0);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let block = ui.button("Block").on_hover_text(
            "Cut every edge and special link (climb, drop, jump) between these polygons and the rest, and \
             switch them off for peds: no path goes in or out (peds, horses, scripted walks). For the \
             ground under a new building.",
        );
        if block.clicked() {
            match editor.block_polygons(&polys) {
                Ok(b) if b == Default::default() && all(poly_flags::SWITCHED_OFF_FOR_PEDS) => {
                    *error = Some("Already blocked".into())
                }
                Ok(_) => *error = None,
                Err(e) => *error = Some(e),
            }
        }
        let can = navmeshes.iter().all(|n| editor.navmesh(*n).is_some_and(|d| d.can_unblock()));
        let unblock = ui
            .add_enabled(can, egui::Button::new("Unblock"))
            .on_hover_text("Put back the game's edges and special links around these polygons, and their flag")
            .on_disabled_hover_text("The game has no navmesh laid out as this one to take the edges from");
        if unblock.clicked() {
            match editor.unblock_polygons(&polys) {
                Ok(_) => *error = None,
                Err(e) => *error = Some(e),
            }
        }
    });

    // The selection.
    ui.horizontal(|ui| {
        if ui.button("Grow").on_hover_text("Add the polygons around the selection").clicked() {
            let more = editor.navmesh_neighbours(&polys);
            tools.helper_selection.extend(keys(&more));
        }
        if ui.button("Clear").on_hover_text("Select nothing").clicked() {
            tools.helper_selection.clear();
        }
        let highlight = tools.navmesh_highlight;
        let label = "Select all with the flag";
        let r = ui
            .add_enabled(highlight.is_some(), egui::Button::new(label))
            .on_hover_text("Every polygon of these navmeshes with the flag \"Colour by flag\" shows")
            .on_disabled_hover_text("Choose a flag in \"Colour by flag\" first");
        if r.clicked()
            && let Some(flag) = highlight
        {
            let found: Vec<PolyRef> = navmeshes.iter().flat_map(|n| editor.navmesh_polygons_with(*n, flag)).collect();
            tools.helper_selection = keys(&found);
        }
    });
    view_ui(ui, tools);
    make_ui(ui, editor, tools, &polys, error);

    // The flags.
    let mut set = 0u64;
    let mut clear = 0u64;
    for (title, names) in GROUPS {
        let editable = title != "Layout";
        ui.add_space(4.0);
        ui.label(RichText::new(title).weak());
        egui::Grid::new(("navmesh-flags", title)).num_columns(2).show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                let Some(b) = bit(name) else { continue };
                let on = current.iter().filter(|p| p.flags() & b != 0).count();
                let mut value = on == current.len() && on > 0;
                let check = egui::Checkbox::new(&mut value, pretty(name)).indeterminate(on > 0 && on < current.len());
                let r = ui.add_enabled(editable, check);
                let r = if tip(name).is_empty() { r } else { r.on_hover_text(tip(name)) };
                if r.changed() {
                    if value {
                        set |= b;
                    } else {
                        clear |= b;
                    }
                }
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
    }
    egui::CollapsingHeader::new("Unknown flags").id_salt("navmesh-unknown").show(ui, |ui| {
        let known: Vec<&str> = GROUPS.iter().flat_map(|(_, n)| n.iter().copied()).collect();
        egui::Grid::new("navmesh-unknown-flags").num_columns(2).show(ui, |ui| {
            let mut col = 0;
            for (b, name) in POLY_FLAG_NAMES.iter().filter(|(_, n)| !known.contains(n)) {
                let on = current.iter().filter(|p| p.flags() & b != 0).count();
                let mut value = on == current.len() && on > 0;
                let check = egui::Checkbox::new(&mut value, *name).indeterminate(on > 0 && on < current.len());
                if ui.add(check).on_hover_text("What it does isn't known").changed() {
                    if value {
                        set |= b;
                    } else {
                        clear |= b;
                    }
                }
                col += 1;
                if col % 2 == 0 {
                    ui.end_row();
                }
            }
        });
    });
    if (set | clear) != 0
        && let Err(e) = editor.set_polygon_flags(&polys, set, clear)
    {
        *error = Some(e);
    }

    // Audio and water depth.
    ui.add_space(4.0);
    let value_row = |ui: &mut egui::Ui, label: &str, values: Vec<u8>, max: u8, tip: &str| -> Option<u8> {
        let mut v = values.first().copied().unwrap_or(0);
        let mixed = values.iter().any(|x| *x != v);
        let mut out = None;
        ui.horizontal(|ui| {
            ui.label(label).on_hover_text(tip);
            let r = ui.add(DragValue::new(&mut v).range(0..=max).speed(0.05));
            if mixed {
                ui.label(RichText::new("mixed").weak());
            }
            if r.changed() {
                out = Some(v);
            }
        });
        out
    };
    let audio = value_row(
        ui,
        "Audio",
        current.iter().map(|p| p.audio_properties()).collect(),
        15,
        "The sound of footsteps on it (0-15)",
    );
    let water = value_row(
        ui,
        "Water depth",
        current.iter().map(|p| p.water_depth()).collect(),
        7,
        "How deep the water is, for polygons with the Water flag (0-7)",
    );
    if (audio.is_some() || water.is_some())
        && let Err(e) = editor.set_polygon_values(&polys, audio, water)
    {
        *error = Some(e);
    }

    ui.add_space(6.0);
    ui.label(
        RichText::new(
            "Click a polygon to select it, Ctrl+click to add or take one, Shift+click to add. Ctrl+drag \
             adds the polygons in a box, Shift+drag paints polygons in, Shift+Ctrl+drag out. A changed \
             navmesh goes in the project, and replaces the game's when exported (RedM loads it when \
             you connect).",
        )
        .weak(),
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(pretty("SwitchedOffForPeds"), "Switched off for peds");
        assert_eq!(pretty("ZeroAreaStitchPolyDLC"), "Zero area stitch poly DLC");
        assert_eq!(pretty("DangerKeepAway1"), "Danger keep away 1");
        assert_eq!(pretty("Shelter4"), "Shelter 4");
        assert_eq!(pretty("Road"), "Road");
    }

    #[test]
    fn every_grouped_flag_is_a_flag() {
        for (_, names) in GROUPS {
            for n in names {
                assert!(bit(n).is_some(), "{n}");
            }
        }
    }
}
