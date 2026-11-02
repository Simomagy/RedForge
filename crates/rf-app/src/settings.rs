//! Settings in the C# RedForge's format: `RedForge.Settings.ini`, one sorted
//! `Name value` line per setting, kept in the user's config folder.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub const GAME_FOLDER: &str = "RDR2.GameFolder";

pub struct Settings {
    values: BTreeMap<String, String>,
    path: Option<PathBuf>,
}

impl Settings {
    pub fn load() -> Self {
        let path = dirs::config_dir().map(|d| d.join("RedForge").join("RedForge.Settings.ini"));
        let mut values = BTreeMap::new();
        if let Some(text) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            for line in text.lines() {
                let line = line.trim_end_matches('\r');
                match line.split_once(' ') {
                    Some((k, v)) => values.insert(k.to_owned(), v.to_owned()),
                    None if !line.is_empty() => values.insert(line.to_owned(), String::new()),
                    None => None,
                };
            }
        }
        Self { values, path }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.values.insert(key.to_owned(), value.into());
        self.save();
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let text: String = self.values.iter().map(|(k, v)| format!("{k} {v}\r\n")).collect();
        let result = std::fs::create_dir_all(path.parent().unwrap()).and_then(|_| std::fs::write(path, text));
        if let Err(e) = result {
            log::warn!("saving {}: {e}", path.display());
        }
    }
}
