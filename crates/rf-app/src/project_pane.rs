//! The Project pane: the project's files (opened from it, made or edited in
//! the editor), saving them, and exporting them as a RedM resource, as the
//! C# RedForge's Project pane does.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use rf_editor::{DocKey, Editor, ProjectEntry, redm};

use crate::settings::Settings;

const RESOURCES_FOLDER: &str = "RedM.ResourcesFolder";
const AUTHOR: &str = "RedM.Author";
/// The C#'s project settings: the .rfproj files opened, most recent first
/// (separated by `|`), the one open when quitting, and whether it opens again.
const RECENT: &str = "Project.Recent";
const LAST: &str = "Project.Last";
const REOPEN_LAST: &str = "Project.ReopenLast";

/// Files go in groups by what they are, as in the C#'s pane, with a colour
/// each; the last group takes everything else.
const GROUPS: [(&str, &[&str], egui::Color32); 6] = [
    ("Maps", &["ymap"], egui::Color32::from_rgb(110, 190, 90)),
    ("Archetypes", &["ytyp"], egui::Color32::from_rgb(230, 160, 60)),
    ("Models", &["ydr", "ydd", "yft"], egui::Color32::from_rgb(90, 160, 235)),
    ("Textures", &["ytd"], egui::Color32::from_rgb(190, 120, 220)),
    ("Collisions", &["ybn"], egui::Color32::from_rgb(225, 95, 90)),
    ("Other", &[], egui::Color32::GRAY),
];

fn group_of(name: &str) -> usize {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    GROUPS.iter().position(|g| g.1.contains(&ext.as_str())).unwrap_or(GROUPS.len() - 1)
}

/// Where a file is: its folder in the project's, or the whole folder when
/// it's somewhere else.
fn folder_text(root: Option<&Path>, path: Option<&Path>) -> String {
    let Some(path) = path else { return "not saved".into() };
    let dir = path.parent().unwrap_or(Path::new(""));
    match root.and_then(|r| dir.strip_prefix(r).ok()) {
        Some(rel) => rel.display().to_string(),
        None => dir.display().to_string(),
    }
}

/// A row's identity, for the selection.
fn entry_key(e: &ProjectEntry) -> String {
    match &e.path {
        Some(p) => p.display().to_string(),
        None => e.name.clone(),
    }
}

/// Something that waits for "discard unsaved changes?".
enum Pending {
    Open(PathBuf),
    Close,
}

#[derive(Default)]
pub struct ProjectPane {
    status: Option<String>,
    new_map_name: String,
    confirm: Option<Pending>,
    /// The export form, filled from the project when it opens.
    export: Option<ExportForm>,
    filter: String,
    collapsed: HashSet<usize>,
    /// The selected files ([`entry_key`]).
    selected: HashSet<String>,
    /// A ymap just picked in the list (by name hash), for the Hierarchy to show.
    pub picked_map: Option<u32>,
    /// Whether the last project was opened again at start.
    started: bool,
}

#[derive(Default, Clone)]
struct ExportForm {
    resource: String,
    folder: String,
    author: String,
    version: String,
    description: String,
}

impl ProjectPane {
    pub fn ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor, settings: &mut Settings) {
        if !std::mem::replace(&mut self.started, true) {
            self.reopen(editor, settings);
        }
        let title = match &editor.project {
            Some(p) => p.name(),
            None => "No project".into(),
        };
        let path = editor.project.as_ref().and_then(|p| p.file.as_ref()).map(|f| f.display().to_string());
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong()).on_hover_text(path.as_deref().unwrap_or("Not saved yet"));
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("New…")
                .on_hover_text("A new project, saved as an .rfproj; what's edited now goes in it")
                .clicked()
                && let Some(file) = rfd::FileDialog::new().add_filter("RedForge project", &["rfproj"]).save_file()
            {
                let file = with_extension(file, "rfproj");
                let made = editor.new_project(&file);
                if made.is_ok() {
                    remember(settings, &file);
                }
                self.report(made.map(|_| "Project made".into()));
            }
            if ui.button("Open…").clicked()
                && let Some(file) = rfd::FileDialog::new().add_filter("RedForge project", &["rfproj"]).pick_file()
            {
                if editor.has_unsaved() {
                    self.confirm = Some(Pending::Open(file));
                } else {
                    self.open(editor, settings, &file);
                }
            }
            if ui.button("Save all").on_hover_text("Ctrl+S").clicked() {
                self.save_all(editor, settings);
            }
            if ui.add_enabled(editor.project.is_some(), egui::Button::new("Close")).clicked() {
                if editor.has_unsaved() {
                    self.confirm = Some(Pending::Close);
                } else {
                    self.close(editor, settings);
                }
            }
            ui.menu_button("More", |ui| {
                let mut reopen = settings.get(REOPEN_LAST) != Some("false");
                if ui.checkbox(&mut reopen, "Reopen the last project at start").changed() {
                    settings.set(REOPEN_LAST, reopen.to_string());
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Add files…").clicked()
                && let Some(files) = rfd::FileDialog::new()
                    .add_filter("Game files", &["ymap", "ytyp", "ydr", "ydd", "yft", "ytd", "ybn", "ymf"])
                    .pick_files()
            {
                let problems = editor.add_project_files(&files);
                self.after_load(editor, problems);
            }
            if ui.button("Add folder…").on_hover_text("Its game files and its subfolders' are in the project").clicked()
                && let Some(folder) = rfd::FileDialog::new().pick_folder()
            {
                let problems = editor.add_project_folder(&folder);
                self.after_load(editor, problems);
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_map_name).hint_text("name").desired_width(140.0));
            if ui.button("New ymap").on_hover_text("An empty ymap to place things in").clicked() {
                match editor.new_map(&self.new_map_name) {
                    Ok(_) => {
                        self.status = Some(format!("Made {}.ymap: new things go in it", self.new_map_name.trim()));
                        self.new_map_name.clear();
                    }
                    Err(e) => self.status = Some(e),
                }
            }
        });
        self.confirm_ui(ui, editor, settings);
        ui.separator();

        let entries = editor.project_entries();
        if entries.is_empty() {
            ui.label(RichText::new("No files yet: edit the map, make a ymap, or add files.").weak());
            let recent = recent(settings);
            if editor.project.is_none() && !recent.is_empty() {
                ui.label(RichText::new("Recent projects").strong());
                for file in recent.iter().take(6) {
                    let name = file.file_stem().unwrap_or_default().to_string_lossy();
                    if ui.link(name).on_hover_text(file.display().to_string()).clicked() {
                        self.open(editor, settings, file);
                    }
                }
            }
        } else {
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Search files"));
        }
        let filter = self.filter.trim().to_lowercase();
        let root = editor.project.as_ref().and_then(|p| p.dir()).map(Path::to_path_buf);
        egui::ScrollArea::vertical().id_salt("project-files").max_height(ui.available_height() * 0.6).show(ui, |ui| {
            for (g, (name, _, colour)) in GROUPS.iter().enumerate() {
                let mut rows: Vec<&ProjectEntry> = entries
                    .iter()
                    .filter(|e| {
                        group_of(&e.name) == g && (filter.is_empty() || e.name.to_lowercase().contains(&filter))
                    })
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                rows.sort_by_key(|e| e.name.to_lowercase());
                // Collapsed groups show their header only, unless searching.
                let open = !self.collapsed.contains(&g) || !filter.is_empty();
                let header = ui.horizontal(|ui| {
                    ui.label(RichText::new(if open { "⏷" } else { "⏵" }).weak());
                    ui.label(RichText::new("■").color(*colour));
                    ui.label(RichText::new(*name).strong());
                    ui.label(RichText::new(rows.len().to_string()).weak());
                });
                let click = ui.interact(header.response.rect, ui.id().with(("project-group", g)), egui::Sense::click());
                if click.clicked() && !self.collapsed.remove(&g) {
                    self.collapsed.insert(g);
                }
                if open {
                    for e in rows {
                        self.entry_ui(ui, editor, e, root.as_deref());
                    }
                }
            }
        });
        ui.separator();
        self.export_ui(ui, editor, settings);
        if let Some(s) = &self.status {
            ui.separator();
            ui.label(RichText::new(s).small());
        }
    }

    fn entry_ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor, e: &ProjectEntry, root: Option<&Path>) {
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            if let Some(DocKey::Map(name)) = e.doc {
                let target = editor.target == Some(name);
                if ui.radio(target, "").on_hover_text("Place and paste into this ymap").clicked() {
                    editor.target = Some(name);
                }
            }
            let mut text = RichText::new(format!("{}{}", e.name, if e.dirty { " •" } else { "" }));
            if e.path.is_none() {
                text = text.italics();
            }
            let tip = match &e.path {
                Some(p) => p.display().to_string(),
                None => "Changed game file, not saved to disk yet".into(),
            };
            let key = entry_key(e);
            let row = ui.selectable_label(self.selected.contains(&key), text).on_hover_text(tip);
            let folder = folder_text(root, e.path.as_deref());
            if !folder.is_empty() {
                ui.add(egui::Label::new(RichText::new(folder).weak().small()).truncate());
            }
            if row.clicked() {
                if ui.input(|i| i.modifiers.command) {
                    if !self.selected.remove(&key) {
                        self.selected.insert(key);
                    }
                } else {
                    self.selected = HashSet::from([key]);
                }
                // A ymap picked shows in the Hierarchy (until the map's selection changes).
                if let Some(DocKey::Map(name)) = e.doc {
                    self.picked_map = Some(name);
                }
            }
            row.context_menu(|ui| {
                if let Some(doc) = e.doc {
                    if ui.button("Save").clicked() {
                        match editor.save_path(doc) {
                            Some(path) => self.report(editor.save_doc(doc, &path).map(|_| format!("Saved {}", e.name))),
                            None => self.save_as(editor, doc, &e.name),
                        }
                        ui.close();
                    }
                    if ui.button("Save as…").clicked() {
                        self.save_as(editor, doc, &e.name);
                        ui.close();
                    }
                }
                if ui.button("Remove from project").on_hover_text("The game's file is used again").clicked() {
                    editor.remove_from_project(e);
                    self.report(editor.save_project_file().map(|_| format!("Removed {}", e.name)));
                    ui.close();
                }
            });
        });
    }

    fn save_as(&mut self, editor: &mut Editor, doc: DocKey, name: &str) {
        let ext = Path::new(name).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let mut dialog = rfd::FileDialog::new().set_file_name(name).add_filter(&ext, &[&ext]);
        if let Some(dir) = editor.project.as_ref().and_then(|p| p.dir()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            self.report(editor.save_doc(doc, &path).map(|_| format!("Saved {name}")));
        }
    }

    fn open(&mut self, editor: &mut Editor, settings: &mut Settings, file: &Path) {
        match editor.open_project(file) {
            Ok(problems) => {
                remember(settings, file);
                self.after_load(editor, problems);
            }
            Err(e) => self.status = Some(e),
        }
        self.export = None;
        self.selected.clear();
    }

    fn close(&mut self, editor: &mut Editor, settings: &mut Settings) {
        editor.close_project();
        settings.set(LAST, "");
        self.export = None;
        self.selected.clear();
    }

    /// At start: the project `REDFORGE_PROJECT` names (for testing, not
    /// remembered), or the last one, as the C# does.
    fn reopen(&mut self, editor: &mut Editor, settings: &mut Settings) {
        if let Ok(file) = std::env::var("REDFORGE_PROJECT") {
            match editor.open_project(Path::new(&file)) {
                Ok(problems) => self.after_load(editor, problems),
                Err(e) => self.status = Some(e),
            }
            return;
        }
        let last = settings.get(LAST).map(PathBuf::from).filter(|f| f.is_file());
        if settings.get(REOPEN_LAST) != Some("false")
            && let Some(file) = last
        {
            self.open(editor, settings, &file);
        }
    }

    fn after_load(&mut self, editor: &mut Editor, problems: Vec<String>) {
        let _ = editor.save_project_file();
        self.status = Some(if problems.is_empty() {
            format!("{} files in the project", editor.project_entries().len())
        } else {
            problems.join("\n")
        });
    }

    fn report(&mut self, result: Result<String, String>) {
        self.status = Some(match result {
            Ok(s) => s,
            Err(e) => e,
        });
    }

    /// Saves every changed file (asking where the project goes if it has no
    /// file yet), then exports if the project says so.
    pub fn save_all(&mut self, editor: &mut Editor, settings: &mut Settings) {
        if editor.project.as_ref().is_none_or(|p| p.file.is_none()) && editor.has_unsaved() {
            let Some(file) = rfd::FileDialog::new()
                .set_title("Where the project goes (its files are saved next to it)")
                .add_filter("RedForge project", &["rfproj"])
                .save_file()
            else {
                return;
            };
            let file = with_extension(file, "rfproj");
            if let Err(e) = editor.new_project(&file) {
                self.status = Some(e);
                return;
            }
            remember(settings, &file);
        }
        match editor.save_all() {
            Ok(homeless) if homeless.is_empty() => {
                self.status = Some("Saved".into());
                let on_save =
                    editor.project.as_ref().is_some_and(|p| p.export.get("onSave").is_some_and(|v| v == "true"));
                if on_save {
                    self.export(editor, settings, true);
                }
            }
            Ok(homeless) => self.status = Some(format!("{} files have nowhere to go", homeless.len())),
            Err(e) => self.status = Some(e),
        }
    }

    fn confirm_ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor, settings: &mut Settings) {
        let Some(pending) = &self.confirm else { return };
        let what = match pending {
            Pending::Open(_) => "Open another project",
            Pending::Close => "Close the project",
        };
        let mut answer = None;
        egui::Modal::new(egui::Id::new("discard-changes")).show(ui.ctx(), |ui| {
            ui.heading(what);
            ui.label("Some files have unsaved changes. Discard them?");
            ui.horizontal(|ui| {
                if ui.button("Discard").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        match answer {
            Some(true) => match self.confirm.take() {
                Some(Pending::Open(file)) => self.open(editor, settings, &file),
                Some(Pending::Close) => self.close(editor, settings),
                None => {}
            },
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    /// Exporting as a RedM resource; the choices are kept in the project.
    fn export_ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor, settings: &mut Settings) {
        egui::CollapsingHeader::new("RedM resource").default_open(false).show(ui, |ui| {
            let Some(project) = &editor.project else {
                ui.label(RichText::new("Make or open a project first.").weak());
                return;
            };
            let form = self.export.get_or_insert_with(|| {
                let saved = &project.export;
                let get = |k: &str| saved.get(k).cloned();
                let folder = get("folder").unwrap_or_else(|| settings.get(RESOURCES_FOLDER).unwrap_or_default().into());
                let resource = get("resource").unwrap_or_else(|| redm::safe_name(&project.name()));
                let existing = redm::read_info(&Path::new(&folder).join(&resource));
                let info = |k: &str| get(k).or_else(|| existing.get(k).cloned());
                ExportForm {
                    author: info("author").unwrap_or_else(|| settings.get(AUTHOR).unwrap_or_default().into()),
                    version: info("version").unwrap_or_else(|| "1.0.0".into()),
                    description: info("description").unwrap_or_default(),
                    resource,
                    folder,
                }
            });
            egui::Grid::new("redm-export").num_columns(2).show(ui, |ui| {
                ui.label("Resource name");
                ui.text_edit_singleline(&mut form.resource);
                ui.end_row();
                ui.label("Resources folder");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut form.folder)
                        .on_hover_text("For example the server's resources/[maps]");
                    if ui.button("…").clicked()
                        && let Some(f) = rfd::FileDialog::new().pick_folder()
                    {
                        form.folder = f.display().to_string();
                    }
                });
                ui.end_row();
                ui.label("Author");
                ui.text_edit_singleline(&mut form.author);
                ui.end_row();
                ui.label("Version");
                ui.text_edit_singleline(&mut form.version);
                ui.end_row();
                ui.label("Description");
                ui.text_edit_singleline(&mut form.description);
                ui.end_row();
            });
            let mut on_save = project.export.get("onSave").is_some_and(|v| v == "true");
            let on_save_changed = ui.checkbox(&mut on_save, "Export when saving").changed();
            ui.label(
                RichText::new(
                    "The game files go to the stream folder. An existing fxmanifest.lua is kept: only author, version, \
                     description and what the map needs are updated in it.",
                )
                .weak()
                .small(),
            );
            let export = ui.button("Export").clicked();
            if on_save_changed && let Some(p) = &mut editor.project {
                p.export.insert("onSave".into(), on_save.to_string());
                let _ = editor.save_project_file();
            }
            if export {
                self.export(editor, settings, false);
            }
            ui.separator();
            if ui.button("Generate _manifest.ymf").on_hover_text("Which ytyps the project's ymaps need").clicked() {
                let path = editor
                    .project
                    .as_ref()
                    .and_then(|p| p.dir())
                    .map(|d| d.join("_manifest.ymf"))
                    .or_else(|| rfd::FileDialog::new().set_file_name("_manifest.ymf").save_file());
                if let Some(path) = path {
                    self.report(editor.write_manifest(&path).map(|_| format!("Wrote {}", path.display())));
                }
            }
        });
    }

    /// Exports the project's saved files (after saving them, unless this is
    /// the export that comes with saving).
    fn export(&mut self, editor: &mut Editor, settings: &mut Settings, after_save: bool) {
        let Some(form) = self.export.clone() else {
            if !after_save {
                self.status = Some("Fill in the RedM resource first".into());
            }
            return;
        };
        if !after_save && editor.has_unsaved() {
            self.save_all(editor, settings);
            if editor.has_unsaved() {
                return;
            }
        }
        if !Path::new(&form.folder).is_dir() {
            self.status = Some("Choose the folder to put the resource in".into());
            return;
        }
        let name = redm::safe_name(&form.resource);
        let resource = Path::new(&form.folder).join(&name);
        settings.set(RESOURCES_FOLDER, form.folder.clone());
        settings.set(AUTHOR, form.author.trim());
        let files: Vec<PathBuf> =
            editor.project_entries().into_iter().filter_map(|e| e.path).filter(|p| p.exists()).collect();
        let info: BTreeMap<String, String> = [
            ("author", form.author.trim()),
            ("version", form.version.trim()),
            ("description", form.description.trim()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        if let Some(p) = &mut editor.project {
            p.export.insert("resource".into(), name.clone());
            p.export.insert("folder".into(), form.folder.clone());
            p.export.extend(info.clone());
        }
        let _ = editor.save_project_file();
        self.status = Some(match redm::export(&resource, &files, &info) {
            Ok(skipped) if skipped.is_empty() => {
                format!("Exported {name}: add `ensure {name}` to server.cfg and restart the server")
            }
            Ok(skipped) => format!("Exported {name} (left out: {})", skipped.join(", ")),
            Err(e) => format!("The export failed: {e}"),
        });
    }
}

/// The recent projects that are still there.
fn recent(settings: &Settings) -> Vec<PathBuf> {
    settings
        .get(RECENT)
        .unwrap_or_default()
        .split('|')
        .filter(|f| !f.is_empty())
        .map(PathBuf::from)
        .filter(|f| f.is_file())
        .collect()
}

/// A project opened: first of the recent ones, and the last one.
fn remember(settings: &mut Settings, file: &Path) {
    let file = file.display().to_string();
    let mut list: Vec<String> = settings.get(RECENT).unwrap_or_default().split('|').map(str::to_owned).collect();
    list.retain(|f| !f.is_empty() && *f != file);
    list.insert(0, file.clone());
    list.truncate(10);
    settings.set(RECENT, list.join("|"));
    settings.set(LAST, file);
}

fn with_extension(mut file: PathBuf, ext: &str) -> PathBuf {
    if file.extension().is_none_or(|e| !e.eq_ignore_ascii_case(ext)) {
        file.set_extension(ext);
    }
    file
}
