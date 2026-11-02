//! Models to and from Sollumz XML, laid out as Sollumz has them: `x.ydr.xml`
//! with its textures as DDS files in the folder `x` next to it.

use std::fs;
use std::path::{Path, PathBuf};

use rf_formats::formats::ytd::TextureDictionary;
use rf_formats::formats::{self, Kind, XmlContext, dds, externals_folder};
use rf_formats::hash::HashIndex;
use rf_formats::rsc8::read_resource;

use crate::ModelData;

/// The folder RedForge exports models to for Blender, one folder per model
/// file: the add-on's `EXPORTS`, which it cleans up and reloads pictures from.
///
/// That's `%LOCALAPPDATA%\RedForge\Blender\edit`, as in the C#. Where there's
/// no `LOCALAPPDATA` (Linux) the add-on uses Python's temporary folder instead,
/// so this does too: `$TMPDIR` (or `$TEMP`, `$TMP`), else `/tmp`.
pub fn default_export_folder() -> PathBuf {
    let base = match std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => python_temp_dir(),
    };
    base.join("RedForge").join("Blender").join("edit")
}

/// Python's `tempfile.gettempdir()`: the first of these that is a folder
/// (Python also checks it can write there).
fn python_temp_dir() -> PathBuf {
    let from_env = ["TMPDIR", "TEMP", "TMP"].into_iter().filter_map(|v| std::env::var_os(v).filter(|v| !v.is_empty()));
    let fallback: Vec<PathBuf> = if cfg!(windows) {
        vec![std::env::temp_dir()]
    } else {
        ["/tmp", "/var/tmp", "/usr/tmp"].map(PathBuf::from).into()
    };
    from_env
        .map(PathBuf::from)
        .chain(fallback)
        .find(|p| p.is_dir())
        .map(|p| std::path::absolute(&p).unwrap_or(p))
        .unwrap_or_else(std::env::temp_dir)
}

/// Exports a model as Sollumz XML into `folder`, as the C#'s
/// `ExportPiecePacks` does for Blender: `<file>.xml`, and in `<stem>/` the
/// textures it embeds, then those of its texture dictionaries as DDS files.
/// Returns the XML's path. Textures that can't be written are only logged:
/// Blender can still edit the model.
pub fn export_model(file: &str, model: &ModelData, hashes: &HashIndex, folder: &Path) -> Result<PathBuf, String> {
    let textures = externals_folder(folder, file);
    fs::create_dir_all(&textures).map_err(|e| format!("{}: {e}", textures.display()))?;
    let cx = XmlContext { hashes, folder: Some(textures.clone()) };
    let xml = formats::to_xml(file, &model.data, &cx).map_err(|e| e.to_string())?;
    let path = folder.join(format!("{file}.xml"));
    fs::write(&path, xml).map_err(|e| format!("{}: {e}", path.display()))?;
    // Worst first, so the best one's texture is the one left (the C# reverses
    // the list for this); they also replace embedded textures of the same name.
    for (i, ytd) in model.texture_dictionaries.iter().enumerate().rev() {
        if let Err(e) = write_textures(ytd, hashes, &textures) {
            log::warn!("{file}: texture dictionary {}: {e}", i + 1);
        }
    }
    Ok(path)
}

/// Writes a texture dictionary's textures as `<name>.dds` files
/// (`ExportPiecePacks`' `exportAllDDS`).
fn write_textures(ytd: &[u8], hashes: &HashIndex, folder: &Path) -> Result<(), String> {
    let td = read_resource(ytd, |r, _| TextureDictionary::read(r, hashes)).map_err(|e| e.to_string())?;
    for t in td.textures.iter().flatten().flatten() {
        let b = &t.base;
        let (Some(name), Some(data)) = (b.name.as_deref(), &b.data) else { continue };
        // (a name that isn't a plain file name would write outside the folder)
        if name.is_empty() || name.contains(['/', '\\']) || name == ".." {
            continue;
        }
        let file = if name.to_lowercase().ends_with(".dds") { name.to_owned() } else { format!("{name}.dds") };
        match dds::write_dds(b.format, b.width, b.height, b.depth, b.mip_levels, data) {
            Ok(dds) => {
                if let Err(e) = fs::write(folder.join(&file), dds) {
                    log::warn!("{file}: {e}");
                }
            }
            Err(e) => log::warn!("{file}: {e}"),
        }
    }
    Ok(())
}

/// Converts XML that Sollumz exported back to a game file (as the C#'s
/// `ConvertFromXml`): the kind comes from the XML's name (`x.ydr.xml`,
/// `x.ytyp.rsc.xml`...), the textures from the folder `x` next to it.
pub fn convert_xml(xml: &Path, hashes: &HashIndex) -> Result<Vec<u8>, String> {
    let name = xml.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = name.to_lowercase();
    if matches!(Kind::of_xml_name(&lower), None | Some(Kind::Fxdb)) {
        return Err(format!("{name} can't be converted."));
    }
    let text = fs::read_to_string(xml).map_err(|e| format!("{}: {e}", xml.display()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    // x.ydr.xml → x.ydr, whose externals are in x/
    let game_file = name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem);
    let folder = externals_folder(xml.parent().unwrap_or(Path::new("")), game_file);
    let cx = XmlContext { hashes, folder: Some(folder) };
    formats::from_xml(&lower, text, &cx).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_formats::formats::dds::TextureFormat;

    /// A ytd of 4×4 BGRA textures, each filled with its byte.
    pub(crate) fn texture_dictionary(dir: &Path, textures: &[(&str, u8)]) -> Vec<u8> {
        let src = dir.join("src");
        fs::create_dir_all(src.join("t")).unwrap();
        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<RDR2TextureDictionary version=\"1\">\n <Textures>\n",
        );
        for (name, fill) in textures {
            let dds = dds::write_dds(TextureFormat::A8R8G8B8, 4, 4, 1, 1, &[*fill; 64]).unwrap();
            fs::write(src.join("t").join(format!("{name}.dds")), dds).unwrap();
            xml += &format!("  <Item>\n   <Name>{name}</Name>\n  </Item>\n");
        }
        xml += " </Textures>\n</RDR2TextureDictionary>";
        fs::write(src.join("t.ytd.xml"), xml).unwrap();
        let data = convert_xml(&src.join("t.ytd.xml"), &HashIndex::new()).unwrap();
        fs::remove_dir_all(&src).unwrap();
        data
    }

    fn pixel(path: &Path) -> u8 {
        let dds = dds::read_dds(&fs::read(path).unwrap()).unwrap();
        dds.data[0]
    }

    #[test]
    fn exports_and_converts_back() {
        let dir = std::env::temp_dir().join(format!("rf-bridge-model-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let hashes = HashIndex::new();
        // The "model" is a ytd itself: it embeds "shared" and "own".
        let model = texture_dictionary(&dir, &[("shared", 1), ("own", 2)]);
        let best = texture_dictionary(&dir, &[("shared", 3), ("best", 4)]);
        let worse = texture_dictionary(&dir, &[("shared", 5), ("best", 6), ("worse", 7)]);
        let data = ModelData { data: model.clone(), texture_dictionaries: vec![best, worse, b"junk".to_vec()] };

        let folder = dir.join("edit").join("box.ytd");
        let xml = export_model("box.ytd", &data, &hashes, &folder).unwrap();
        assert_eq!(xml, folder.join("box.ytd.xml"));
        let textures = folder.join("box");
        // The best dictionary wins, over the model's own texture too.
        assert_eq!(pixel(&textures.join("shared.dds")), 3);
        assert_eq!(pixel(&textures.join("best.dds")), 4);
        assert_eq!(pixel(&textures.join("worse.dds")), 7);
        assert_eq!(pixel(&textures.join("own.dds")), 2);

        // Back from the export: its textures come from box/ next to it.
        let back = convert_xml(&xml, &hashes).unwrap();
        let read = |d: &[u8]| read_resource(d, |r, _| TextureDictionary::read(r, &hashes)).unwrap();
        let names = |td: &TextureDictionary| -> Vec<String> {
            td.textures.iter().flatten().flatten().filter_map(|t| t.base.name.clone()).collect()
        };
        assert_eq!(names(&read(&back)), names(&read(&model)));
        let shared = read(&back).textures.unwrap().into_iter().flatten().find(|t| t.name() == Some("shared")).unwrap();
        assert_eq!(shared.base.data.unwrap()[0], 3);

        assert!(export_model("box.ydr", &data, &hashes, &folder).is_err());
        assert!(convert_xml(&folder.join("nothing.ydr.xml"), &hashes).is_err());
        assert_eq!(convert_xml(&folder.join("box.txt"), &hashes), Err("box.txt can't be converted.".into()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn export_folder_is_the_add_ons() {
        let folder = default_export_folder();
        assert!(folder.ends_with(Path::new("RedForge").join("Blender").join("edit")), "{}", folder.display());
        assert!(folder.is_absolute());
    }
}
