//! Peds and animals, put together from their parts as the C# RedForge shows
//! them (`RDR2Peds.cs`): the ped list from `PED_METADATA_FILE`, a ped's metaped
//! (`<name>.ymt`) and its first outfit's parts, the drawables and textures they
//! name through the asset owner map, and the skeleton of the ped's fragment.
//!
//! As in the C#, an outfit made of shop items (the multiplayer peds) is shown
//! as its bare body: the shop items of the body categories (head, eyes, teeth,
//! hair, upper and lower body), without clothes. Other outfits are their
//! explicit assets, each a drawable with its textures, tint palette and tints.
//!
//! Not done: other outfits and sub-outfits (the C# can pick them; it starts
//! with the first outfit too), the outfit's `assets` (picked by tags, which
//! the C# skips as well), and cloth (ylds), which the C# simulates.

use std::collections::HashMap;
use std::sync::OnceLock;

use rf_formats::formats::XmlContext;
use rf_formats::formats::databag::{DataBag, ValueType};
use rf_formats::formats::ydr::{self, DrawableBase, DrawableDictionary, Skeleton};
use rf_formats::formats::yft;
use rf_formats::formats::ytd::{TextureBase, TextureDictionary};
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};
use rf_formats::rsc8::read_resource;
use rf_formats::{Error, Result};

use crate::GameData;
use crate::files::{Doc, bags, load_doc, string};

/// A ped's model info (`CPedModelInfo__InitData`): what's used here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PedInitData {
    /// Lowercase, as the C# keys them.
    pub name: String,
    /// The fragment (yft) with the ped's skeleton, by the hash of its lowercase name.
    pub frag_name: u32,
    pub clip_dictionary: u32,
}

/// A part of a ped (`MetaPedDefExplicitAsset`): a drawable, and the textures
/// that replace its own (0 for none).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PedAsset {
    pub drawable: u32,
    pub albedo: u32,
    pub normal: u32,
    pub material: u32,
    pub palette: u32,
    pub tints: [u8; 3],
}

/// What a ped is made of.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PedDef {
    pub name: String,
    /// The fragment whose skeleton the ped has.
    pub frag: Option<FileRef>,
    pub assets: Vec<PedAsset>,
}

/// The peds the game knows (`RDR2Peds`).
pub struct Peds {
    init_datas: HashMap<String, PedInitData>,
    /// Sorted.
    names: Vec<String>,
    /// `shop_items.ymt`, read when a ped first needs it.
    shop_items: OnceLock<Option<Doc>>,
}

/// The shop item categories a multiplayer ped's bare body is made of.
const BODY_CATEGORIES: [&str; 6] = ["heads", "eyes", "teeth", "hair", "bodies_upper", "bodies_lower"];

/// A name or hash field as a hash: a hash field's value, a string's joaat as the
/// C#'s `new JenkHash(string)` (0 for an empty one).
fn asset_hash(bag: &DataBag, field: &str) -> u32 {
    let key = joaat(field);
    match bag.class.field(key).map(|f| f.data_type) {
        Some(ValueType::Hash) => bag.hash(key),
        _ => bag.string(key).filter(|s| !s.is_empty()).map_or(0, joaat),
    }
}

fn read_asset(b: &DataBag) -> PedAsset {
    PedAsset {
        drawable: asset_hash(b, "drawable"),
        albedo: asset_hash(b, "albedo"),
        normal: asset_hash(b, "normal"),
        material: asset_hash(b, "material"),
        palette: asset_hash(b, "palette"),
        tints: [b.u8("tint0"), b.u8("tint1"), b.u8("tint2")],
    }
}

impl Peds {
    /// Reads the ped list from the game's `PED_METADATA_FILE`s.
    pub fn load(fs: &Rpf8FileSystem, hashes: &HashIndex, gd: &GameData) -> Self {
        let mut init_datas = HashMap::new();
        for &f in gd.enabled_data_files.get("PED_METADATA_FILE").into_iter().flatten() {
            let Some(doc) = load_doc(fs, hashes, f) else { continue };
            let schema = doc.schema.as_ref();
            for d in bags(&doc.bag, joaat("InitDatas")) {
                let text = |field: &str| string(d, joaat(field), schema, hashes).unwrap_or_default();
                let name = text("Name").to_lowercase();
                if name.is_empty() {
                    continue;
                }
                let lower_hash = |field: &str| joaat(&text(field).to_lowercase());
                let data = PedInitData {
                    frag_name: lower_hash("FragName"),
                    clip_dictionary: lower_hash("ClipDictionaryName"),
                    name: name.clone(),
                };
                init_datas.insert(name, data);
            }
        }
        let mut names: Vec<String> = init_datas.keys().cloned().collect();
        names.sort();
        Self { init_datas, names, shop_items: OnceLock::new() }
    }

    /// The peds' names, lowercase and sorted.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn init_data(&self, name: &str) -> Option<&PedInitData> {
        self.init_datas.get(&name.to_lowercase())
    }

    /// What a ped is made of, with its first outfit (the C#'s default).
    pub fn definition(&self, fs: &Rpf8FileSystem, hashes: &HashIndex, gd: &GameData, name: &str) -> Option<PedDef> {
        let init = self.init_data(name)?;
        let doc = load_doc(fs, hashes, gd.stream_entry("ymt", joaat(&init.name))?)?;
        let mut def =
            PedDef { name: init.name.clone(), frag: gd.stream_entry("yft", init.frag_name), assets: Vec::new() };
        let Some(outfit) = bags(&doc.bag, joaat("outfits")).next() else { return Some(def) };
        let kind = string(&doc.bag, joaat("type"), doc.schema.as_ref(), hashes).unwrap_or_default();
        let body = self.body_shop_items(fs, hashes, gd, outfit, &kind);
        def.assets =
            if body.is_empty() { bags(outfit, joaat("explicitAssets")).map(read_asset).collect() } else { body };
        Some(def)
    }

    /// `LoadBodyShopItems`: the parts of an outfit's shop items of the body
    /// categories, or none when it has no such items.
    fn body_shop_items(
        &self,
        fs: &Rpf8FileSystem,
        hashes: &HashIndex,
        gd: &GameData,
        outfit: &DataBag,
        kind: &str,
    ) -> Vec<PedAsset> {
        let entries: Vec<u32> = bags(outfit, joaat("shopItems")).map(|e| asset_hash(e, "itemName")).collect();
        if entries.is_empty() {
            return Vec::new();
        }
        let Some(doc) = self.shop_items(fs, hashes, gd) else { return Vec::new() };
        let lists: [&str; 2] = match kind {
            "MPT_FEMALE" => ["shopItemFemaleMp", "shopItemFemaleSp"],
            "MPT_ANIMAL" => ["shopItemAnimalMp", "shopItemAnimalSp"],
            _ => ["shopItemMaleMp", "shopItemMaleSp"],
        };
        let categories: Vec<u32> = BODY_CATEGORIES.iter().map(|c| joaat(c)).collect();
        let mut out = Vec::new();
        for name in entries {
            let item = lists.iter().find_map(|l| bags(&doc.bag, joaat(l)).find(|i| asset_hash(i, "name") == name));
            let Some(item) = item else { continue };
            if !categories.contains(&asset_hash(item, "shopCategory")) {
                continue;
            }
            out.extend(bags(item, joaat("assets")).map(read_asset));
        }
        out
    }

    fn shop_items(&self, fs: &Rpf8FileSystem, hashes: &HashIndex, gd: &GameData) -> Option<&Doc> {
        self.shop_items
            .get_or_init(|| {
                let f = *gd.enabled_data_files.get("METAPED_SHOP_ITEMS")?.last()?;
                load_doc(fs, hashes, f)
            })
            .as_ref()
    }
}

/// Where a ped texture is (`GetTexture`'s lookups).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssetTexture {
    /// The dictionary with it; `None` to look in the graphics dictionary.
    pub ytd: Option<FileRef>,
    /// Its index there, for when its name isn't found.
    pub index: u16,
    /// A dictionary with an HD version, by name.
    pub hd: Option<FileRef>,
}

impl GameData {
    /// `GetDrawable`'s lookup: the drawable dictionary (ydd) with a ped part's
    /// drawable, its owner in the asset owner map or the ydd named like it.
    pub fn asset_drawable_dictionary(&self, drawable: u32) -> Option<FileRef> {
        let owner = self.asset_owner_map.get(&drawable).map_or(0, |a| a.owner);
        self.stream_entry("ydd", if owner != 0 { owner } else { drawable })
    }

    /// `GetTexture`'s lookup: the texture dictionaries with a ped texture.
    pub fn asset_texture(&self, texture: u32) -> AssetTexture {
        let a = self.asset_owner_map.get(&texture).copied().unwrap_or_default();
        let owner = [a.owner, texture, a.owner_hd].into_iter().find(|&h| h != 0).unwrap_or(0);
        AssetTexture {
            ytd: self.stream_entry("ytd", owner),
            index: a.index1,
            hd: (a.owner_hd != 0).then(|| self.stream_entry("ytd", a.owner_hd)).flatten(),
        }
    }
}

/// A ped part read from the archives.
pub struct PedPart {
    /// The drawable's name, or its hash as `0x...` when unknown.
    pub name: String,
    pub drawable: DrawableBase,
    /// The textures replacing the drawable's diffuse, normal and material maps.
    pub textures: [Option<TextureBase>; 3],
    /// The tint palette, and the columns of it the diffuse's channels pick.
    pub palette: Option<TextureBase>,
    pub tints: [u8; 3],
}

/// A ped read from the archives: its fragment's skeleton and its parts.
pub struct LoadedPed {
    pub name: String,
    pub skeleton: Option<Skeleton>,
    pub parts: Vec<PedPart>,
    /// Parts that couldn't be found or read.
    pub missing: Vec<String>,
}

/// Reads a ped's skeleton, drawables and textures (`RDR2Ped.BuildPiece`'s file
/// loads).
pub fn load_ped(fs: &Rpf8FileSystem, hashes: &HashIndex, gd: &GameData, def: &PedDef) -> Result<LoadedPed> {
    let cx = XmlContext { hashes, folder: None };
    let skeleton = match def.frag {
        Some(f) => yft::read(&fs.extract_loose(f)?, &cx)?.drawable.and_then(|d| d.base.skeleton),
        None => None,
    };
    let mut ped = LoadedPed { name: def.name.clone(), skeleton, parts: Vec::new(), missing: Vec::new() };
    let mut ydds: HashMap<FileRef, Option<DrawableDictionary>> = HashMap::new();
    let mut ytds: HashMap<FileRef, Option<TextureDictionary>> = HashMap::new();
    let graphics = gd.map_lookup(fs, "platform:/textures/graphics").and_then(|e| e.file());
    for a in &def.assets {
        let name = hashes.name(a.drawable);
        let Some(ydd) = gd.asset_drawable_dictionary(a.drawable) else {
            ped.missing.push(name);
            continue;
        };
        let dict = ydds.entry(ydd).or_insert_with(|| read_cached(fs, ydd, |d| ydr::read_dictionary(d, &cx)));
        let found = dict.as_ref().and_then(|d| {
            let i = d.hashes.as_ref()?.iter().position(|&h| h == a.drawable)?;
            Some(d.drawables.as_ref()?.get(i)?.as_ref()?.base.clone())
        });
        let Some(drawable) = found else {
            ped.missing.push(name);
            continue;
        };
        let mut texture = |hash: u32| -> Option<TextureBase> {
            if hash == 0 {
                return None;
            }
            let at = gd.asset_texture(hash);
            let mut tex =
                at.ytd.or(graphics).and_then(|f| find_texture(fs, hashes, &mut ytds, f, hash, Some(at.index)));
            if let Some(hd) = at.hd {
                let by_name = tex.as_ref().and_then(|t| t.name.as_deref()).map_or(hash, |n| joaat(&n.to_lowercase()));
                if let Some(t) = find_texture(fs, hashes, &mut ytds, hd, by_name, None) {
                    tex = Some(t);
                }
            }
            tex
        };
        let textures = [texture(a.albedo), texture(a.normal), texture(a.material)];
        let palette = texture(a.palette);
        ped.parts.push(PedPart { name, drawable, textures, palette, tints: a.tints });
    }
    Ok(ped)
}

/// Reads a file once, logging (and remembering) a failure.
fn read_cached<T>(fs: &Rpf8FileSystem, f: FileRef, read: impl FnOnce(&[u8]) -> Result<T>) -> Option<T> {
    match fs.extract_loose(f).map_err(Error::from).and_then(|d| read(&d)) {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!("{}: {e}", fs.path(f));
            None
        }
    }
}

/// A texture of a dictionary by the hash of its name, or else by index.
fn find_texture(
    fs: &Rpf8FileSystem,
    hashes: &HashIndex,
    cache: &mut HashMap<FileRef, Option<TextureDictionary>>,
    f: FileRef,
    hash: u32,
    index: Option<u16>,
) -> Option<TextureBase> {
    let dict = cache
        .entry(f)
        .or_insert_with(|| read_cached(fs, f, |d| read_resource(d, |r, _| TextureDictionary::read(r, hashes))))
        .as_ref()?;
    let textures = dict.textures.as_ref()?;
    let by_name =
        textures.iter().flatten().find(|t| t.base.name.as_deref().is_some_and(|n| joaat(&n.to_lowercase()) == hash));
    let t = by_name.or_else(|| textures.get(index? as usize)?.as_ref())?;
    Some(t.base.clone())
}
