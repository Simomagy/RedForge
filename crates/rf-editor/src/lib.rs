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
pub mod project;
pub mod redm;
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
pub use crate::project::Project;

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
    /// In the project: loaded from it, made in the editor, or edited.
    pub in_project: bool,
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
    pub in_project: bool,
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
pub enum DocKey {
    /// A ymap, by name hash.
    Map(u32),
    /// A ytyp.
    Types(FileRef),
}

/// A file's root before and after a step; `None` when it didn't exist.
type DocChange = (DocKey, Option<Arc<DataBag>>, Option<Arc<DataBag>>);

/// A file of the project, for showing it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectEntry {
    /// File name, with its extension.
    pub name: String,
    /// The open document; `None` for a file that isn't loaded.
    pub doc: Option<DocKey>,
    /// Where it is on disk; `None` until it's saved.
    pub path: Option<PathBuf>,
    pub dirty: bool,
}

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
    /// The project: made when something is first edited, as in the C#.
    pub project: Option<Project>,
    /// Project files that aren't loaded (custom models, textures, collisions
    /// and new ytyps aren't yet).
    pub unloaded: Vec<PathBuf>,
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
            project: None,
            unloaded: Vec::new(),
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
                in_project: false,
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
            let doc = TypesDoc {
                file,
                file_name,
                header,
                root: root.clone(),
                original: root,
                saved: None,
                path: None,
                in_project: false,
            };
            self.types.insert(file, doc);
        }
        Ok(self.types.get_mut(&file).unwrap())
    }

    /// Writes an edited ytyp.
    pub fn save_types(&mut self, file: FileRef, path: &Path) -> Result<(), String> {
        self.project.get_or_insert_with(Project::new).add_file(path);
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
            in_project: true,
        };
        self.maps.insert(name, doc);
        self.project.get_or_insert_with(Project::new);
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
        self.project.get_or_insert_with(Project::new).add_file(path);
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
    // Project

    /// Puts a changed file in the project (making one if there's none).
    fn adopt(&mut self, key: DocKey) {
        self.project.get_or_insert_with(Project::new);
        match key {
            DocKey::Map(name) => {
                if let Some(m) = self.maps.get_mut(&name) {
                    m.in_project = true;
                }
            }
            DocKey::Types(file) => {
                if let Some(t) = self.types.get_mut(&file) {
                    t.in_project = true;
                }
            }
        }
    }

    /// Makes a new project, saved to `file` (an `.rfproj`); what's edited now
    /// stays in it.
    pub fn new_project(&mut self, file: &Path) -> Result<(), String> {
        let mut p = self.project.take().unwrap_or_default();
        p.file = None;
        let saved = p.save(Some(file));
        self.project = Some(p);
        saved.map(|_| ())
    }

    /// Closes the project: its files go back to the game's, and undo starts
    /// over.
    pub fn close_project(&mut self) {
        let maps: Vec<u32> = self.maps.values().filter(|m| m.in_project).map(|m| m.name).collect();
        for name in maps {
            let doc = self.maps.remove(&name).unwrap();
            self.world.clear_map(name, None);
            let _ = doc;
        }
        let types: Vec<FileRef> = self.types.values().filter(|t| t.in_project).map(|t| t.file).collect();
        for file in types {
            let doc = self.types.remove(&file).unwrap();
            let original = doc.original.clone();
            self.types.insert(file, TypesDoc { root: original.clone(), ..doc });
            self.apply_types(file, original);
            self.types.remove(&file);
        }
        self.project = None;
        self.unloaded.clear();
        self.selection.clear();
        self.target = None;
        self.undo.clear();
        self.redo.clear();
    }

    /// Opens an `.rfproj` (closing the project there is): its ymaps and ytyps
    /// replace the game's, or add to them.
    pub fn open_project(&mut self, file: &Path) -> Result<Vec<String>, String> {
        let project = Project::load(file)?;
        self.close_project();
        let items = project.items();
        self.project = Some(project);
        Ok(self.load_project_files(&items))
    }

    /// Adds files to the project and loads them; returns the problems.
    pub fn add_project_files(&mut self, files: &[PathBuf]) -> Vec<String> {
        let project = self.project.get_or_insert_with(Project::new);
        for f in files {
            project.add_file(f);
        }
        let mut files = files.to_vec();
        files.sort_by_key(|f| project::load_order(f));
        self.load_project_files(&files)
    }

    /// Adds a folder (its game files) to the project and loads them.
    pub fn add_project_folder(&mut self, folder: &Path) -> Vec<String> {
        let project = self.project.get_or_insert_with(Project::new);
        let before = project.items();
        project.add_folder(folder);
        let new: Vec<PathBuf> = project.items().into_iter().filter(|f| !before.contains(f)).collect();
        self.load_project_files(&new)
    }

    fn load_project_files(&mut self, files: &[PathBuf]) -> Vec<String> {
        let mut problems = Vec::new();
        for f in files {
            let result = match project::extension(f).as_str() {
                "ymap" => self.load_map_file(f),
                "ytyp" => self.load_types_file(f),
                _ => Err(String::new()),
            };
            match result {
                Ok(()) => {}
                Err(e) if e.is_empty() => self.unloaded.push(f.clone()),
                Err(e) => {
                    problems.push(e);
                    self.unloaded.push(f.clone());
                }
            }
        }
        problems
    }

    /// A ymap from disk, in place of the game's of the same name (or added).
    fn load_map_file(&mut self, path: &Path) -> Result<(), String> {
        let shown = path.display().to_string();
        let bytes = std::fs::read(path).map_err(|e| format!("{shown}: {e}"))?;
        let mut header = Meta::read(&bytes).map_err(|e| format!("{shown}: {e}"))?;
        let root = Arc::new(header.root.take().ok_or_else(|| format!("{shown}: no map data"))?);
        let file_name = path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
        let name = joaat(&file_name);
        let doc = MapDoc {
            name,
            source: self.world.map_file(name),
            file_name,
            header,
            root: root.clone(),
            original: None,
            saved: Some(root.clone()),
            path: Some(path.to_owned()),
            exists: true,
            in_project: true,
        };
        self.maps.insert(name, doc);
        self.apply_map(name, Some(root));
        Ok(())
    }

    /// A ytyp from disk in place of the game's of the same name, for its
    /// interiors. (New ytyps, with new archetypes, aren't loaded yet.)
    fn load_types_file(&mut self, path: &Path) -> Result<(), String> {
        let shown = path.display().to_string();
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
        let file = self.data.stream_entry("ytyp", joaat(&stem)).ok_or_else(String::new)?;
        let bytes = std::fs::read(path).map_err(|e| format!("{shown}: {e}"))?;
        let mut meta = Meta::read(&bytes).map_err(|e| format!("{shown}: {e}"))?;
        let root = Arc::new(meta.root.take().ok_or_else(|| format!("{shown}: no data"))?);
        let doc = self.open_types(file)?;
        doc.root = root.clone();
        doc.saved = Some(root.clone());
        doc.path = Some(path.to_owned());
        doc.in_project = true;
        self.apply_types(file, root);
        Ok(())
    }

    /// The project's files, loaded or not, and what's edited but not saved
    /// yet.
    pub fn project_entries(&self) -> Vec<ProjectEntry> {
        let mut out = Vec::new();
        for m in self.maps.values().filter(|m| m.in_project && m.exists) {
            out.push(ProjectEntry {
                name: format!("{}.ymap", m.file_name),
                doc: Some(DocKey::Map(m.name)),
                path: m.path.clone(),
                dirty: m.is_dirty(),
            });
        }
        for t in self.types.values().filter(|t| t.in_project) {
            out.push(ProjectEntry {
                name: format!("{}.ytyp", t.file_name),
                doc: Some(DocKey::Types(t.file)),
                path: t.path.clone(),
                dirty: t.is_dirty(),
            });
        }
        for f in &self.unloaded {
            let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
            out.push(ProjectEntry { name, doc: None, path: Some(f.clone()), dirty: false });
        }
        out.sort_by(|a, b| {
            let order = |e: &ProjectEntry| project::load_order(Path::new(&e.name));
            order(a).cmp(&order(b)).then_with(|| a.name.cmp(&b.name))
        });
        out
    }

    /// Where a file of the project is saved: where it was, or the project's
    /// folder.
    pub fn save_path(&self, doc: DocKey) -> Option<PathBuf> {
        let (path, name) = match doc {
            DocKey::Map(n) => self.maps.get(&n).map(|m| (m.path.clone(), format!("{}.ymap", m.file_name)))?,
            DocKey::Types(f) => self.types.get(&f).map(|t| (t.path.clone(), format!("{}.ytyp", t.file_name)))?,
        };
        path.or_else(|| Some(self.project.as_ref()?.dir()?.join(name)))
    }

    pub fn save_doc(&mut self, doc: DocKey, path: &Path) -> Result<(), String> {
        match doc {
            DocKey::Map(n) => self.save_map(n, path)?,
            DocKey::Types(f) => self.save_types(f, path)?,
        }
        self.save_project_file()
    }

    /// Writes the `.rfproj`, if the project has one.
    pub fn save_project_file(&mut self) -> Result<(), String> {
        match &mut self.project {
            Some(p) if p.file.is_some() => p.save(None).map(|_| ()),
            _ => Ok(()),
        }
    }

    /// Saves every changed file of the project where it goes. Returns the
    /// ones that have nowhere to go yet (the project isn't saved).
    pub fn save_all(&mut self) -> Result<Vec<DocKey>, String> {
        let mut homeless = Vec::new();
        for e in self.project_entries().into_iter().filter(|e| e.dirty) {
            let Some(doc) = e.doc else { continue };
            match self.save_path(doc) {
                Some(path) => match doc {
                    DocKey::Map(n) => self.save_map(n, &path)?,
                    DocKey::Types(f) => self.save_types(f, &path)?,
                },
                None => homeless.push(doc),
            }
        }
        self.save_project_file()?;
        Ok(homeless)
    }

    /// The project's `_manifest.ymf`, as XML (the C#'s
    /// `RDR2ProjectYmfGenerator`): the ytyps each of its ymaps needs, and for
    /// interiors the ytyps their entities need.
    pub fn manifest_xml(&self) -> String {
        use std::collections::{BTreeMap, BTreeSet};
        let ytyp_of = |archetype: u32| {
            let a = self.data.archetypes.get(&archetype)?;
            let path = self.fs.path(a.ytyp);
            let name = path.rsplit('/').next().unwrap_or(&path);
            Some(name.rsplit_once('.').map_or(name, |(s, _)| s).to_lowercase())
        };
        let mut imaps: BTreeMap<String, (BTreeSet<String>, bool)> = BTreeMap::new();
        let mut ityps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mlo_class = joaat("CMloArchetypeDef");
        for m in self.maps.values().filter(|m| m.in_project && m.exists) {
            let (deps, interior) = imaps.entry(m.file_name.clone()).or_default();
            for def in lod::entities(&m.root).iter().flatten() {
                let archetype = def.hash("archetypeName");
                let Some(ytyp) = ytyp_of(archetype) else { continue };
                deps.insert(ytyp.clone());
                if placement::is_mlo_instance(def) {
                    *interior = true;
                    let typdeps = ityps.entry(ytyp.clone()).or_default();
                    // The interior's own entities, from its (maybe edited) ytyp.
                    let a = &self.data.archetypes[&archetype];
                    let root = match self.types.get(&a.ytyp) {
                        Some(t) => Some(t.root.clone()),
                        None => self.data.archetype_bag(&self.fs, &self.hashes, archetype).map(|b| {
                            let mut wrap = Meta::create("CMapTypes").expect("CMapTypes");
                            wrap.set_object(
                                "archetypes",
                                Some(rf_formats::formats::databag::Value::Bags(vec![Some(b)])),
                            );
                            Arc::new(wrap)
                        }),
                    };
                    let Some(root) = root else { continue };
                    for mlo in interior::archetypes(&root).iter().flatten().filter(|b| b.class_name() == mlo_class) {
                        if mlo.hash("name") != archetype {
                            continue;
                        }
                        let sets = mlo.bags("entitySets").unwrap_or_default().iter().flatten();
                        for e in lod::entities(mlo).iter().chain(sets.flat_map(|s| lod::entities(s).iter())).flatten() {
                            if let Some(y) = ytyp_of(e.hash("archetypeName")) {
                                typdeps.insert(y);
                            }
                        }
                    }
                }
            }
            if deps.is_empty() {
                imaps.remove(&m.file_name);
            }
        }
        let mut x =
            String::from("<CPackFileMetaData>\n  <MapDataGroups/>\n  <HDTxdBindingArray/>\n  <imapDependencies/>\n");
        if imaps.is_empty() {
            x.push_str("  <imapDependencies_2/>\n");
        } else {
            x.push_str("  <imapDependencies_2>\n");
            for (name, (deps, interior)) in &imaps {
                x.push_str(&format!("    <Item>\n      <imapName>{name}</imapName>\n"));
                x.push_str(if *interior {
                    "      <manifestFlags>INTERIOR_DATA</manifestFlags>\n"
                } else {
                    "      <manifestFlags/>\n"
                });
                x.push_str("      <itypDepArray>\n");
                for d in deps {
                    x.push_str(&format!("        <Item>{d}</Item>\n"));
                }
                x.push_str("      </itypDepArray>\n    </Item>\n");
            }
            x.push_str("  </imapDependencies_2>\n");
        }
        if ityps.is_empty() {
            x.push_str("  <itypDependencies_2/>\n");
        } else {
            x.push_str("  <itypDependencies_2>\n");
            for (name, deps) in &ityps {
                x.push_str(&format!(
                    "    <Item>\n      <itypName>{name}</itypName>\n      <manifestFlags>INTERIOR_DATA</manifestFlags>\n      <itypDepArray>\n"
                ));
                for d in deps {
                    x.push_str(&format!("        <Item>{d}</Item>\n"));
                }
                x.push_str("      </itypDepArray>\n    </Item>\n");
            }
            x.push_str("  </itypDependencies_2>\n");
        }
        if ityps.is_empty() {
            x.push_str("  <Interiors/>\n");
        } else {
            x.push_str("  <Interiors itemType=\"CInteriorBoundsFiles\">\n");
            for name in ityps.keys() {
                x.push_str(&format!(
                    "    <Item>\n      <Name>{name}</Name>\n      <Bounds>\n        <Item>{name}</Item>\n      </Bounds>\n    </Item>\n"
                ));
            }
            x.push_str("  </Interiors>\n");
        }
        x.push_str("</CPackFileMetaData>\n");
        x
    }

    /// Writes the project's `_manifest.ymf` (PSO, as the game's) and adds it
    /// to the project.
    pub fn write_manifest(&mut self, path: &Path) -> Result<(), String> {
        let cx = rf_formats::formats::XmlContext { hashes: &self.hashes, folder: None };
        let data = rf_formats::formats::pso::from_xml(&self.manifest_xml(), &cx).map_err(|e| e.to_string())?;
        std::fs::write(path, data).map_err(|e| format!("{}: {e}", path.display()))?;
        let project = self.project.get_or_insert_with(Project::new);
        project.add_file(path);
        if !self.unloaded.iter().any(|f| f == path) {
            self.unloaded.push(path.to_owned());
        }
        self.save_project_file()
    }

    /// Whether anything changed isn't saved (the files or the `.rfproj`).
    pub fn has_unsaved(&self) -> bool {
        self.project_entries().iter().any(|e| e.dirty)
    }

    /// Takes a file out of the project: the game's is used again (its edits
    /// are dropped).
    pub fn remove_from_project(&mut self, entry: &ProjectEntry) {
        if let Some(path) = &entry.path
            && let Some(p) = &mut self.project
        {
            p.remove_file(path);
        }
        match entry.doc {
            Some(DocKey::Map(name)) => {
                self.maps.remove(&name);
                self.world.clear_map(name, None);
                if self.target == Some(name) {
                    self.target = None;
                }
            }
            Some(DocKey::Types(file)) => {
                if let Some(doc) = self.types.remove(&file) {
                    let original = doc.original.clone();
                    self.types.insert(file, TypesDoc { root: original.clone(), ..doc });
                    self.apply_types(file, original);
                    self.types.remove(&file);
                }
            }
            None => self.unloaded.retain(|f| Some(f) != entry.path.as_ref()),
        }
        self.selection.retain(|id| self.maps.contains_key(&id.node));
        self.undo.clear();
        self.redo.clear();
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
            self.adopt(key);
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
