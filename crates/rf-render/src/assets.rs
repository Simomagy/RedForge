//! Loading models and their textures out of the game's archives.

use std::collections::HashMap;
use std::sync::Arc;

use rf_formats::formats::XmlContext;
use rf_formats::formats::ydr::{self, DrawableBase};
use rf_formats::formats::yft;
use rf_formats::formats::ytd::TextureDictionary;
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};
use rf_formats::rsc8::read_resource;

use crate::GpuContext;
use crate::model::TextureSource;
use crate::texture::GpuTexture;

/// The drawables of a model file: one for a ydr or yft, several for a ydd.
pub struct ModelFile {
    pub path: String,
    pub drawables: Vec<(String, DrawableBase)>,
}

/// Index of the game's files by name, for finding related files.
pub struct AssetIndex {
    /// Lowercase file name → the files with it.
    by_name: HashMap<String, Vec<FileRef>>,
}

impl AssetIndex {
    pub fn new(fs: &Rpf8FileSystem) -> Self {
        let mut by_name: HashMap<String, Vec<FileRef>> = HashMap::new();
        for (path, f) in fs.files() {
            let name = path.rsplit('/').next().unwrap_or(path);
            by_name.entry(name.to_owned()).or_default().push(f);
        }
        Self { by_name }
    }

    pub fn by_name(&self, name: &str) -> &[FileRef] {
        self.by_name.get(&name.to_lowercase()).map(Vec::as_slice).unwrap_or(&[])
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn stem(path: &str) -> &str {
    let name = file_name(path);
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// Reads a model file (ydr, ydd or yft) from loose-file bytes.
pub fn read_model(path: &str, loose: &[u8], hashes: &HashIndex) -> rf_formats::Result<ModelFile> {
    let cx = XmlContext { hashes, folder: None };
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    let mut drawables = Vec::new();
    match ext.as_str() {
        "ydr" => drawables.push((stem(path).to_owned(), ydr::read(loose, &cx)?.base)),
        "ydd" => {
            let dict = ydr::read_dictionary(loose, &cx)?;
            let hashes_list = dict.hashes.clone().unwrap_or_default();
            for (i, d) in dict.drawables.into_iter().flatten().enumerate() {
                if let Some(d) = d {
                    let name =
                        hashes_list.get(i).map(|h| hashes.name(*h)).unwrap_or_else(|| format!("{}[{i}]", stem(path)));
                    drawables.push((name, d.base));
                }
            }
        }
        "yft" => {
            // The main drawable first, then the others (damaged parts, physics
            // groups, cloth); a few fragments' main drawable is a placeholder.
            let frag = yft::read(loose, &cx)?;
            let name = stem(path);
            for (i, d) in frag.drawables().enumerate() {
                let label = if i == 0 { name.to_owned() } else { format!("{name} [{i}]") };
                drawables.push((label, d.base.clone()));
            }
        }
        _ => return Err(rf_formats::Error::Unsupported(format!("{path}: not a model file"))),
    }
    Ok(ModelFile { path: path.to_owned(), drawables })
}

/// Reads a texture dictionary from loose-file bytes.
pub fn read_texture_dictionary(loose: &[u8], hashes: &HashIndex) -> rf_formats::Result<TextureDictionary> {
    read_resource(loose, |r, _| TextureDictionary::read(r, hashes))
}

/// The texture dictionaries that go with a model, best first: its `+hidr` and
/// `+hi` versions and its own, in the model's folder, then anywhere.
pub fn related_dictionaries(fs: &Rpf8FileSystem, index: &AssetIndex, model_path: &str) -> Vec<FileRef> {
    let base = stem(model_path).to_lowercase();
    let folder = model_path.rsplit_once('/').map(|(f, _)| f.to_lowercase()).unwrap_or_default();
    let mut out: Vec<FileRef> = Vec::new();
    for suffix in ["+hidr", "+hi", ""] {
        let name = format!("{base}{suffix}.ytd");
        let mut files = index.by_name(&name).to_vec();
        // The model's own folder first.
        files.sort_by_key(|f| !fs.path(*f).to_lowercase().starts_with(&folder));
        for f in files.into_iter().take(1) {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

/// Textures from a list of texture dictionaries, uploaded on first use.
pub struct DictionaryTextures {
    ctx: GpuContext,
    dictionaries: Vec<TextureDictionary>,
    cache: HashMap<String, Option<Arc<GpuTexture>>>,
}

impl DictionaryTextures {
    pub fn new(ctx: GpuContext, dictionaries: Vec<TextureDictionary>) -> Self {
        Self { ctx, dictionaries, cache: HashMap::new() }
    }

    /// Loads the dictionaries related to a model.
    pub fn for_model(
        ctx: GpuContext,
        fs: &Rpf8FileSystem,
        index: &AssetIndex,
        hashes: &HashIndex,
        model_path: &str,
    ) -> Self {
        let mut dicts = Vec::new();
        for f in related_dictionaries(fs, index, model_path) {
            match fs.extract_loose(f).map_err(rf_formats::Error::from).and_then(|d| read_texture_dictionary(&d, hashes))
            {
                Ok(d) => dicts.push(d),
                Err(e) => log::warn!("{}: {e}", fs.path(f)),
            }
        }
        Self::new(ctx, dicts)
    }

    pub fn dictionaries(&self) -> &[TextureDictionary] {
        &self.dictionaries
    }
}

impl TextureSource for DictionaryTextures {
    fn texture(&mut self, name: &str) -> Option<Arc<GpuTexture>> {
        let key = name.to_lowercase();
        if let Some(t) = self.cache.get(&key) {
            return t.clone();
        }
        let found = self.dictionaries.iter().find_map(|d| {
            d.textures
                .as_ref()?
                .iter()
                .flatten()
                .find(|t| t.base.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
        });
        let tex = found.and_then(|t| match GpuTexture::upload(&self.ctx, &t.base) {
            Ok(t) => Some(Arc::new(t)),
            Err(e) => {
                log::warn!("{e}");
                None
            }
        });
        self.cache.insert(key, tex.clone());
        tex
    }
}
