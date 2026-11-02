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
