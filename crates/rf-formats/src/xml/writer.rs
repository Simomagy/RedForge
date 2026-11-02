//! Writes XML exactly as RedForge's C# `XmlMetaNodeWriter` does, so files stay
//! interchangeable with it and with Sollumz.
//!
//! Layout rules: one space of indent per level; a node with no children closes
//! itself (`<X />`); ids starting with `@` become attributes of the current node;
//! scalars are written as `<id value="..." />`; strings as `<id>text</id>`.

use std::fmt::Display;
use std::path::PathBuf;

use super::number::push_f32;
use crate::hash::HashIndex;
use crate::xml::number::{f32_to_string, f64_to_string};

pub const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
const NL: &str = "\n";

#[derive(Default)]
struct Context {
    /// Indent of this node's children.
    indent: usize,
    attributes: String,
    elements: String,
    /// The root's context collects nothing, like the C# writer's default context.
    collects: bool,
}

pub struct XmlWriter<'a> {
    pub hashes: &'a HashIndex,
    /// Where files referenced by the XML (DDS textures) go.
    pub externals: Option<PathBuf>,
    /// Errors writing external files; the XML is still produced.
    pub external_errors: Vec<String>,
    cx: Context,
    stack: Vec<Context>,
    last: Option<String>,
}

fn is_attribute(id: &str) -> bool {
    id.starts_with('@')
}

fn indent(s: &mut String, n: usize) {
    s.extend(std::iter::repeat_n(' ', n));
}

impl<'a> XmlWriter<'a> {
    pub fn new(hashes: &'a HashIndex) -> Self {
        Self {
            hashes,
            externals: None,
            external_errors: Vec::new(),
            cx: Context::default(),
            stack: Vec::new(),
            last: None,
        }
    }

    /// A whole document: the header and one root node.
    pub fn document(
        hashes: &'a HashIndex,
        root: &str,
        externals: Option<PathBuf>,
        f: impl FnOnce(&mut Self),
    ) -> String {
        let mut w = Self::new(hashes);
        w.externals = externals;
        w.node(root, f);
        let body = w.last.take().unwrap_or_else(|| format!("<{root} />"));
        format!("{HEADER}{NL}{body}")
    }

    pub fn begin(&mut self, _id: &str) {
        let i = self.stack.len();
        let parent = std::mem::take(&mut self.cx);
        self.stack.push(parent);
        self.cx = Context { indent: i + 1, collects: true, ..Default::default() };
    }

    pub fn end(&mut self, id: &str) {
        let done = std::mem::take(&mut self.cx);
        self.cx = self.stack.pop().expect("XmlWriter::end without begin");
        let mut ele = String::with_capacity(done.elements.len() + id.len() * 2 + 16);
        indent(&mut ele, self.cx.indent);
        ele.push('<');
        ele.push_str(id);
        ele.push_str(&done.attributes);
        if done.elements.is_empty() {
            ele.push_str(" />");
            ele.push_str(NL);
        } else {
            ele.push('>');
            ele.push_str(NL);
            ele.push_str(&done.elements);
            indent(&mut ele, self.cx.indent);
            ele.push_str("</");
            ele.push_str(id);
            ele.push('>');
            ele.push_str(NL);
        }
        if self.cx.collects {
            self.cx.elements.push_str(&ele);
        }
        self.last = Some(ele);
    }

    /// `WriteNode`: a child node written by `f`.
    pub fn node(&mut self, id: &str, f: impl FnOnce(&mut Self)) {
        self.begin(id);
        f(self);
        self.end(id);
    }

    /// `WriteNode` with a `type` attribute.
    pub fn typed_node(&mut self, id: &str, ty: &str, f: impl FnOnce(&mut Self)) {
        self.begin(id);
        self.string("@type", ty);
        f(self);
        self.end(id);
    }

    /// `WriteNodeArray`: nothing for `None`, `<id />` when empty, otherwise one
    /// `Item` per element (`None` items become `<Item type="null" />`).
    pub fn node_array<T>(&mut self, id: &str, items: Option<&[T]>, mut f: impl FnMut(&mut Self, &T)) {
        self.node_array_with(id, items, |_, _| None, |w, _, item| f(w, item), false)
    }

    /// `WriteNodeArray` with optional `type` and `i` (index) attributes.
    pub fn node_array_with<T>(
        &mut self,
        id: &str,
        items: Option<&[T]>,
        mut type_of: impl FnMut(usize, &T) -> Option<String>,
        mut f: impl FnMut(&mut Self, usize, &T),
        index_attrs: bool,
    ) {
        let Some(items) = items else { return };
        if items.is_empty() {
            self.element(&format!("<{id} />"));
            return;
        }
        self.begin(id);
        for (i, item) in items.iter().enumerate() {
            self.begin("Item");
            if index_attrs {
                self.i32("@i", i as i32);
            }
            if let Some(t) = type_of(i, item) {
                self.string("@type", &t);
            }
            f(self, i, item);
            self.end("Item");
        }
        self.end(id);
    }

    /// `WriteNodeArray` over optional items.
    pub fn node_array_opt<T>(&mut self, id: &str, items: Option<&[Option<T>]>, mut f: impl FnMut(&mut Self, &T)) {
        let Some(items) = items else { return };
        if items.is_empty() {
            self.element(&format!("<{id} />"));
            return;
        }
        self.begin(id);
        for item in items {
            match item {
                None => self.element("<Item type=\"null\" />"),
                Some(item) => {
                    self.begin("Item");
                    f(self, item);
                    self.end("Item");
                }
            }
        }
        self.end(id);
    }

    /// A raw line in the current node, indented.
    pub fn element(&mut self, e: &str) {
        if !self.cx.collects {
            return;
        }
        indent(&mut self.cx.elements, self.cx.indent);
        self.cx.elements.push_str(e);
        self.cx.elements.push_str(NL);
    }

    fn attribute(&mut self, id: &str, v: &str) {
        if !self.cx.collects {
            return;
        }
        let a = &mut self.cx.attributes;
        a.push(' ');
        a.push_str(&id[1..]);
        a.push_str("=\"");
        a.push_str(v);
        a.push('"');
    }

    /// `WriteValue`: `<id value="v" />`, or an attribute.
    pub fn value(&mut self, id: &str, v: &str) {
        if is_attribute(id) {
            self.attribute(id, v);
        } else if self.cx.collects {
            let e = &mut self.cx.elements;
            indent(e, self.cx.indent);
            e.push('<');
            e.push_str(id);
            e.push_str(" value=\"");
            e.push_str(v);
            e.push_str("\" />");
            e.push_str(NL);
        }
    }

    /// `WriteText`/`WriteString`: `<id>text</id>`, `<id />` when empty, or an attribute.
    pub fn string(&mut self, id: &str, text: &str) {
        if is_attribute(id) {
            self.attribute(id, text);
            return;
        }
        if !self.cx.collects {
            return;
        }
        let ind = self.cx.indent;
        let e = &mut self.cx.elements;
        indent(e, ind);
        if text.is_empty() {
            e.push('<');
            e.push_str(id);
            e.push_str(" />");
        } else if text.ends_with('\n') {
            e.push('<');
            e.push_str(id);
            e.push('>');
            e.push_str(NL);
            e.push_str(text);
            indent(e, ind);
            e.push_str("</");
            e.push_str(id);
            e.push('>');
        } else {
            e.push('<');
            e.push_str(id);
            e.push('>');
            e.push_str(text);
            e.push_str("</");
            e.push_str(id);
            e.push('>');
        }
        e.push_str(NL);
    }

    /// A string that may be absent (`null` in C#): nothing is written for `None`.
    pub fn opt_string(&mut self, id: &str, text: Option<&str>) {
        if let Some(t) = text {
            self.string(id, t);
        }
    }

    pub fn bool(&mut self, id: &str, v: bool) {
        self.value(id, if v { "True" } else { "False" });
    }
    pub fn u8(&mut self, id: &str, v: u8) {
        self.value(id, &v.to_string());
    }
    pub fn i8(&mut self, id: &str, v: i8) {
        self.value(id, &v.to_string());
    }
    pub fn i16(&mut self, id: &str, v: i16) {
        self.value(id, &v.to_string());
    }
    pub fn u16(&mut self, id: &str, v: u16) {
        self.value(id, &v.to_string());
    }
    pub fn i32(&mut self, id: &str, v: i32) {
        self.value(id, &v.to_string());
    }
    pub fn u32(&mut self, id: &str, v: u32) {
        self.value(id, &v.to_string());
    }
    pub fn i64(&mut self, id: &str, v: i64) {
        self.value(id, &v.to_string());
    }
    pub fn u64(&mut self, id: &str, v: u64) {
        self.value(id, &v.to_string());
    }
    pub fn f32(&mut self, id: &str, v: f32) {
        self.value(id, &f32_to_string(v));
    }
    pub fn f64(&mut self, id: &str, v: f64) {
        self.value(id, &f64_to_string(v));
    }

    /// `WriteJenkHash`: the hash's name, a made-up name for unknown hashes, or
    /// `<id />` for 0.
    pub fn hash(&mut self, id: &str, h: u32) {
        let s = self.hashes.xml_name(h);
        self.string(id, &s);
    }

    pub fn colour(&mut self, id: &str, [r, g, b, a]: [u8; 4]) {
        self.element(&format!("<{id} r=\"{r}\" g=\"{g}\" b=\"{b}\" a=\"{a}\" />"));
    }
    pub fn colourf(&mut self, id: &str, [r, g, b, a]: [f32; 4]) {
        let (r, g, b, a) = (f32_to_string(r), f32_to_string(g), f32_to_string(b), f32_to_string(a));
        self.element(&format!("<{id} r=\"{r}\" g=\"{g}\" b=\"{b}\" a=\"{a}\" />"));
    }
    pub fn vec2(&mut self, id: &str, [x, y]: [f32; 2]) {
        self.element(&format!("<{id} x=\"{}\" y=\"{}\" />", f32_to_string(x), f32_to_string(y)));
    }
    pub fn vec3(&mut self, id: &str, [x, y, z]: [f32; 3]) {
        let (x, y, z) = (f32_to_string(x), f32_to_string(y), f32_to_string(z));
        self.element(&format!("<{id} x=\"{x}\" y=\"{y}\" z=\"{z}\" />"));
    }
    pub fn vec4(&mut self, id: &str, [x, y, z, w]: [f32; 4]) {
        let (x, y, z, w) = (f32_to_string(x), f32_to_string(y), f32_to_string(z), f32_to_string(w));
        self.element(&format!("<{id} x=\"{x}\" y=\"{y}\" z=\"{z}\" w=\"{w}\" />"));
    }
    /// 12 floats, 4 per row.
    pub fn matrix3x4(&mut self, id: &str, m: &[f32; 12]) {
        self.raw_array(id, Some(m), |s, v| push_f32(s, *v), 4, "", false);
    }
    /// 16 floats, 4 per row.
    pub fn matrix4x4(&mut self, id: &str, m: &[f32; 16]) {
        self.raw_array(id, Some(m), |s, v| push_f32(s, *v), 4, "", false);
    }

    /// `Xml.WriteRawArray`: values separated by spaces, `row` per line; on one
    /// line when they fit in a row. Nothing for `None`, `<id />` when empty.
    pub fn raw_array<T>(
        &mut self,
        id: &str,
        items: Option<&[T]>,
        mut fmt: impl FnMut(&mut String, &T),
        row: usize,
        attributes: &str,
        force_multiline: bool,
    ) {
        let Some(items) = items else { return };
        if !self.cx.collects {
            return;
        }
        let ind = self.cx.indent;
        let e = &mut self.cx.elements;
        indent(e, ind);
        if items.is_empty() {
            e.push('<');
            e.push_str(id);
            e.push_str(attributes);
            e.push_str(" />");
            e.push_str(NL);
            return;
        }
        e.push('<');
        e.push_str(id);
        e.push_str(attributes);
        e.push('>');
        if items.len() <= row && !force_multiline {
            for (n, v) in items.iter().enumerate() {
                if n > 0 {
                    e.push(' ');
                }
                fmt(e, v);
            }
        } else {
            e.push_str(NL);
            let last = items.len() - 1;
            for (n, v) in items.iter().enumerate() {
                let col = n % row;
                if col == 0 {
                    indent(e, ind + 1);
                } else {
                    e.push(' ');
                }
                fmt(e, v);
                if col == row - 1 || n == last {
                    e.push_str(NL);
                }
            }
            indent(e, ind);
        }
        e.push_str("</");
        e.push_str(id);
        e.push('>');
        e.push_str(NL);
    }

    fn display_array<T: Display>(&mut self, id: &str, items: Option<&[T]>, row: usize) {
        self.raw_array(id, items, |s, v| s.push_str(&v.to_string()), row, "", false);
    }

    pub fn bool_array(&mut self, id: &str, v: Option<&[bool]>) {
        self.raw_array(id, v, |s, b| s.push_str(if *b { "True" } else { "False" }), 16, "", false);
    }
    /// Up to 10 bytes in decimal, more in hex 16 per row with `base="16"`.
    pub fn byte_array(&mut self, id: &str, v: Option<&[u8]>) {
        match v {
            Some(b) if b.len() > 10 => {
                self.raw_array(id, v, |s, x| s.push_str(&format!("{x:02X}")), 16, " base=\"16\"", false)
            }
            _ => self.display_array(id, v, 10),
        }
    }
    pub fn i8_array(&mut self, id: &str, v: Option<&[i8]>) {
        self.display_array(id, v, 16);
    }
    pub fn i16_array(&mut self, id: &str, v: Option<&[i16]>) {
        self.display_array(id, v, 10);
    }
    pub fn u16_array(&mut self, id: &str, v: Option<&[u16]>) {
        self.display_array(id, v, 10);
    }
    pub fn i32_array(&mut self, id: &str, v: Option<&[i32]>) {
        self.display_array(id, v, 10);
    }
    pub fn u32_array(&mut self, id: &str, v: Option<&[u32]>) {
        self.display_array(id, v, 10);
    }
    pub fn i64_array(&mut self, id: &str, v: Option<&[i64]>) {
        self.display_array(id, v, 1);
    }
    /// `0x` and 16 hex digits, one per line.
    pub fn u64_array(&mut self, id: &str, v: Option<&[u64]>) {
        self.raw_array(id, v, |s, x| s.push_str(&format!("0x{x:016X}")), 1, "", false);
    }
    pub fn f32_array(&mut self, id: &str, v: Option<&[f32]>) {
        self.raw_array(id, v, |s, x| push_f32(s, *x), 10, "", false);
    }
    pub fn f64_array(&mut self, id: &str, v: Option<&[f64]>) {
        self.raw_array(id, v, |s, x| s.push_str(&f64_to_string(*x)), 10, "", false);
    }

    /// `Xml.WriteStringItemArray`: one `<Item>` per string.
    pub fn string_array<S: AsRef<str>>(&mut self, id: &str, v: Option<&[S]>) {
        let Some(items) = v else { return };
        if !self.cx.collects {
            return;
        }
        if items.is_empty() {
            self.element(&format!("<{id} />"));
            return;
        }
        let ind = self.cx.indent;
        indent(&mut self.cx.elements, ind);
        self.cx.elements.push('<');
        self.cx.elements.push_str(id);
        self.cx.elements.push('>');
        self.cx.elements.push_str(NL);
        // Items are written one level deeper, as children of `id`.
        self.cx.indent += 1;
        for s in items {
            self.string("Item", s.as_ref());
        }
        self.cx.indent -= 1;
        indent(&mut self.cx.elements, ind);
        self.cx.elements.push_str("</");
        self.cx.elements.push_str(id);
        self.cx.elements.push('>');
        self.cx.elements.push_str(NL);
    }

    pub fn hash_array(&mut self, id: &str, v: Option<&[u32]>) {
        let names: Option<Vec<String>> = v.map(|v| v.iter().map(|h| self.hashes.xml_name(*h)).collect());
        self.string_array(id, names.as_deref());
    }

    pub fn colour_array(&mut self, id: &str, v: Option<&[[u8; 4]]>) {
        self.raw_array(
            id,
            v,
            |s, [r, g, b, a]| s.push_str(&format!("<Item r=\"{r}\" g=\"{g}\" b=\"{b}\" a=\"{a}\" />")),
            1,
            "",
            true,
        );
    }
    pub fn vec2_array(&mut self, id: &str, v: Option<&[[f32; 2]]>) {
        self.raw_array(
            id,
            v,
            |s, [x, y]| s.push_str(&format!("<Item x=\"{}\" y=\"{}\" />", f32_to_string(*x), f32_to_string(*y))),
            1,
            "",
            true,
        );
    }
    pub fn vec3_array(&mut self, id: &str, v: Option<&[[f32; 3]]>) {
        self.raw_array(
            id,
            v,
            |s, [x, y, z]| {
                s.push_str(&format!(
                    "<Item x=\"{}\" y=\"{}\" z=\"{}\" />",
                    f32_to_string(*x),
                    f32_to_string(*y),
                    f32_to_string(*z)
                ))
            },
            1,
            "",
            true,
        );
    }
    pub fn vec4_array(&mut self, id: &str, v: Option<&[[f32; 4]]>) {
        self.raw_array(
            id,
            v,
            |s, [x, y, z, w]| {
                s.push_str(&format!(
                    "<Item x=\"{}\" y=\"{}\" z=\"{}\" w=\"{}\" />",
                    f32_to_string(*x),
                    f32_to_string(*y),
                    f32_to_string(*z),
                    f32_to_string(*w)
                ))
            },
            1,
            "",
            true,
        );
    }
    fn matrix_items<const N: usize>(&mut self, id: &str, v: Option<&[[f32; N]]>) {
        self.raw_array(
            id,
            v,
            |s, m| {
                s.push_str("<Item>");
                for (i, f) in m.iter().enumerate() {
                    if i > 0 {
                        s.push(' ');
                    }
                    push_f32(s, *f);
                }
                s.push_str("</Item>");
            },
            1,
            "",
            true,
        );
    }
    pub fn matrix3x4_array(&mut self, id: &str, v: Option<&[[f32; 12]]>) {
        self.matrix_items(id, v);
    }
    pub fn matrix4x4_array(&mut self, id: &str, v: Option<&[[f32; 16]]>) {
        self.matrix_items(id, v);
    }

    /// Writes a file next to the XML (e.g. a texture's DDS).
    pub fn external(&mut self, filename: &str, data: &[u8]) {
        let Some(folder) = &self.externals else { return };
        if filename.is_empty() {
            return;
        }
        let result = std::fs::create_dir_all(folder).and_then(|_| std::fs::write(folder.join(filename), data));
        if let Err(e) = result {
            self.external_errors.push(format!("{filename}: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_redforge() {
        let h = HashIndex::new();
        let xml = XmlWriter::document(&h, "RDR2TextureDictionary", None, |w| {
            w.u32("@version", 1);
            w.node_array("Textures", Some(&["a", "b"][..]), |w, n| {
                w.string("Name", n);
                w.u32("Flags", 402685954);
            });
            w.node_array::<u8>("Empty", Some(&[]), |_, _| {});
            w.vec3("Pos", [1.0, -0.5, 0.00001]);
            w.u16_array("Few", Some(&[1, 2, 3]));
            w.u16_array("Many", Some(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]));
            w.string("Blank", "");
        });
        let want = [
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            r#"<RDR2TextureDictionary version="1">"#,
            r#" <Textures>"#,
            r#"  <Item>"#,
            r#"   <Name>a</Name>"#,
            r#"   <Flags value="402685954" />"#,
            r#"  </Item>"#,
            r#"  <Item>"#,
            r#"   <Name>b</Name>"#,
            r#"   <Flags value="402685954" />"#,
            r#"  </Item>"#,
            r#" </Textures>"#,
            r#" <Empty />"#,
            r#" <Pos x="1" y="-0.5" z="1E-05" />"#,
            r#" <Few>1 2 3</Few>"#,
            r#" <Many>"#,
            r#"  1 2 3 4 5 6 7 8 9 10"#,
            r#"  11 12"#,
            r#" </Many>"#,
            r#" <Blank />"#,
            r#"</RDR2TextureDictionary>"#,
        ]
        .map(|l| format!("{l}\n"))
        .concat();
        assert_eq!(xml, want);
    }
}
