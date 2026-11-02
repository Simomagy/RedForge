//! RedForge's editor: the map data being edited, the selection, undo and
//! redo, and saving ymaps. No UI and no GPU: the app drives it and draws what
//! it says.
//!
//! - **Maps** ([`MapDoc`]): a ymap is opened from the game when one of its
//!   entities is selected, and becomes edited when changed: from then on the
//!   [`World`] streams the edited data instead of the file (as the C#
//!   RedForge adopts a game ymap into the project when it's edited). New ymaps
//!   are made with [`Editor::new_map`].
//! - **Undo**: every change is one step holding the maps it changed, before
//!   and after (the data is shared, so a step costs little), and the selection
//!   around it. A gizmo drag or a Properties edit is one step; selecting isn't.
//! - **Entities** are known by [`EntityId`]: the ymap's name hash and the
//!   index in its `entities`. Removing entities moves the indices after them,
//!   so a delete fixes `parentIndex` and `numChildren` in the map, its parent
//!   map and its child maps (the C# hides entities instead).
//! - **Interior entities** live in their interior's MLO archetype, in a ytyp
//!   ([`TypesDoc`]), shared by every placed instance of it: editing one edits
//!   the ytyp, and the world places every instance again. Rooms, portals and
//!   entity sets point at them by index, which deletes keep right.

pub mod interior;
pub mod lod;
pub mod placement;
pub mod save;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rf_formats::formats::databag::DataBag;
use rf_formats::formats::meta::Meta;
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};
use rf_game::GameData;
pub use rf_world::{EntityId, World};

pub use crate::placement::Placement;

/// `lodLevel` values (`rage__eLodType`).
pub mod lod_level {
    pub const HD: i64 = 0;
    pub const LOD: i64 = 1;
    pub const SLOD1: i64 = 2;
    pub const ORPHAN_HD: i64 = 5;
}

/// A ymap open in the editor.
pub struct MapDoc {
    /// The name hash the world knows it by (its file name's).
    pub name: u32,
    /// Lowercase, without folder or extension.
    pub file_name: String,
    /// The game file it came from; `None` for a new ymap.
    pub source: Option<FileRef>,
    /// The file's header and schema, for saving (its root is in `root`).
    header: Meta,
    /// Its `CMapData` now.
    pub root: Arc<DataBag>,
    /// As the game has it; `None` for a new ymap.
    original: Option<Arc<DataBag>>,
    /// As last saved.
    saved: Option<Arc<DataBag>>,
    /// Where it was last saved.
    pub path: Option<PathBuf>,
    /// False for a new ymap whose making was undone.
    pub exists: bool,
}

impl MapDoc {
    /// Whether it differs from the game's.
    pub fn is_edited(&self) -> bool {
        self.original.as_ref().is_none_or(|o| !Arc::ptr_eq(o, &self.root))
    }

    /// Whether it has changes that aren't saved.
    pub fn is_dirty(&self) -> bool {
        match (&self.saved, &self.original) {
            (Some(s), _) => !Arc::ptr_eq(s, &self.root),
            (None, Some(o)) => !Arc::ptr_eq(o, &self.root),
            (None, None) => self.exists,
        }
    }
}

/// A ytyp open in the editor, for its interiors.
pub struct TypesDoc {
    pub file: FileRef,
    /// Lowercase, without folder or extension.
    pub file_name: String,
    header: Meta,
    /// Its `CMapTypes` now.
    pub root: Arc<DataBag>,
    original: Arc<DataBag>,
    saved: Option<Arc<DataBag>>,
    pub path: Option<PathBuf>,
}

impl TypesDoc {
    pub fn is_edited(&self) -> bool {
        !Arc::ptr_eq(&self.original, &self.root)
    }

    pub fn is_dirty(&self) -> bool {
        !Arc::ptr_eq(self.saved.as_ref().unwrap_or(&self.original), &self.root)
    }
}

/// A file the editor changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DocKey {
    /// A ymap, by name hash.
    Map(u32),
    /// A ytyp.
    Types(FileRef),
}

/// A file's root before and after a step; `None` when it didn't exist.
type DocChange = (DocKey, Option<Arc<DataBag>>, Option<Arc<DataBag>>);

/// One undoable change.
struct Step {
    label: String,
    docs: Vec<DocChange>,
    selection: [Vec<EntityId>; 2],
}

/// Where an interior entity is defined.
#[derive(Clone, Copy, Debug)]
struct InteriorRef {
    ytyp: FileRef,
    /// The MLO archetype's index in the ytyp's `archetypes`.
    archetype: usize,
    slot: interior::Slot,
    /// Its instance's placement (an MLO instance's rotation is stored as is).
    instance: Placement,
}

pub struct Editor {
    pub world: World,
    fs: Arc<Rpf8FileSystem>,
    hashes: Arc<HashIndex>,
    data: Arc<GameData>,
    maps: HashMap<u32, MapDoc>,
    types: HashMap<FileRef, TypesDoc>,
    selection: Vec<EntityId>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Copied entities, as definitions with their world placement.
    clipboard: Vec<DataBag>,
    /// Where new entities go: a ymap made in the editor or edited.
    pub target: Option<u32>,
}

/// How many steps undo keeps.
const UNDO_LIMIT: usize = 500;

impl Editor {
    pub fn new(fs: Arc<Rpf8FileSystem>, hashes: Arc<HashIndex>, data: Arc<GameData>) -> Self {
        let world = World::new(fs.clone(), hashes.clone(), data.clone());
        Self {
            world,
            fs,
            hashes,
            data,
            maps: HashMap::new(),
            types: HashMap::new(),
            selection: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            clipboard: Vec::new(),
            target: None,
        }
    }

    pub fn data(&self) -> &GameData {
        &self.data
    }

    pub fn hashes(&self) -> &HashIndex {
        &self.hashes
    }

    // -----------------------------------------------------------------------
    // Maps

    pub fn map(&self, name: u32) -> Option<&MapDoc> {
        self.maps.get(&name).filter(|m| m.exists)
    }

    /// The ymaps edited or made in the editor, by file name.
    pub fn edited_maps(&self) -> Vec<&MapDoc> {
        let mut list: Vec<&MapDoc> = self.maps.values().filter(|m| m.exists && m.is_edited()).collect();
        list.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        list
    }

    /// Opens a ymap of the game (or the world's edited data for it).
    fn open(&mut self, name: u32) -> Result<&mut MapDoc, String> {
        if !self.maps.contains_key(&name) {
            let file = self.world.map_file(name).ok_or("not a ymap of the game")?;
            let file_name = self.world.map_file_name(name).unwrap_or_default().to_owned();
            let bytes = self.fs.extract_loose(file).map_err(|e| format!("{file_name}: {e}"))?;
            let mut header = Meta::read(&bytes).map_err(|e| format!("{file_name}: {e}"))?;
            let root = Arc::new(header.root.take().ok_or_else(|| format!("{file_name}: no map data"))?);
            let doc = MapDoc {
                name,
                file_name,
                source: Some(file),
                header,
                root: root.clone(),
                original: Some(root),
                saved: None,
                path: None,
                exists: true,
            };
            self.maps.insert(name, doc);
        }
        Ok(self.maps.get_mut(&name).unwrap())
    }

    /// The ytyps whose interiors were edited, by file name.
    pub fn edited_types(&self) -> Vec<&TypesDoc> {
        let mut list: Vec<&TypesDoc> = self.types.values().filter(|t| t.is_edited()).collect();
        list.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        list
    }

    fn open_types(&mut self, file: FileRef) -> Result<&mut TypesDoc, String> {
        if !self.types.contains_key(&file) {
            let path = self.fs.path(file);
            let file_name = path.rsplit('/').next().unwrap_or(&path);
            let file_name = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s).to_lowercase();
            let bytes = self.fs.extract_loose(file).map_err(|e| format!("{file_name}: {e}"))?;
            let mut header = Meta::read(&bytes).map_err(|e| format!("{file_name}: {e}"))?;
            let root = Arc::new(header.root.take().ok_or_else(|| format!("{file_name}: no data"))?);
            let doc = TypesDoc { file, file_name, header, root: root.clone(), original: root, saved: None, path: None };
            self.types.insert(file, doc);
        }
        Ok(self.types.get_mut(&file).unwrap())
    }

    /// Writes an edited ytyp.
    pub fn save_types(&mut self, file: FileRef, path: &Path) -> Result<(), String> {
        let doc = self.types.get(&file).ok_or("no such ytyp")?;
        let mut meta = doc.header.clone();
        meta.root = Some((*doc.root).clone());
        let bytes = meta.save().map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc = self.types.get_mut(&file).unwrap();
        doc.saved = Some(doc.root.clone());
        doc.path = Some(path.to_owned());
        Ok(())
    }

    /// Puts a ytyp's data in place, and its interiors in the world.
    fn apply_types(&mut self, file: FileRef, root: Arc<DataBag>) {
        let Some(doc) = self.types.get_mut(&file) else { return };
        doc.root = root;
        let edited = doc.is_edited();
        let mlo = joaat("CMloArchetypeDef");
        for a in interior::archetypes(&doc.root).iter().flatten().filter(|a| a.class_name() == mlo) {
            self.world.set_interior(a.hash("name"), edited.then_some(a));
        }
    }

    fn apply(&mut self, key: DocKey, root: Option<Arc<DataBag>>) {
        match (key, root) {
            (DocKey::Map(name), root) => self.apply_map(name, root),
            (DocKey::Types(file), Some(root)) => self.apply_types(file, root),
            (DocKey::Types(_), None) => {}
        }
    }

    fn root(&self, key: DocKey) -> Option<&Arc<DataBag>> {
        match key {
            DocKey::Map(name) => self.map(name).map(|m| &m.root),
            DocKey::Types(file) => self.types.get(&file).map(|t| &t.root),
        }
    }

    /// Makes a new, empty ymap and makes it the [target](Self::target).
    pub fn new_map(&mut self, file_name: &str) -> Result<u32, String> {
        let file_name = file_name.trim().trim_end_matches(".ymap").to_lowercase();
        if file_name.is_empty() {
            return Err("the ymap needs a name".into());
        }
        let name = joaat(&file_name);
        if self.world.map_file_name(name).is_some() || self.maps.get(&name).is_some_and(|m| m.exists) {
            return Err(format!("there's already a ymap called {file_name}"));
        }
        let mut root = Meta::create("CMapData").ok_or("no CMapData in the schema")?;
        root.set_hash("name", name);
        let root = Arc::new(root);
        let doc = MapDoc {
            name,
            file_name,
            source: None,
            header: Meta::new(None),
            root: root.clone(),
            original: None,
            saved: None,
            path: None,
            exists: false,
        };
        self.maps.insert(name, doc);
        let before = self.selection.clone();
        self.apply_map(name, Some(root.clone()));
        self.push(Step {
            label: "New ymap".into(),
            docs: vec![(DocKey::Map(name), None, Some(root))],
            selection: [before.clone(), before],
        });
        self.target = Some(name);
        Ok(name)
    }

    /// Writes an edited ymap (as a resource, like the game's).
    pub fn save_map(&mut self, name: u32, path: &Path) -> Result<(), String> {
        let doc = self.maps.get(&name).filter(|m| m.exists).ok_or("no such ymap")?;
        let mut root = (*doc.root).clone();
        save::update_map(&mut root, &self.data.archetypes);
        let mut meta = doc.header.clone();
        meta.root = Some(root);
        let bytes = meta.save().map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc = self.maps.get_mut(&name).unwrap();
        doc.saved = Some(doc.root.clone());
        doc.path = Some(path.to_owned());
        Ok(())
    }

    /// Puts a map's data in place (and in the world).
    fn apply_map(&mut self, name: u32, root: Option<Arc<DataBag>>) {
        let Some(doc) = self.maps.get_mut(&name) else { return };
        match root {
            Some(root) => {
                doc.exists = true;
                doc.root = root.clone();
                if doc.is_edited() {
                    self.world.set_map(name, &doc.file_name, root);
                } else {
                    self.world.clear_map(name, doc.original.as_deref());
                }
            }
            None => {
                doc.exists = false;
                if self.target == Some(name) {
                    self.target = None;
                }
                self.world.clear_map(name, None);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Selection

    pub fn selection(&self) -> &[EntityId] {
        &self.selection
    }

    /// Selects entities (not an undo step); opens their maps, and the ytyps
    /// of interior entities.
    pub fn select(&mut self, ids: Vec<EntityId>) {
        for id in &ids {
            if let Err(e) = self.open(id.node) {
                log::warn!("{e}");
                continue;
            }
            if id.interior_entity != u32::MAX
                && let Some(def) = lod::entities(&self.maps[&id.node].root).get(id.entity as usize).cloned().flatten()
                && let Some(a) = self.data.archetypes.get(&def.hash("archetypeName"))
                && let Err(e) = self.open_types(a.ytyp)
            {
                log::warn!("{e}");
            }
        }
        self.selection = ids;
    }

    /// Where an interior entity is defined (its ytyp must be open).
    fn interior_ref(&self, id: EntityId) -> Option<InteriorRef> {
        if id.interior_entity == u32::MAX {
            return None;
        }
        let def = lod::entities(&self.map(id.node)?.root).get(id.entity as usize)?.as_ref()?;
        let a = self.data.archetypes.get(&def.hash("archetypeName"))?;
        let mlo = interior::archetypes(&self.types.get(&a.ytyp)?.root).get(a.index as usize)?.as_ref()?;
        let slot = interior::slot(mlo, id.interior_entity as usize)?;
        Some(InteriorRef { ytyp: a.ytyp, archetype: a.index as usize, slot, instance: Placement::of_entity(def) })
    }

    fn interior_def(&self, r: &InteriorRef) -> Option<&DataBag> {
        let mlo = interior::archetypes(&self.types.get(&r.ytyp)?.root).get(r.archetype)?.as_ref()?;
        interior::entity(mlo, r.slot)
    }

    /// Adds an entity to the selection, or takes it out.
    pub fn toggle(&mut self, id: EntityId) {
        let mut ids = self.selection.clone();
        match ids.iter().position(|s| *s == id) {
            Some(i) => {
                ids.remove(i);
            }
            None => ids.push(id),
        }
        self.select(ids);
    }

    /// An entity's definition: a ymap's `CEntityDef` or `CMloInstanceDef`, or
    /// an interior's `CEntityDef` (placed in the interior's space).
    pub fn entity_def(&self, id: EntityId) -> Option<&DataBag> {
        if id.interior_entity != u32::MAX {
            return self.interior_def(&self.interior_ref(id)?);
        }
        lod::entities(&self.map(id.node)?.root).get(id.entity as usize)?.as_ref()
    }

    /// Where an entity is in the world.
    pub fn placement(&self, id: EntityId) -> Option<Placement> {
        if let Some(r) = self.interior_ref(id) {
            let local = Placement::of_entity(self.interior_def(&r)?);
            let mlo = r.instance;
            return Some(Placement {
                position: mlo.position + mlo.orientation * local.position,
                orientation: (mlo.orientation * local.orientation).normalize(),
                scale: local.scale,
            });
        }
        if let Some(def) = self.entity_def(id) {
            return Some(Placement::of_entity(def));
        }
        let e = self.world.entity(id)?;
        let (scale, orientation, position) = glam::Mat4::from_cols_array(&e.transform).to_scale_rotation_translation();
        Some(Placement { position, orientation, scale })
    }

    /// A world placement in an interior's space (`RDR2Entity.ApplyTransforms`).
    fn to_interior(r: &InteriorRef, p: Placement) -> Placement {
        let inv = r.instance.orientation.inverse();
        Placement {
            position: inv * (p.position - r.instance.position),
            orientation: (inv * p.orientation).normalize(),
            scale: p.scale,
        }
    }

    /// Whether an entity can be changed here.
    pub fn is_editable(&self, id: EntityId) -> bool {
        self.entity_def(id).is_some()
    }

    // -----------------------------------------------------------------------
    // Changes

    /// Shows entities elsewhere while dragging, without a step.
    pub fn preview(&mut self, items: &[(EntityId, Placement)]) {
        for (id, p) in items {
            self.world.preview_transform(*id, p.matrix().to_cols_array());
        }
    }

    /// Moves entities: one step.
    pub fn set_placements(&mut self, label: &str, items: &[(EntityId, Placement)]) {
        let edits: Vec<(EntityId, Placement)> = items
            .iter()
            .filter(|(id, _)| self.is_editable(*id))
            .map(|&(id, p)| match self.interior_ref(id) {
                Some(r) => (id, Self::to_interior(&r, p)),
                None => (id, p),
            })
            .collect();
        self.change_entities(label, edits.iter().map(|(id, _)| *id), |id, def| {
            if let Some((_, p)) = edits.iter().find(|(i, _)| *i == id) {
                p.write_entity(def);
            }
        });
    }

    /// Changes the selected entities' definitions with `f`: one step.
    pub fn modify(&mut self, label: &str, mut f: impl FnMut(&mut DataBag)) {
        let ids: Vec<EntityId> = self.selection.iter().copied().filter(|id| self.is_editable(*id)).collect();
        self.change_entities(label, ids.into_iter(), |_, def| f(def));
    }

    fn change_entities(
        &mut self,
        label: &str,
        ids: impl Iterator<Item = EntityId>,
        mut f: impl FnMut(EntityId, &mut DataBag),
    ) {
        let mut roots: HashMap<DocKey, DataBag> = HashMap::new();
        for id in ids {
            match self.interior_ref(id) {
                Some(r) => {
                    let key = DocKey::Types(r.ytyp);
                    let Some(base) = self.root(key) else { continue };
                    let root = roots.entry(key).or_insert_with(|| (**base).clone());
                    if let Some(def) = interior::archetypes_mut(root)
                        .and_then(|l| l.get_mut(r.archetype))
                        .and_then(Option::as_mut)
                        .and_then(|mlo| interior::entity_mut(mlo, r.slot))
                    {
                        f(id, def);
                    }
                }
                None => {
                    let key = DocKey::Map(id.node);
                    let Some(base) = self.root(key) else { continue };
                    let root = roots.entry(key).or_insert_with(|| (**base).clone());
                    if let Some(Some(def)) = lod::entities_mut(root).get_mut(id.entity as usize) {
                        f(id, def);
                    }
                }
            }
        }
        if roots.is_empty() {
            return;
        }
        let selection = self.selection.clone();
        self.commit(label, roots.into_iter().collect(), selection);
    }

    /// Records a step changing files to `roots`, and applies it.
    fn commit(&mut self, label: &str, roots: Vec<(DocKey, DataBag)>, selection: Vec<EntityId>) {
        let mut docs = Vec::new();
        for (key, root) in roots {
            let before = self.root(key).cloned();
            let after = Arc::new(root);
            self.apply(key, Some(after.clone()));
            docs.push((key, before, Some(after)));
        }
        let before = std::mem::replace(&mut self.selection, selection.clone());
        self.push(Step { label: label.to_owned(), docs, selection: [before, selection] });
    }

    fn push(&mut self, step: Step) {
        self.undo.push(step);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Removes the selected entities, fixing the indices that point at the
    /// ones after them (LOD parents; for interiors, rooms, portals and sets).
    /// Refuses entities with LOD children.
    pub fn delete_selected(&mut self) -> Result<(), String> {
        let mut by_map: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut by_interior: HashMap<(FileRef, usize), Vec<interior::Slot>> = HashMap::new();
        for id in &self.selection {
            if !self.is_editable(*id) {
                return Err("something selected can't be edited".into());
            }
            match self.interior_ref(*id) {
                Some(r) => by_interior.entry((r.ytyp, r.archetype)).or_default().push(r.slot),
                None => by_map.entry(id.node).or_default().push(id.entity),
            }
        }
        let mut types: HashMap<FileRef, DataBag> = HashMap::new();
        for ((ytyp, archetype), slots) in by_interior {
            let root = types.entry(ytyp).or_insert_with(|| (*self.types[&ytyp].root).clone());
            if let Some(Some(mlo)) = interior::archetypes_mut(root).and_then(|l| l.get_mut(archetype)) {
                interior::remove(mlo, &slots);
            }
        }
        let mut roots: HashMap<u32, DataBag> = HashMap::new();
        for (name, mut removed) in by_map {
            removed.sort_unstable();
            removed.dedup();
            let doc = self.map(name).ok_or("map not open")?;
            if let Some(i) = lod::with_children(&doc.root, &removed) {
                let archetype = lod::entities(&doc.root)[i as usize].as_ref().map_or(0, |d| d.hash("archetypeName"));
                return Err(format!("{} has LOD children: delete them first", self.hashes.name(archetype)));
            }
            let mut root = roots.remove(&name).unwrap_or_else(|| (*doc.root).clone());
            let r = lod::remove(&mut root, &removed);
            roots.insert(name, root);
            // Child maps point into this one by index.
            for child in self.world.child_maps(name) {
                if self.open(child).is_err() {
                    continue;
                }
                let mut c = roots.remove(&child).unwrap_or_else(|| (*self.maps[&child].root).clone());
                if lod::follow_parent_map(&mut c, &r.remap) {
                    roots.insert(child, c);
                }
            }
            // The parents of removed entities in the parent map have fewer children.
            if !r.parent_lost.is_empty()
                && let Some(parent) = self.world.parent_map(name)
                && self.open(parent).is_ok()
            {
                let mut p = roots.remove(&parent).unwrap_or_else(|| (*self.maps[&parent].root).clone());
                lod::drop_children(&mut p, &r.parent_lost);
                roots.insert(parent, p);
            }
        }
        let changes = roots
            .into_iter()
            .map(|(k, v)| (DocKey::Map(k), v))
            .chain(types.into_iter().map(|(k, v)| (DocKey::Types(k), v)))
            .collect::<Vec<_>>();
        if !changes.is_empty() {
            self.commit("Delete", changes, Vec::new());
        }
        Ok(())
    }

    /// Copies the selected entities, as standalone ones (no LOD links).
    fn detached(def: &DataBag) -> DataBag {
        let mut d = def.clone();
        d.set_i32("parentIndex", -1);
        d.set_u32("numChildren", 0);
        d.set_u64("id", 0);
        d.set_u32("flags", d.u32("flags") & !lod::LOD_IN_PARENT_MAP);
        if d.enum_value("lodLevel") == lod_level::HD {
            d.set_enum_value("lodLevel", lod_level::ORPHAN_HD);
        }
        d
    }

    /// Duplicates the selected entities where they are (as the C#'s
    /// Shift+drag): into their ymaps, or their interiors (in the same rooms).
    /// Selects the copies.
    pub fn duplicate_selected(&mut self) -> Result<(), String> {
        let mut roots: HashMap<DocKey, DataBag> = HashMap::new();
        // Interior copies: (their id without the index, the MLO, the slot).
        let mut interior_copies = Vec::new();
        let mut copies = Vec::new();
        for id in self.selection.clone() {
            let def = self.entity_def(id).ok_or("something selected can't be edited")?;
            let copy = Self::detached(def);
            match self.interior_ref(id) {
                Some(r) => {
                    let key = DocKey::Types(r.ytyp);
                    let root = roots.entry(key).or_insert_with(|| (*self.types[&r.ytyp].root).clone());
                    let mlo = interior::archetypes_mut(root)
                        .and_then(|l| l.get_mut(r.archetype))
                        .and_then(Option::as_mut)
                        .ok_or("interior not found")?;
                    let slot = interior::duplicate(mlo, r.slot, copy).ok_or("interior not found")?;
                    interior_copies.push((copies.len(), r, slot));
                    copies.push(id);
                }
                None => {
                    let key = DocKey::Map(id.node);
                    let root = roots.entry(key).or_insert_with(|| (*self.maps[&id.node].root).clone());
                    let list = lod::entities_mut(root);
                    copies.push(EntityId { node: id.node, entity: list.len() as u32, interior_entity: u32::MAX });
                    list.push(Some(copy));
                }
            }
        }
        // Copies in interiors count after every change to their interior.
        for (i, r, slot) in interior_copies {
            let root = &roots[&DocKey::Types(r.ytyp)];
            if let Some(mlo) = interior::archetypes(root).get(r.archetype).and_then(Option::as_ref) {
                copies[i].interior_entity = interior::world_index(mlo, slot);
            }
        }
        if !copies.is_empty() {
            self.commit("Duplicate", roots.into_iter().collect(), copies);
        }
        Ok(())
    }

    /// Copies the selected entities, as standalone ones placed in the world.
    pub fn copy(&mut self) {
        self.clipboard = self
            .selection
            .iter()
            .filter_map(|id| {
                let mut def = Self::detached(self.entity_def(*id)?);
                if id.interior_entity != u32::MAX {
                    self.placement(*id)?.write_entity(&mut def);
                }
                Some(def)
            })
            .collect();
    }

    pub fn can_paste(&self) -> bool {
        !self.clipboard.is_empty()
    }

    /// Pastes copied entities where they were, into the target ymap.
    pub fn paste(&mut self) -> Result<(), String> {
        let target = self.target.filter(|t| self.map(*t).is_some()).ok_or("choose a ymap to place into first")?;
        let mut root = (*self.maps[&target].root).clone();
        let list = lod::entities_mut(&mut root);
        let mut ids = Vec::new();
        for def in &self.clipboard {
            ids.push(EntityId { node: target, entity: list.len() as u32, interior_entity: u32::MAX });
            list.push(Some(def.clone()));
        }
        self.commit("Paste", vec![(DocKey::Map(target), root)], ids);
        Ok(())
    }

    /// A new entity of an archetype in the target ymap, with the C#'s
    /// defaults: standalone, its archetype's `lodDist`.
    pub fn place(&mut self, archetype: u32, placement: Placement) -> Result<EntityId, String> {
        let target = self.target.filter(|t| self.map(*t).is_some()).ok_or("choose a ymap to place into first")?;
        let arch = self.data.archetypes.get(&archetype).ok_or("unknown archetype")?;
        if arch.class == rf_game::Archetype::MLO_CLASS {
            return Err("interiors can't be placed yet".into());
        }
        let mut def = Meta::create("CEntityDef").ok_or("no CEntityDef in the schema")?;
        def.set_hash("archetypeName", archetype);
        def.set_i32("parentIndex", -1);
        def.set_f32("lodDist", arch.lod_dist);
        def.set_enum_value("lodLevel", lod_level::ORPHAN_HD);
        def.set_u8("blendAgeLayer", 255);
        def.set_u8("blendAgeDirt", 255);
        placement.write_entity(&mut def);
        let mut root = (*self.maps[&target].root).clone();
        let list = lod::entities_mut(&mut root);
        let id = EntityId { node: target, entity: list.len() as u32, interior_entity: u32::MAX };
        list.push(Some(def));
        self.commit("Place", vec![(DocKey::Map(target), root)], vec![id]);
        Ok(id)
    }

    // -----------------------------------------------------------------------
    // Undo

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The steps that can be undone, latest first.
    pub fn undo_labels(&self) -> impl Iterator<Item = &str> {
        self.undo.iter().rev().map(|s| s.label.as_str())
    }

    pub fn redo_labels(&self) -> impl Iterator<Item = &str> {
        self.redo.iter().rev().map(|s| s.label.as_str())
    }

    pub fn undo(&mut self) -> Option<String> {
        let step = self.undo.pop()?;
        for (key, before, _) in &step.docs {
            self.apply(*key, before.clone());
        }
        self.selection = step.selection[0].clone();
        let label = step.label.clone();
        self.redo.push(step);
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        let step = self.redo.pop()?;
        for (key, _, after) in &step.docs {
            self.apply(*key, after.clone());
        }
        self.selection = step.selection[1].clone();
        let label = step.label.clone();
        self.undo.push(step);
        Some(label)
    }
}
