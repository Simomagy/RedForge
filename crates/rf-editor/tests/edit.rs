//! Editing a real RDR2 install's map. These only run when `REDFORGE_GAME_DIR`
//! points at the game folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec3;
use rf_editor::{Editor, EntityId, Placement, lod};
use rf_formats::formats::meta::Meta;
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_game::GameData;

fn editor() -> Option<Editor> {
    let dir = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    let fs = Rpf8FileSystem::open(&dir, Rpf8Keys::from_game_folder(&dir).unwrap(), &strings).unwrap();
    let gd = Arc::new(GameData::load(&fs, &strings));
    Some(Editor::new(Arc::new(fs), Arc::new(strings), gd))
}

fn settle(ed: &mut Editor, p: [f32; 3]) {
    let start = Instant::now();
    loop {
        ed.world.update(p, 12.0);
        if ed.world.stats().nodes_loading == 0 {
            ed.world.update(p, 12.0);
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "still loading");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn position(ed: &Editor, id: EntityId) -> Vec3 {
    let t = ed.world.entity(id).unwrap().transform;
    Vec3::new(t[12], t[13], t[14])
}

#[test]
fn move_delete_place_and_undo() {
    let Some(mut ed) = editor() else { return };
    let val = ed.world.places().into_iter().find(|(n, _)| n == "Valentine").unwrap().1;
    settle(&mut ed, val);

    // An HD entity whose LOD parent is in the parent map.
    let ids: Vec<EntityId> = ed.world.visible().iter().map(|e| e.id).collect();
    let id = ids
        .into_iter()
        .find(|id| {
            id.interior_entity == u32::MAX && {
                ed.select(vec![*id]);
                ed.entity_def(*id).and_then(lod::lod_parent).is_some_and(|(_, other)| other)
                    && ed.entity_def(*id).unwrap().u32("numChildren") == 0
            }
        })
        .expect("an HD entity with its LOD in the parent map");
    let map = id.node;
    let archetype = ed.world.entity(id).unwrap().archetype;
    let parent_map = ed.world.parent_map(map).unwrap();

    // Move it 10 m east: the world shows it there, the map is edited.
    let start = ed.placement(id).unwrap();
    let before = position(&ed, id);
    let mut moved = start;
    moved.position += Vec3::X * 10.0;
    ed.set_placements("Move", &[(id, moved)]);
    assert!(ed.map(map).unwrap().is_edited() && ed.map(map).unwrap().is_dirty());
    assert!(ed.world.map_override(map).is_some());
    assert!((position(&ed, id) - before - Vec3::X * 10.0).length() < 1e-3);
    assert!(ed.undo().is_some());
    assert!(!ed.map(map).unwrap().is_edited() && ed.world.map_override(map).is_none());
    assert!((position(&ed, id) - before).length() < 1e-3);
    assert!(ed.redo().is_some());
    assert!((position(&ed, id) - before - Vec3::X * 10.0).length() < 1e-3);
    assert!(ed.undo().is_some());

    // Delete it: its LOD parent in the parent map counts one child fewer.
    let (lod_index, _) = lod::lod_parent(ed.entity_def(id).unwrap()).unwrap();
    let count = lod::entities(&ed.map(map).unwrap().root).len();
    ed.select(vec![EntityId { node: parent_map, entity: lod_index, interior_entity: u32::MAX }]);
    let children = ed.entity_def(ed.selection()[0]).unwrap().u32("numChildren");
    ed.select(vec![id]);
    ed.delete_selected().unwrap();
    assert!(ed.selection().is_empty());
    assert_eq!(lod::entities(&ed.map(map).unwrap().root).len(), count - 1);
    let lod_def = |ed: &Editor| {
        lod::entities(&ed.map(parent_map).unwrap().root)[lod_index as usize].as_ref().unwrap().u32("numChildren")
    };
    assert_eq!(lod_def(&ed), children - 1);
    assert!(ed.map(parent_map).unwrap().is_edited());
    // The LOD parent itself can't go while it has children.
    ed.select(vec![EntityId { node: parent_map, entity: lod_index, interior_entity: u32::MAX }]);
    if children > 1 {
        assert!(ed.delete_selected().is_err());
    }
    assert_eq!(ed.undo().as_deref(), Some("Delete"));
    assert_eq!(lod_def(&ed), children);
    assert!(ed.edited_maps().is_empty(), "undo leaves nothing edited");

    // A new ymap, an entity placed in it, saved and read back.
    let name = ed.new_map("rf_editor_test").unwrap();
    assert_eq!(ed.target, Some(name));
    let placed = Placement { position: Vec3::from(val), ..Default::default() };
    let new_id = ed.place(archetype, placed).unwrap();
    assert_eq!(ed.selection(), [new_id]);
    settle(&mut ed, val);
    assert!(ed.world.visible().iter().any(|e| e.id == new_id), "the placed entity shows");
    let dir = std::env::temp_dir().join(format!("rf-editor-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rf_editor_test.ymap");
    ed.save_map(name, &path).unwrap();
    assert!(!ed.map(name).unwrap().is_dirty());
    let back = Meta::read(&std::fs::read(&path).unwrap()).unwrap();
    let root = back.root.unwrap();
    assert_eq!(lod::entities(&root).len(), 1);
    assert_eq!(lod::entities(&root)[0].as_ref().unwrap().hash("archetypeName"), archetype);
    // Saving worked out its extents: they hold the placed entity.
    let inside = |min: &str, max: &str| {
        let (lo, hi) = (Vec3::from(root.vec3(min)), Vec3::from(root.vec3(max)));
        lo.cmple(placed.position).all() && hi.cmpge(placed.position).all() && lo != hi
    };
    assert!(inside("entitiesExtentsMin", "entitiesExtentsMax"));
    assert!(inside("streamingExtentsMin", "streamingExtentsMax"));
    assert_ne!(root.u32("contentFlags"), 0);
    // The manifest names the ymap and the ytyp its archetype comes from.
    let ymf = dir.join("_manifest.ymf");
    ed.write_manifest(&ymf).unwrap();
    let cx = rf_formats::formats::XmlContext { hashes: ed.hashes(), folder: None };
    let xml = rf_formats::formats::to_xml("_manifest.ymf", &std::fs::read(&ymf).unwrap(), &cx).unwrap();
    assert!(xml.contains("<imapName>rf_editor_test</imapName>"), "{xml}");
    assert!(xml.matches("<Item>").count() >= 2, "{xml}");
    std::fs::remove_dir_all(&dir).unwrap();

    // Undoing everything removes the new ymap.
    while ed.undo().is_some() {}
    assert!(ed.map(name).is_none() && ed.target.is_none());
}

#[test]
fn interior_entities() {
    let Some(mut ed) = editor() else { return };
    let val = ed.world.places().into_iter().find(|(n, _)| n == "Valentine").unwrap().1;
    settle(&mut ed, val);

    // A plain interior entity (not in an entity set) that's shown.
    let ids: Vec<EntityId> =
        ed.world.visible().iter().map(|e| e.id).filter(|id| id.interior_entity != u32::MAX).collect();
    assert!(!ids.is_empty(), "interiors show from the town centre");
    let id = ids
        .into_iter()
        .find(|id| {
            ed.select(vec![*id]);
            ed.is_editable(*id)
        })
        .expect("an editable interior entity");
    let before = position(&ed, id);
    let p = ed.placement(id).unwrap();
    assert!((p.position - before).length() < 1e-3, "the editor places it where the world does");

    // Moving it edits the interior's ytyp, and the world follows.
    let mut moved = p;
    moved.position += Vec3::X;
    ed.set_placements("Move", &[(id, moved)]);
    assert_eq!(ed.edited_types().len(), 1);
    assert!((position(&ed, id) - before - Vec3::X).length() < 1e-3);
    assert!((ed.placement(id).unwrap().position - moved.position).length() < 1e-3);
    ed.undo();
    assert!(ed.edited_types().is_empty());
    assert!((position(&ed, id) - before).length() < 1e-3);

    // A copy lands next to it in the interior, selected.
    ed.select(vec![id]);
    ed.duplicate_selected().unwrap();
    let copy = ed.selection()[0];
    assert_ne!(copy, id);
    assert_eq!((copy.node, copy.entity), (id.node, id.entity));
    ed.world.update(val, 12.0);
    assert!((position(&ed, copy) - before).length() < 1e-3, "the copy is where the original is");
    ed.undo();

    // Deleting keeps the rooms' and portals' entity indices in range.
    ed.select(vec![id]);
    ed.delete_selected().unwrap();
    let ytyp = ed.edited_types()[0].root.clone();
    for mlo in rf_editor::interior::archetypes(&ytyp).iter().flatten() {
        let n = lod::entities(mlo).len() as u32;
        for holder in ["rooms", "portals"] {
            for b in mlo.bags(holder).unwrap_or_default().iter().flatten() {
                if let Some(rf_formats::formats::databag::Value::U32s(v)) = b.object("attachedObjects") {
                    assert!(v.iter().all(|&i| i < n), "{holder}: {v:?} with {n} entities");
                }
            }
        }
    }
    ed.undo();
    assert!(ed.edited_types().is_empty());
}

#[test]
fn project_round_trip() {
    let Some(mut ed) = editor() else { return };
    let val = ed.world.places().into_iter().find(|(n, _)| n == "Valentine").unwrap().1;
    settle(&mut ed, val);
    let ids: Vec<EntityId> = ed.world.visible().iter().map(|e| e.id).collect();
    let id = ids
        .into_iter()
        .find(|id| {
            id.interior_entity == u32::MAX && {
                ed.select(vec![*id]);
                ed.is_editable(*id)
            }
        })
        .unwrap();
    let before = position(&ed, id);

    // Editing a game ymap puts it in a (new) project; saving puts it in the project's folder.
    let mut p = ed.placement(id).unwrap();
    p.position += Vec3::Z * 5.0;
    ed.set_placements("Move", &[(id, p)]);
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("test.rfproj");
    ed.new_project(&proj).unwrap();
    assert!(ed.has_unsaved());
    assert!(ed.save_all().unwrap().is_empty());
    assert!(!ed.has_unsaved());
    let file = ed.world.map_file_name(id.node).unwrap().to_owned();
    assert!(dir.path().join(format!("{file}.ymap")).exists());

    // Closing goes back to the game's ymap; opening brings the edit back.
    ed.close_project();
    assert!(ed.world.map_override(id.node).is_none());
    settle(&mut ed, val);
    assert!((position(&ed, id) - before).length() < 1e-3);
    let problems = ed.open_project(&proj).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    assert!((position(&ed, id) - before - Vec3::Z * 5.0).length() < 1e-3);
    let entries = ed.project_entries();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].path.is_some() && !entries[0].dirty);
}

#[test]
fn custom_props_from_a_project() {
    let Some(mut ed) = editor() else { return };
    let val = ed.world.places().into_iter().find(|(n, _)| n == "Valentine").unwrap().1;
    settle(&mut ed, val);
    // A drawable archetype of the game to copy.
    let ids: Vec<EntityId> = ed.world.visible().iter().map(|e| e.id).collect();
    let (source, model) = ids
        .iter()
        .filter_map(|id| ed.world.entity(*id).map(|e| e.archetype))
        .find_map(|a| {
            let m = rf_world::model_files(ed.data(), a)?;
            (m.kind == rf_world::ModelKind::Ydr).then_some((a, m))
        })
        .expect("a ydr archetype");

    // A project with a new ytyp (one archetype, `rf_test_prop`) and its ydr.
    let dir = tempfile::tempdir().unwrap();
    let name = rf_formats::hash::joaat("rf_test_prop");
    let fs = rf_formats::rpf8::Rpf8FileSystem::open(
        Path::new(&std::env::var("REDFORGE_GAME_DIR").unwrap()),
        rf_formats::rpf8::Rpf8Keys::from_game_folder(Path::new(&std::env::var("REDFORGE_GAME_DIR").unwrap())).unwrap(),
        ed.hashes(),
    )
    .unwrap();
    let mut arch = ed.data().archetype_bag(&fs, ed.hashes(), source).unwrap();
    arch.set_hash("name", name);
    arch.set_hash("assetName", name);
    let mut types = Meta::create("CMapTypes").unwrap();
    types.set_hash("name", rf_formats::hash::joaat("rf_test_types"));
    types.set_object("archetypes", Some(rf_formats::formats::databag::Value::Bags(vec![Some(arch)])));
    std::fs::write(dir.path().join("rf_test_types.ytyp"), Meta::new(Some(types)).save().unwrap()).unwrap();
    std::fs::write(dir.path().join("rf_test_prop.ydr"), fs.extract_loose(model.file).unwrap()).unwrap();

    ed.new_project(&dir.path().join("p.rfproj")).unwrap();
    let problems = ed.add_project_folder(dir.path());
    assert!(problems.is_empty(), "{problems:?}");
    assert!(ed.take_data_change().is_some());
    let a = ed.data().archetypes.get(&name).expect("the project's archetype");
    assert_eq!(a.asset_name, name);
    let files = rf_world::model_files(ed.data(), name).expect("its model");
    assert_eq!(files.file.archive, rf_formats::rpf8::LOOSE, "the model is the project's file");

    // Placed in a new ymap, it shows.
    ed.new_map("rf_test_map").unwrap();
    let id = ed.place(name, Placement { position: Vec3::from(val), ..Default::default() }).unwrap();
    settle(&mut ed, val);
    assert!(ed.world.visible().iter().any(|e| e.id == id && e.archetype == name));

    // Out of the project, the archetype goes.
    ed.close_project();
    assert!(!ed.data().archetypes.contains_key(&name));
}

#[test]
fn batch_instances() {
    let Some(mut ed) = editor() else { return };
    let val = ed.world.places().into_iter().find(|(n, _)| n == "Valentine").unwrap().1;
    settle(&mut ed, val);

    // A prop batch's instance (a tree, bush or rock near Valentine).
    let vb = ed.world.visible_batches().iter().find(|b| !b.batch.grass && b.batch.len() > 3).unwrap().clone();
    let id = EntityId::instance(vb.key.0, false, vb.batch.list_index, vb.batch.source_index(1));
    ed.select(vec![id]);
    assert!(ed.is_editable(id));
    let before = ed.placement(id).unwrap();
    assert!((before.position - position(&ed, id)).length() < 1e-3);

    // Moved: the ymap's instance and the world's.
    let moved = Placement { position: before.position + Vec3::new(3.0, 0.0, 0.0), ..before };
    ed.set_placements("Move", &[(id, moved)]);
    ed.world.update(val, 12.0);
    assert!((position(&ed, id) - moved.position).length() < 1e-3);
    assert!((ed.placement(id).unwrap().position - moved.position).length() < 1e-3);
    assert!(ed.map(id.node).unwrap().is_edited());
    ed.undo();
    ed.world.update(val, 12.0);
    assert!((position(&ed, id) - before.position).length() < 1e-3);

    // Duplicated in its batch, then the copy deleted.
    let count = rf_world::read_batches(&ed.map(id.node).unwrap().root)
        .iter()
        .find(|b| !b.grass && b.list_index == vb.batch.list_index)
        .unwrap()
        .len();
    ed.duplicate_selected().unwrap();
    let copy = ed.selection()[0];
    assert_eq!(copy.batch_instance(), Some((false, vb.batch.list_index, count as u32)));
    assert!((ed.placement(copy).unwrap().position - before.position).length() < 1e-3);
    ed.delete_selected().unwrap();
    let after = rf_world::read_batches(&ed.map(id.node).unwrap().root);
    assert_eq!(after.iter().find(|b| !b.grass && b.list_index == vb.batch.list_index).unwrap().len(), count);

    // Copied, it pastes as an entity of its archetype.
    ed.select(vec![id]);
    ed.copy();
    assert!(ed.can_paste());

    // Saved and read back, the moved instance is where it was put.
    ed.select(vec![id]);
    ed.set_placements("Move", &[(id, moved)]);
    let dir = std::env::temp_dir().join(format!("rf-batches-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("moved.ymap");
    ed.save_map(id.node, &file).unwrap();
    let meta = Meta::read(&std::fs::read(&file).unwrap()).unwrap();
    let batches = rf_world::read_batches(meta.root.as_ref().unwrap());
    let b = batches.iter().find(|b| !b.grass && b.list_index == vb.batch.list_index).unwrap();
    let t = b.transform(b.index_of(id.batch_instance().unwrap().2).unwrap());
    assert!((Vec3::new(t[12], t[13], t[14]) - moved.position).length() < 1e-3);
    std::fs::remove_dir_all(&dir).ok();
}

/// The game's occluders around Valentine: moved, deleted, undone, and saved.
#[test]
fn occluders_move_delete_undo_and_save() {
    use rf_editor::{OccluderId, OccluderKind, occlusion};

    let Some(mut ed) = editor() else { return };
    let val = ed.world.places().into_iter().find(|(n, _)| n == "Valentine").unwrap().1;
    settle(&mut ed, val);
    let helpers = ed.world.map_helpers();
    let (boxes, _) = helpers.iter().find(|(_, h)| !h.box_occluders.is_empty()).expect("a ymap with box occluders");
    let (models, m) = helpers.iter().find(|(_, h)| h.occlude_models.len() > 1).expect("a ymap with occlude models");
    let (boxes, models, model_count) = (*boxes, *models, m.occlude_models.len());

    // A box: 5 m up, a quarter turn, twice as wide.
    let id = OccluderId { map: boxes, kind: OccluderKind::Box, index: 0 };
    assert!(ed.open_map(boxes));
    let from = ed.occluder_placement(id).unwrap();
    let to = Placement {
        position: from.position + Vec3::Z * 5.0,
        orientation: glam::Quat::from_rotation_z(occlusion::yaw(&from) + 1.0),
        scale: from.scale * Vec3::new(2.0, 1.0, 1.0),
    };
    assert!(ed.set_occluders("Move occluder", &[(id, from, to)]));
    let shown = |ed: &Editor, map: u32| ed.world.map_helpers().into_iter().find(|(n, _)| *n == map).unwrap().1;
    let b = &shown(&ed, boxes).box_occluders[0];
    assert!((Vec3::from(b.centre) - to.position).length() < 1e-4);
    assert!((Vec3::from(b.size) - to.scale).length() < 1e-3);
    assert!((b.angle - occlusion::yaw(&to)).abs() < 1e-4);
    assert!(ed.map(boxes).unwrap().is_edited() && ed.map(boxes).unwrap().in_project);

    // A model: 3 m east; its vertices and box follow.
    let mid = OccluderId { map: models, kind: OccluderKind::Model, index: 1 };
    assert!(ed.open_map(models));
    let before = shown(&ed, models).occlude_models[1].clone();
    let mfrom = ed.occluder_placement(mid).unwrap();
    let mto = Placement { position: mfrom.position + Vec3::X * 3.0, ..mfrom };
    assert!(ed.set_occluders("Move occluder", &[(mid, mfrom, mto)]));
    let after = shown(&ed, models).occlude_models[1].clone();
    assert_eq!((after.vertices.len(), &after.indices), (before.vertices.len(), &before.indices));
    for (a, b) in after.vertices.iter().zip(&before.vertices) {
        assert!((Vec3::from(*a) - Vec3::from(*b) - Vec3::X * 3.0).length() < 1e-3);
    }
    assert!((after.min[0] - before.min[0] - 3.0).abs() < 1e-3);

    // Deleted, the others stay; undone, it's back as moved.
    let other = shown(&ed, models).occlude_models[0].clone();
    assert_eq!(ed.delete_occluders(&[mid]).unwrap(), 1);
    let left = shown(&ed, models);
    assert_eq!((left.occlude_models.len(), &left.occlude_models[0]), (model_count - 1, &other));
    ed.undo();
    assert_eq!(shown(&ed, models).occlude_models[1], after);
    ed.undo();
    assert_eq!(shown(&ed, models).occlude_models[1], before);
    assert!(!ed.map(models).unwrap().is_edited());
    ed.redo();
    ed.redo();

    // Saved and read again: the ymap has what the editor shows.
    let dir = std::env::temp_dir().join(format!("rf-occluders-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for map in [boxes, models] {
        let path = dir.join(format!("{}.ymap", ed.map(map).unwrap().file_name));
        ed.save_map(map, &path).unwrap();
        let root = Meta::read(&std::fs::read(&path).unwrap()).unwrap().root.unwrap();
        assert_eq!(
            occlusion::count(&root, OccluderKind::Model),
            occlusion::count(&ed.map(map).unwrap().root, OccluderKind::Model)
        );
        if map == boxes {
            let p = occlusion::placement(&root, OccluderKind::Box, 0).unwrap();
            assert!(
                (p.position - to.position).length() < 1e-4 && (occlusion::yaw(&p) - occlusion::yaw(&to)).abs() < 1e-4
            );
        } else {
            assert_eq!(occlusion::count(&root, OccluderKind::Model), model_count - 1);
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// A stretch of Valentine's main street blocked off, as the in-game test had
/// it: undo, saving into a project, opening it again and unblocking.
#[test]
fn navmesh_block_save_and_unblock() {
    use rf_editor::PolyRef;
    use rf_editor::navmesh::neighbours;
    let Some(mut ed) = editor() else { return };
    let name = rf_formats::hash::joaat("navmesh[312][333]");
    let game = ed.open_navmesh(name).unwrap().root.clone();
    assert!(ed.navmesh(name).unwrap().can_unblock());

    // The polygons whose centre is in the stretch.
    let (verts, idx, polys) = (game.vertex_positions().unwrap(), game.indices().unwrap(), game.polygons().unwrap());
    let set: Vec<PolyRef> = polys
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            let n = p.index_count() as usize;
            n >= 3 && {
                let c = (0..n).map(|i| Vec3::from(p.vertex(i, &verts, &idx))).sum::<Vec3>() / n as f32;
                (-286.0..=-276.0).contains(&c.x) && (786.0..=800.0).contains(&c.y)
            }
        })
        .map(|(i, _)| PolyRef { navmesh: name, index: i as u32 })
        .collect();
    assert_eq!(set.len(), 126);
    let outside = ed.navmesh_neighbours(&set);
    assert!(!outside.is_empty());

    // Blocked: nothing inside has a neighbour outside, and the other way round.
    assert_eq!(ed.block_polygons(&set).unwrap(), 86);
    let joined = |ed: &Editor| {
        let nm = &ed.navmesh(name).unwrap().root;
        let (p, e) = (nm.polygons().unwrap(), nm.edges().unwrap());
        let inside: std::collections::HashSet<u32> = set.iter().map(|r| r.index).collect();
        (0..p.len() as u32)
            .filter(|i| neighbours(nm, &p, &e, *i).iter().any(|q| inside.contains(i) != inside.contains(q)))
            .count()
    };
    assert_eq!(joined(&ed), 0);
    assert!(ed.navmesh_neighbours(&set).is_empty());
    assert!(
        ed.navmesh_polygon(set[0]).unwrap().flags() & rf_formats::formats::ynv::poly_flags::SWITCHED_OFF_FOR_PEDS != 0
    );
    assert!(ed.undo().is_some());
    assert!(Arc::ptr_eq(&ed.navmesh(name).unwrap().root, &game));
    assert!(ed.redo().is_some());
    assert_eq!(joined(&ed), 0);

    // Saved with the project, and opened again.
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("nav.rfproj");
    ed.new_project(&proj).unwrap();
    assert!(ed.has_unsaved());
    assert!(ed.save_all().unwrap().is_empty());
    let file = dir.path().join("navmesh[312][333].ynv");
    assert!(file.exists());
    ed.close_project();
    assert!(ed.navmesh(name).is_none());
    let problems = ed.open_project(&proj).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    let entries = ed.project_entries();
    assert_eq!((entries.len(), entries[0].name.as_str(), entries[0].dirty), (1, "navmesh[312][333].ynv", false));
    assert_eq!(joined(&ed), 0);

    // Unblocked: the game's edges and flags again.
    assert_eq!(ed.unblock_polygons(&set).unwrap(), 86);
    let nm = ed.navmesh(name).unwrap().root.clone();
    assert_eq!(nm.edges(), game.edges());
    assert_eq!(nm.polygons(), game.polygons());
    assert!(ed.has_unsaved());
}
