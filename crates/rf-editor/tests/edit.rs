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
    let blocked = ed.block_polygons(&set).unwrap();
    assert_eq!(blocked.edges, 86);
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
    let back = ed.unblock_polygons(&set).unwrap();
    assert_eq!(back, blocked);
    let nm = ed.navmesh(name).unwrap().root.clone();
    assert_eq!(nm.edges(), game.edges());
    assert_eq!(nm.polygons(), game.polygons());
    assert_eq!(nm.special_links_inds, game.special_links_inds);
    assert!(ed.has_unsaved());
}

/// A box of Valentine's main street made again from its collision: still
/// walkable across, joined both ways, about as much ground, and undone.
#[test]
fn navmesh_generated_in_a_box() {
    use rf_editor::navgen::{Settings, stitch::Region};
    use rf_formats::formats::ynv;
    let Some(mut ed) = editor() else { return };
    let at = [-275.0, 790.0, 118.0];
    settle(&mut ed, at);
    let name = rf_formats::hash::joaat("navmesh[312][333]");
    let old = ed.open_navmesh(name).unwrap().root.clone();
    let region =
        Region { min: glam::Vec2::new(-285.0, 780.0), max: glam::Vec2::new(-265.0, 800.0), z_min: 110.0, z_max: 135.0 };
    let started = Instant::now();
    let report = ed.generate_navmesh(region, &Settings::default()).unwrap();
    let s = &report.stitched;
    eprintln!(
        "{} triangles, {} removed, {} pieces, {} added, links {} moved {} dropped, next cells' links {} moved {} lost, in {:?}",
        report.triangles,
        s.removed,
        s.pieces,
        s.added,
        s.links_moved,
        s.links_dropped,
        report.neighbour_links,
        report.neighbour_links_lost,
        started.elapsed()
    );
    assert!(report.triangles > 1000);
    assert!(report.stitched.added > 20 && report.stitched.removed > 10);
    let nm = ed.navmesh(name).unwrap().root.clone();

    // Joined both ways, every index in range.
    let (p, e) = (nm.polygons().unwrap(), nm.edges().unwrap());
    for (i, poly) in p.iter().enumerate() {
        for k in 0..poly.index_count() as usize {
            let q = e[poly.index_id as usize + k].poly_id1();
            if q != 0x7FFF {
                let back = &p[q as usize];
                assert!(
                    (0..back.index_count() as usize).any(|j| e[back.index_id as usize + j].poly_id1() == i as u32),
                    "{i} -> {q} not back"
                );
            }
        }
    }
    // Across the box: from the street west of it to the street east of it.
    let (v, idx) = (nm.vertex_positions().unwrap(), nm.indices().unwrap());
    let centre = |i: usize| {
        let q = &p[i];
        (0..q.index_count() as usize).map(|k| Vec3::from(q.vertex(k, &v, &idx))).sum::<Vec3>() / q.index_count() as f32
    };
    let nearest = |at: Vec3| {
        (0..p.len())
            .filter(|&i| p[i].index_count() >= 3)
            .min_by(|&a, &b| centre(a).distance(at).total_cmp(&centre(b).distance(at)))
            .unwrap()
    };
    let (from, to) = (nearest(Vec3::new(-292.0, 793.0, 117.4)), nearest(Vec3::new(-258.0, 793.0, 117.4)));
    let mut seen = vec![false; p.len()];
    let mut stack = vec![from];
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        for k in 0..p[i].index_count() as usize {
            let q = e[p[i].index_id as usize + k].poly_id1();
            if q != 0x7FFF {
                stack.push(q as usize);
            }
        }
    }
    assert!(seen[to], "no way across the box");
    // About as much walkable ground in the box as the game's.
    let area_in = |nm: &ynv::Navmesh| {
        let (v, i, p) = (nm.vertex_positions().unwrap(), nm.indices().unwrap(), nm.polygons().unwrap());
        p.iter()
            .filter(|q| q.index_count() >= 3)
            .map(|q| (0..q.index_count() as usize).map(|k| Vec3::from(q.vertex(k, &v, &i))).collect::<Vec<_>>())
            .map(|c| rf_editor::navgen::stitch::inside_part(&c, &region))
            .filter(|c| c.len() >= 3)
            .map(|c| rf_editor::navgen::signed_area(&c))
            .sum::<f32>()
    };
    let (before, after) = (area_in(&old), area_in(&nm));
    eprintln!("walkable in the box: {before:.0} m² before, {after:.0} m² after");
    assert!(after > before * 0.75 && after < before * 1.3);
    // Written and read back.
    let again = ynv::read(&ynv::write(&nm)).unwrap();
    assert_eq!(again.polygons(), nm.polygons());
    // The next cell's links into this one point at polygons there.
    let west = rf_formats::hash::joaat("navmesh[309][333]");
    if let Some(w) = ed.navmesh(west) {
        for l in w.root.special_links.iter().flatten() {
            if l.area_id2 as u32 == nm.area_id {
                assert!((l.poly_id2 as usize) < p.len());
            }
        }
    }
    assert!(ed.undo().is_some());
    assert!(Arc::ptr_eq(&ed.navmesh(name).unwrap().root, &old));
}

/// A box across the border of Valentine's two cells (x = -300), shown first
/// and then applied: both cells change, their border polygons keep the
/// stretches the other cell's meet, and the street is still walkable from
/// one cell to the other through them.
#[test]
fn navmesh_made_across_cells() {
    use rf_editor::navgen::{Settings, stitch::Region};
    use rf_formats::formats::ynv::{self, poly_flags};
    use std::collections::{HashMap, HashSet};
    let Some(mut ed) = editor() else { return };
    let at = [-300.0, 790.0, 118.0];
    settle(&mut ed, at);
    let (west, east) = (rf_formats::hash::joaat("navmesh[309][333]"), rf_formats::hash::joaat("navmesh[312][333]"));
    let old_west = ed.open_navmesh(west).unwrap().root.clone();
    let old_east = ed.open_navmesh(east).unwrap().root.clone();
    let region =
        Region { min: glam::Vec2::new(-312.0, 780.0), max: glam::Vec2::new(-290.0, 800.0), z_min: 108.0, z_max: 135.0 };

    // Made, not applied: the documents are as they were.
    let made = ed.make_navmesh(region, &Settings::default()).unwrap();
    let g = &made.generated;
    eprintln!(
        "{:?}: {} triangles, {} added, {} removed, {} pieces, {} border polygons, links {} moved {} dropped, next cells' {} moved {} lost",
        g.navmeshes,
        g.triangles,
        g.stitched.added,
        g.stitched.removed,
        g.stitched.pieces,
        g.stitched.borders,
        g.stitched.links_moved,
        g.stitched.links_dropped,
        g.neighbour_links,
        g.neighbour_links_lost
    );
    assert_eq!(g.navmeshes.len(), 2);
    assert!(g.stitched.borders > 2);
    assert!(Arc::ptr_eq(&ed.navmesh(west).unwrap().root, &old_west));
    assert!(made.navmesh(west).is_some() && made.navmesh(east).is_some());
    ed.apply_made_navmesh(made).unwrap();
    let (new_west, new_east) = (ed.navmesh(west).unwrap().root.clone(), ed.navmesh(east).unwrap().root.clone());

    // A navmesh's polygons: corners and the polygon across each edge.
    struct Mesh {
        corners: Vec<Vec<Vec3>>,
        across: Vec<Vec<Option<usize>>>,
        border: Vec<bool>,
    }
    let mesh = |nm: &ynv::Navmesh| {
        let (v, i, p, e) =
            (nm.vertex_positions().unwrap(), nm.indices().unwrap(), nm.polygons().unwrap(), nm.edges().unwrap());
        let corners: Vec<Vec<Vec3>> = p
            .iter()
            .map(|q| (0..q.index_count() as usize).map(|k| Vec3::from(q.vertex(k, &v, &i))).collect())
            .collect();
        let across = p
            .iter()
            .map(|q| {
                (0..q.index_count() as usize)
                    .map(|k| {
                        let x = e[q.index_id as usize + k];
                        let own = nm.adj_area_ids.get(x.area_id1() as usize) == Some(&nm.area_id);
                        (own && x.poly_id1() != 0x7FFF).then(|| x.poly_id1() as usize)
                    })
                    .collect()
            })
            .collect();
        let border = p.iter().map(|q| q.flags() & poly_flags::ZERO_AREA_STITCH_POLY_DLC != 0).collect();
        Mesh { corners, across, border }
    };
    let key = |p: Vec3| ((p.x * 20.0).round() as i64, (p.y * 20.0).round() as i64);
    // The stretches the border polygons on x = -300 face out along, near the
    // box (from y0 to y1).
    type Stretch = ((i64, i64), (i64, i64));
    let stretches = |m: &Mesh, out: f32, y0: f32, y1: f32| -> HashSet<Stretch> {
        let mut s = HashSet::new();
        for (c, b) in m.corners.iter().zip(&m.border) {
            if !b || !c.iter().all(|p| (p.x + 300.0).abs() < 0.01) || c.iter().all(|p| p.y < y0 || p.y > y1) {
                continue;
            }
            for k in 0..c.len() {
                let (a, b) = (c[k], c[(k + 1) % c.len()]);
                // Left of the edge is out of the cell.
                if (b - a).truncate().perp().x * out > 0.0 {
                    let (ka, kb) = (key(a), key(b));
                    s.insert(if ka < kb { (ka, kb) } else { (kb, ka) });
                }
            }
        }
        s
    };
    let (mw0, me0, mw, me) = (mesh(&old_west), mesh(&old_east), mesh(&new_west), mesh(&new_east));
    assert_eq!(
        stretches(&mw, 1.0, 770.0, 810.0),
        stretches(&mw0, 1.0, 770.0, 810.0),
        "the west cell's stretches changed"
    );
    assert_eq!(
        stretches(&me, -1.0, 770.0, 810.0),
        stretches(&me0, -1.0, 770.0, 810.0),
        "the east cell's stretches changed"
    );
    // Where the box is, they meet (further on, the game's own don't always:
    // another level?).
    assert_eq!(
        stretches(&mw, 1.0, 781.0, 799.0),
        stretches(&me, -1.0, 781.0, 799.0),
        "the cells' stretches don't meet"
    );

    // Joined both ways in each cell.
    for m in [&mw, &me] {
        for (pi, a) in m.across.iter().enumerate() {
            for q in a.iter().flatten() {
                assert!(m.across[*q].contains(&Some(pi)), "{pi} -> {q} not back");
            }
        }
    }

    // From the street west of the box (in the west cell) to the street east
    // of it (in the east cell), crossing between the cells' border polygons
    // by the stretch they share, as the game does when it loads them.
    let nearest = |m: &Mesh, at: Vec3| {
        (0..m.corners.len())
            .filter(|&i| !m.border[i] && m.corners[i].len() >= 3)
            .min_by(|&a, &b| {
                let c = |i: usize| m.corners[i].iter().sum::<Vec3>() / m.corners[i].len() as f32;
                c(a).distance(at).total_cmp(&c(b).distance(at))
            })
            .unwrap()
    };
    let meshes = [&mw, &me];
    let mut by_stretch: HashMap<Stretch, Vec<(usize, usize)>> = HashMap::new();
    for (mi, (m, out)) in meshes.iter().zip([1.0f32, -1.0]).enumerate() {
        for (pi, (c, b)) in m.corners.iter().zip(&m.border).enumerate() {
            if !b || !c.iter().all(|p| (p.x + 300.0).abs() < 0.01) {
                continue;
            }
            for k in 0..c.len() {
                let (a, b) = (c[k], c[(k + 1) % c.len()]);
                if (b - a).truncate().perp().x * out > 0.0 {
                    let (ka, kb) = (key(a), key(b));
                    by_stretch.entry(if ka < kb { (ka, kb) } else { (kb, ka) }).or_default().push((mi, pi));
                }
            }
        }
    }
    let from = (0, nearest(&mw, Vec3::new(-318.0, 793.0, 117.0)));
    let to = (1, nearest(&me, Vec3::new(-284.0, 793.0, 117.4)));
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    let mut stack = vec![from];
    while let Some((mi, pi)) = stack.pop() {
        if !seen.insert((mi, pi)) {
            continue;
        }
        let m = meshes[mi];
        stack.extend(m.across[pi].iter().flatten().map(|q| (mi, *q)));
        if m.border[pi] {
            let c = &m.corners[pi];
            for k in 0..c.len() {
                let (ka, kb) = (key(c[k]), key(c[(k + 1) % c.len()]));
                if let Some(list) = by_stretch.get(&if ka < kb { (ka, kb) } else { (kb, ka) }) {
                    stack.extend(list.iter().filter(|(o, _)| *o != mi).copied());
                }
            }
        }
    }
    assert!(seen.contains(&to), "no way from the west cell to the east one through the box");

    // Written and read back; one undo brings both back.
    for nm in [&new_west, &new_east] {
        assert_eq!(ynv::read(&ynv::write(nm)).unwrap().polygons(), nm.polygons());
    }
    assert!(ed.undo().is_some());
    assert!(Arc::ptr_eq(&ed.navmesh(west).unwrap().root, &old_west));
    assert!(Arc::ptr_eq(&ed.navmesh(east).unwrap().root, &old_east));

    // Made, then the navmesh changed: not applied.
    let made = ed.make_navmesh(region, &Settings::default()).unwrap();
    let any = rf_editor::navmesh::PolyRef { navmesh: east, index: 0 };
    let had = ed.navmesh_polygon(any).unwrap().flags() & poly_flags::ISOLATED;
    ed.set_polygon_flags(&[any], poly_flags::ISOLATED ^ had, had).unwrap();
    assert!(!Arc::ptr_eq(&ed.navmesh(east).unwrap().root, &old_east));
    assert!(ed.apply_made_navmesh(made).is_err());
}
