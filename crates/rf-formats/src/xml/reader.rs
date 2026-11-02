//! Reads XML the way RedForge's C# `XmlMetaNodeReader` does: leniently, with a
//! default for anything missing or unparsable.

use std::path::{Path, PathBuf};

use roxmltree::Node;

use super::number::parse_f32;
use crate::hash::joaat;

#[derive(Debug, thiserror::Error)]
pub enum XmlError {
    #[error("XML parse error: {0}")]
    Parse(#[from] roxmltree::Error),
    #[error("{0}")]
    Invalid(String),
}

/// A parsed XML document.
pub struct XmlDocument {
    text: String,
}

impl XmlDocument {
    pub fn new(text: impl Into<String>) -> Self {
        let mut text: String = text.into();
        if text.starts_with('\u{feff}') {
            text.remove(0);
        }
        Self { text }
    }

    /// Parses and calls `f` with the root element.
    pub fn with_root<T>(&self, externals: Option<&Path>, f: impl FnOnce(XmlNode<'_, '_>) -> T) -> Result<T, XmlError> {
        let opt = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
        let doc = roxmltree::Document::parse_with_options(&self.text, opt)?;
        let ctx = ReadContext { externals: externals.map(Path::to_owned) };
        Ok(f(XmlNode { node: doc.root_element(), ctx: &ctx }))
    }
}

pub struct ReadContext {
    externals: Option<PathBuf>,
}

/// An element, and the reading methods of `XmlMetaNodeReader` relative to it.
#[derive(Clone, Copy)]
pub struct XmlNode<'a, 'input> {
    pub node: Node<'a, 'input>,
    ctx: &'a ReadContext,
}

fn inner_text(n: Node) -> String {
    n.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect()
}

fn parse_uint(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x") {
        return u64::from_str_radix(hex, 16).ok();
    }
    let t = s.trim();
    t.strip_prefix('+').unwrap_or(t).parse().ok()
}

fn parse_int(s: &str) -> Option<i64> {
    let t = s.trim();
    t.strip_prefix('+').unwrap_or(t).parse().ok()
}

fn parse_bool(s: &str) -> Option<bool> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("true") {
        Some(true)
    } else if t.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

/// `JenkHash.Parse`: `0x` and 8 hex digits, a made-up `<name>_0x<hex>` collision
/// name (in any case), or a name to hash.
pub fn parse_hash(text: &str) -> u32 {
    let b = text.as_bytes();
    if b.len() == 10
        && text[..2].eq_ignore_ascii_case("0x")
        && let Ok(h) = u32::from_str_radix(&text[2..], 16)
    {
        return h;
    }
    if b.len() >= 11 {
        let i = b.len() - 11;
        if b[i] == b'_'
            && text[i + 1..i + 3].eq_ignore_ascii_case("0x")
            && let Ok(h) = u32::from_str_radix(&text[i + 3..], 16)
        {
            return h;
        }
    }
    joaat(text)
}

fn split_values(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c.is_whitespace()).filter(|s| !s.is_empty())
}

impl<'a, 'input> XmlNode<'a, 'input> {
    pub fn name(&self) -> &str {
        self.node.tag_name().name()
    }

    /// `SelectSingleNode(id)`: the first child element with that name.
    pub fn child(&self, id: &str) -> Option<XmlNode<'a, 'input>> {
        self.node
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == id)
            .map(|node| XmlNode { node, ctx: self.ctx })
    }

    /// Child elements with that name.
    pub fn children(&self, id: &'a str) -> impl Iterator<Item = XmlNode<'a, 'input>> + 'a {
        let ctx = self.ctx;
        self.node
            .children()
            .filter(move |c| c.is_element() && c.tag_name().name() == id)
            .map(move |node| XmlNode { node, ctx })
    }

    pub fn attr(&self, name: &str) -> Option<&'a str> {
        self.node.attribute(name)
    }

    pub fn has_attributes(&self) -> bool {
        self.node.attributes().len() > 0
    }

    pub fn text(&self) -> String {
        inner_text(self.node)
    }

    /// The text a scalar id refers to: an attribute of this node for `@name`,
    /// otherwise the `attribute` of the child element `id`.
    fn value_of(&self, id: &str, attribute: &str) -> Option<&'a str> {
        match id.strip_prefix('@') {
            Some(a) => self.attr(a),
            None => self.child(id)?.attr(attribute),
        }
    }

    pub fn bool(&self, id: &str, def: bool) -> bool {
        self.value_of(id, "value").and_then(parse_bool).unwrap_or(def)
    }
    pub fn u32(&self, id: &str, def: u32) -> u32 {
        self.value_of(id, "value").and_then(parse_uint).map(|v| v as u32).unwrap_or(def)
    }
    pub fn u64(&self, id: &str, def: u64) -> u64 {
        self.value_of(id, "value").and_then(parse_uint).unwrap_or(def)
    }
    pub fn i32(&self, id: &str, def: i32) -> i32 {
        self.value_of(id, "value").and_then(parse_int).map(|v| v as i32).unwrap_or(def)
    }
    pub fn i64(&self, id: &str, def: i64) -> i64 {
        let v = self.value_of(id, "value");
        match v {
            Some(s) if s.starts_with("0x") => u64::from_str_radix(&s[2..], 16).map(|v| v as i64).unwrap_or(def),
            Some(s) => parse_int(s).unwrap_or(def),
            None => def,
        }
    }
    /// `ReadByte` = `(byte)ReadUInt32`.
    pub fn u8(&self, id: &str, def: u8) -> u8 {
        self.u32(id, def as u32) as u8
    }
    pub fn u16(&self, id: &str, def: u16) -> u16 {
        self.u32(id, def as u32) as u16
    }
    pub fn i8(&self, id: &str, def: i8) -> i8 {
        self.i32(id, def as i32) as i8
    }
    pub fn i16(&self, id: &str, def: i16) -> i16 {
        self.i32(id, def as i32) as i16
    }
    pub fn f32(&self, id: &str, def: f32) -> f32 {
        self.value_of(id, "value").and_then(parse_f32).unwrap_or(def)
    }
    pub fn f64(&self, id: &str, def: f64) -> f64 {
        self.value_of(id, "value").and_then(|s| s.trim().parse().ok()).unwrap_or(def)
    }

    /// `ReadString`: an attribute for `@name`, otherwise the child's inner text.
    pub fn string(&self, id: &str) -> Option<String> {
        match id.strip_prefix('@') {
            Some(a) => self.attr(a).map(str::to_owned),
            None => self.child(id).map(|c| c.text()),
        }
    }

    /// `ReadJenkHash`.
    pub fn hash(&self, id: &str) -> u32 {
        self.string(id).map(|s| parse_hash(&s)).unwrap_or(0)
    }

    fn float_attr(n: Option<XmlNode>, a: &str, def: f32) -> f32 {
        n.and_then(|n| n.attr(a)).and_then(parse_f32).unwrap_or(def)
    }
    fn vec_of<const N: usize>(n: Option<XmlNode>, names: [&str; N]) -> [f32; N] {
        names.map(|a| Self::float_attr(n, a, 0.0))
    }
    pub fn vec2(&self, id: &str) -> [f32; 2] {
        Self::vec_of(self.child(id), ["x", "y"])
    }
    pub fn vec3(&self, id: &str) -> [f32; 3] {
        Self::vec_of(self.child(id), ["x", "y", "z"])
    }
    pub fn vec4(&self, id: &str) -> [f32; 4] {
        Self::vec_of(self.child(id), ["x", "y", "z", "w"])
    }
    pub fn colourf(&self, id: &str) -> [f32; 4] {
        Self::vec_of(self.child(id), ["r", "g", "b", "a"])
    }
    pub fn colour(&self, id: &str) -> [u8; 4] {
        let c = self.child(id);
        ["r", "g", "b", "a"].map(|a| c.and_then(|c| c.attr(a)).and_then(parse_uint).unwrap_or(0) as u8)
    }

    /// `Matrix3x4`/`Matrix4x4`: `N` floats, or `None` if there aren't exactly `N`.
    pub fn matrix<const N: usize>(&self, id: &str) -> Option<[f32; N]> {
        let v = self.f32_array(id)?;
        v.try_into().ok()
    }

    /// The whitespace-separated values of a child, `None` when absent or empty
    /// (the C# "Nullable" array readers).
    fn raw_values<T>(&self, id: &str, mut parse: impl FnMut(&str) -> T) -> Option<Vec<T>> {
        let c = self.child(id)?;
        let text = c.text();
        let v: Vec<T> = split_values(&text).map(&mut parse).collect();
        (!v.is_empty()).then_some(v)
    }

    pub fn f32_array(&self, id: &str) -> Option<Vec<f32>> {
        self.raw_values(id, |s| parse_f32(s).unwrap_or(0.0))
    }
    pub fn f64_array(&self, id: &str) -> Option<Vec<f64>> {
        self.raw_values(id, |s| s.parse().unwrap_or(0.0))
    }
    pub fn u16_array(&self, id: &str) -> Option<Vec<u16>> {
        self.raw_values(id, |s| s.parse().unwrap_or(0))
    }
    pub fn i16_array(&self, id: &str) -> Option<Vec<i16>> {
        self.raw_values(id, |s| s.parse().unwrap_or(0))
    }
    pub fn u32_array(&self, id: &str) -> Option<Vec<u32>> {
        self.raw_values(id, |s| parse_uint(s).unwrap_or(0) as u32)
    }
    pub fn i32_array(&self, id: &str) -> Option<Vec<i32>> {
        self.raw_values(id, |s| s.parse().unwrap_or(0))
    }
    pub fn u64_array(&self, id: &str) -> Option<Vec<u64>> {
        self.raw_values(id, |s| parse_uint(s).unwrap_or(0))
    }
    pub fn i64_array(&self, id: &str) -> Option<Vec<i64>> {
        self.raw_values(id, |s| match s.strip_prefix("0x") {
            Some(h) => u64::from_str_radix(h, 16).unwrap_or(0) as i64,
            None => s.parse().unwrap_or(0),
        })
    }
    pub fn i8_array(&self, id: &str) -> Option<Vec<i8>> {
        self.raw_values(id, |s| s.parse().unwrap_or(0))
    }
    /// Bytes in the base given by the child's `base` attribute, 10 by default.
    pub fn byte_array(&self, id: &str) -> Option<Vec<u8>> {
        let base = self.child(id).and_then(|c| c.attr("base")).and_then(|b| b.parse().ok()).unwrap_or(10);
        self.raw_values(id, |s| u8::from_str_radix(s, base).unwrap_or(0))
    }
    pub fn bool_array(&self, id: &str) -> Option<Vec<bool>> {
        let c = self.child(id)?;
        let items: Vec<bool> =
            c.children("Item").map(|i| i.attr("value").and_then(parse_bool).unwrap_or(false)).collect();
        if !items.is_empty() {
            return Some(items);
        }
        self.raw_values(id, |s| parse_bool(s).unwrap_or(false))
    }

    /// `Xml.GetStringItemArray`: the text of each `Item` (or `item`) child,
    /// `None` when absent or empty.
    pub fn string_array(&self, id: &str) -> Option<Vec<String>> {
        let c = self.child(id)?;
        let mut v: Vec<String> = c.children("Item").map(|i| i.text()).collect();
        if v.is_empty() {
            v = c.children("item").map(|i| i.text()).collect();
        }
        (!v.is_empty()).then_some(v)
    }
    pub fn hash_array(&self, id: &str) -> Option<Vec<u32>> {
        self.string_array(id).map(|v| v.iter().map(|s| parse_hash(s)).collect())
    }

    fn item_attrs<T>(&self, id: &str, f: impl Fn(XmlNode) -> T) -> Option<Vec<T>> {
        let c = self.child(id)?;
        Some(c.children("Item").map(f).collect())
    }
    pub fn vec2_array(&self, id: &str) -> Option<Vec<[f32; 2]>> {
        self.item_attrs(id, |i| Self::vec_of(Some(i), ["x", "y"]))
    }
    pub fn vec3_array(&self, id: &str) -> Option<Vec<[f32; 3]>> {
        self.item_attrs(id, |i| Self::vec_of(Some(i), ["x", "y", "z"]))
    }
    pub fn vec4_array(&self, id: &str) -> Option<Vec<[f32; 4]>> {
        self.item_attrs(id, |i| Self::vec_of(Some(i), ["x", "y", "z", "w"]))
    }
    pub fn colour_array(&self, id: &str) -> Option<Vec<[u8; 4]>> {
        self.item_attrs(id, |i| ["r", "g", "b", "a"].map(|a| i.attr(a).and_then(parse_uint).unwrap_or(0) as u8))
    }
    pub fn matrix_array<const N: usize>(&self, id: &str) -> Option<Vec<[f32; N]>> {
        self.item_attrs(id, |i| {
            let mut m = [0.0; N];
            for (d, s) in m.iter_mut().zip(split_values(&i.text())) {
                *d = parse_f32(s).unwrap_or(0.0);
            }
            m
        })
    }

    /// `ReadNode`: the child element, if present.
    pub fn node(&self, id: &str) -> Option<XmlNode<'a, 'input>> {
        self.child(id)
    }

    /// `ReadNodeArray`: the `Item` children of `id`; `None` when `id` is absent.
    /// Items with `type="null"` come back as `None`.
    pub fn node_array(&self, id: &str) -> Option<Vec<Option<XmlNode<'a, 'input>>>> {
        let c = self.child(id)?;
        Some(c.children("Item").map(|i| if i.attr("type") == Some("null") { None } else { Some(i) }).collect())
    }

    /// `ReadNodeArray` mapped through `f`, with null items skipped.
    pub fn map_array<T>(&self, id: &str, mut f: impl FnMut(XmlNode<'a, 'input>) -> T) -> Option<Vec<T>> {
        self.node_array(id).map(|v| v.into_iter().flatten().map(&mut f).collect())
    }

    /// Reads a file that sits next to the XML (e.g. a texture's DDS).
    pub fn external(&self, filename: &str) -> Option<Vec<u8>> {
        let folder = self.ctx.externals.as_ref()?;
        if filename.is_empty() {
            return None;
        }
        std::fs::read(folder.join(filename)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_like_redforge() {
        let doc = XmlDocument::new(
            r#"<?xml version="1.0"?><Root version="2"><A value="0x10" /><B value="1E-05" /><Name>abc</Name>
            <Hash>oIZmQFA_0xE9F32960</Hash><Arr>1 2
            3</Arr><Empty /><V x="1" y="2" z="3" /><List><Item><X value="1"/></Item><Item type="null"/></List></Root>"#,
        );
        doc.with_root(None, |r| {
            assert_eq!(r.u32("@version", 0), 2);
            assert_eq!(r.u32("A", 0), 16);
            assert_eq!(r.f32("B", 0.0), 0.00001);
            assert_eq!(r.u32("Missing", 7), 7);
            assert_eq!(r.string("Name").as_deref(), Some("abc"));
            assert_eq!(r.hash("Hash"), 0xE9F32960);
            assert_eq!(r.u16_array("Arr"), Some(vec![1, 2, 3]));
            assert_eq!(r.u16_array("Empty"), None);
            assert_eq!(r.vec3("V"), [1.0, 2.0, 3.0]);
            let l = r.node_array("List").unwrap();
            assert_eq!(l.len(), 2);
            assert_eq!(l[0].unwrap().u32("X", 0), 1);
            assert!(l[1].is_none());
        })
        .unwrap();
    }
}
