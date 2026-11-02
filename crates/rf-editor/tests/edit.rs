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
    std::fs::remove_dir_all(&dir).unwrap();

    // Undoing everything removes the new ymap.
    while ed.undo().is_some() {}
    assert!(ed.map(name).is_none() && ed.target.is_none());
}
