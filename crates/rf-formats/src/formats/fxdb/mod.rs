//! Shader effects databases (`.fxdb`, magic `SGD1` or `SGDB`), from `FxdbFile.cs`.
//! Read and XML export only: the C# has no XML import and no binary writing.
//!
//! A database holds every compiled shader of the game, one list per stage
//! (vertex, pixel, geometry, domain, hull, compute), and the effects. An effect
//! has techniques; a technique is a range of render shader sets (the passes),
//! and each pass picks one shader per stage (through the effect's index lists),
//! render states and samplers. Shaders and effects describe their parameters
//! with a [`ShaderData`]: resources (constant buffers, textures, samplers...),
//! the elements (variables) of each constant buffer, and annotations.
//!
//! The XML is the C#'s: shader bytecode is read (and Oodle-decompressed) but not
//! written, as the C# has its `WriteExternal` commented out, so no files go next
//! to the XML.
//!
//! Checked against the C# on all six game databases (identical XML). Four of
//! them have Oodle-compressed bytecode, which the C# oracle can't decompress on
//! Linux (no `oo2core_5_win64.dll`), so `rf compare` reports C# errors for them;
//! they were checked by zeroing the compressed-bytecode magic in copies for the
//! C#, which then skips the decompression (the XML doesn't depend on it).
//!
//! Known issues:
//! - A file cut short fails here, where the C# `ReadBytes` would read fewer
//!   bytes (zero-filled) and carry on. No game file is cut short.
//! - Strings that aren't valid UTF-8 are decoded with Rust's replacement rules,
//!   which can differ from .NET's in the number of U+FFFD written. All names in
//!   the game files are ASCII.

mod xml;

use std::ops::Range;

pub use xml::{enum_names, to_xml_string};

use super::XmlContext;
use crate::hash::HashIndex;
use crate::rpf8::compression::oodle_decompress;
use crate::{Result, bail};

/// `FxdbMagic.SGDB`: the companion app's databases.
pub const MAGIC_SGDB: u32 = 0x4244_4753;
/// `FxdbMagic.SGD1`: RDR2's. These have a feature level and a signature per shader.
pub const MAGIC_SGD1: u32 = 0x3144_4753;

/// The `SGDB` and `SGD1` magics.
pub fn is_magic(m: u32) -> bool {
    m == MAGIC_SGDB || m == MAGIC_SGD1
}

/// The fxdb file to XML (`RDR2EffectsDatabase`).
pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let db = FxdbFile::read(file, cx.hashes)?;
    Ok(to_xml_string(&db, cx.hashes))
}

/// A shader stage, which picks one of the database's shader lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShaderStage {
    Vertex,
    Pixel,
    Geometry,
    Domain,
    Hull,
    Compute,
}

impl ShaderStage {
    pub const ALL: [ShaderStage; 6] =
        [Self::Vertex, Self::Pixel, Self::Geometry, Self::Domain, Self::Hull, Self::Compute];
}

/// `FxdbFile`: the whole database.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FxdbFile {
    /// [`MAGIC_SGD1`] or [`MAGIC_SGDB`].
    pub magic: u32,
    pub vertex_shaders: Vec<Shader>,
    pub pixel_shaders: Vec<Shader>,
    pub geometry_shaders: Vec<Shader>,
    pub domain_shaders: Vec<Shader>,
    pub hull_shaders: Vec<Shader>,
    pub compute_shaders: Vec<Shader>,
    pub effects: Vec<Effect>,
}

impl FxdbFile {
    /// `FxdbFile.Load`. Like the C#, adds the names of shader parameters to
    /// `hashes` (they name some of the effects' hashes in the XML).
    pub fn read(data: &[u8], hashes: &HashIndex) -> Result<Self> {
        let mut r = Reader { data, pos: 0 };
        let magic = r.u32()?;
        if !is_magic(magic) {
            bail!("fxdb: unknown file magic 0x{magic:08X}");
        }
        let list = |r: &mut Reader| -> Result<Vec<Shader>> {
            let count = r.u32()?;
            (0..count).map(|_| Shader::read(r, magic, hashes)).collect()
        };
        let mut db = FxdbFile {
            magic,
            vertex_shaders: list(&mut r)?,
            pixel_shaders: list(&mut r)?,
            geometry_shaders: list(&mut r)?,
            domain_shaders: list(&mut r)?,
            hull_shaders: list(&mut r)?,
            compute_shaders: list(&mut r)?,
            effects: Vec::new(),
        };
        let count = r.u32()?;
        db.effects = (0..count).map(|_| Effect::read(&mut r, hashes)).collect::<Result<_>>()?;
        Ok(db)
    }

    pub fn shaders(&self, stage: ShaderStage) -> &[Shader] {
        match stage {
            ShaderStage::Vertex => &self.vertex_shaders,
            ShaderStage::Pixel => &self.pixel_shaders,
            ShaderStage::Geometry => &self.geometry_shaders,
            ShaderStage::Domain => &self.domain_shaders,
            ShaderStage::Hull => &self.hull_shaders,
            ShaderStage::Compute => &self.compute_shaders,
        }
    }

    /// The shader a pass uses for a stage, as the C#'s `getShader`: the pass's
    /// index goes through the effect's index list into the database's list.
    /// `None` when either index is out of range (the stage is unused).
    pub fn pass_shader(&self, effect: &Effect, pass: &RenderShaderSetDesc, stage: ShaderStage) -> Option<&Shader> {
        let i = *effect.shader_indices(stage).get(pass.shader_index(stage) as usize)?;
        self.shaders(stage).get(i as usize)
    }
}

/// `FxdbShader`: one compiled shader.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shader {
    /// Not unique: several shaders can have the same name.
    pub name: String,
    /// `SGD1` only (50, 60...); 0 for `SGDB`.
    pub feature_level: u8,
    /// The shader bytecode (DXBC, DXIL or SPIR-V), decompressed when the file
    /// has it Oodle-compressed.
    pub bytecode: Vec<u8>,
    /// `atDataHash64` of the bytecode and usage. `SGD1` only.
    pub signature: u64,
    pub usage: ShaderUsage,
    pub data: ShaderData,
}

/// The `FxdbCompressedDataHeader` magic at the start of compressed bytecode.
const COMPRESSED_MAGIC: u16 = 0xDCE7;

impl Shader {
    fn read(r: &mut Reader, magic: u32, hashes: &HashIndex) -> Result<Self> {
        let sgd1 = magic == MAGIC_SGD1;
        let name = r.string_1b()?;
        let feature_level = if sgd1 { r.u8()? } else { 0 };
        let len = r.u32()? as usize;
        let mut bytecode = r.bytes(len)?.to_vec();
        let signature = if sgd1 { r.u64()? } else { 0 };
        let usage = ShaderUsage { inputs: ShaderUsageMask::read(r)?, outputs: ShaderUsageMask::read(r)? };
        let data = ShaderData::read(r, hashes)?;
        // Header: u16 magic, u8 compressor, u8 padding, i32 compressed size
        // with the header, i32 decompressed size.
        if bytecode.len() >= 12 && u16::from_le_bytes([bytecode[0], bytecode[1]]) == COMPRESSED_MAGIC {
            let size = i32::from_le_bytes(bytecode[8..12].try_into().unwrap());
            bytecode = match size {
                ..0 => bail!("fxdb: shader {name}: bad decompressed size {size}"),
                0 => Vec::new(),
                _ => oodle_decompress(&bytecode[12..], size as usize)
                    .map_err(|e| crate::Error::Format(format!("fxdb: shader {name}: {e}")))?,
            };
        }
        Ok(Shader { name, feature_level, bytecode, signature, usage, data })
    }
}

/// `FxdbShaderUsage` (`rage::sga::ShaderUsage`): the shader's inputs and outputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderUsage {
    pub inputs: ShaderUsageMask,
    pub outputs: ShaderUsageMask,
}

/// `FxdbShaderUsageMask` (`rage::sga::ShaderUsageMask`).
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderUsageMask {
    /// Bit `n` set: semantic `n` (`POSITION` = 0 ... `TEXCOORD23` = 0x33) is used.
    pub semantics_mask: u64,
    /// Bit `n` set: system-value semantic `0x34 + n` (`SV_Position`...) is used.
    pub sv_semantics_mask: u64,
    /// The buffer format (`FxdbBufferFormat`) of each non-SV semantic, 0 if unused.
    pub semantics_formats: [u8; 52],
    pub padding: u32,
}

impl Default for ShaderUsageMask {
    fn default() -> Self {
        Self { semantics_mask: 0, sv_semantics_mask: 0, semantics_formats: [0; 52], padding: 0 }
    }
}

/// The first system-value semantic, `SV_Position`.
pub const SV_FIRST: u8 = 0x34;
/// The last semantic, `SV_Target7`.
pub const SV_LAST: u8 = 0x5B;
/// The last non-SV semantic, `TEXCOORD23`.
pub const SEMANTIC_LAST: u8 = 0x33;

impl ShaderUsageMask {
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            semantics_mask: r.u64()?,
            sv_semantics_mask: r.u64()?,
            semantics_formats: r.array()?,
            padding: r.u32()?,
        })
    }

    /// `GetSemantics`: the used semantics (`FxdbIndexedSemantic` values), the
    /// normal ones first. Like the C#, the list has one entry per set bit, and
    /// bits past the known semantics are left as 0 (`POSITION`) at the end.
    pub fn semantics(&self) -> Vec<u8> {
        let n = (self.semantics_mask.count_ones() + self.sv_semantics_mask.count_ones()) as usize;
        let mut out = Vec::with_capacity(n);
        for s in 0..=SEMANTIC_LAST {
            if self.semantics_mask & (1u64 << s) != 0 {
                out.push(s);
            }
        }
        for s in SV_FIRST..=SV_LAST {
            if self.sv_semantics_mask & (1u64 << (s - SV_FIRST)) != 0 {
                out.push(s);
            }
        }
        out.resize(n, 0);
        out
    }
}

/// `FxdbShaderData` (`rage::sga::ShaderData`): the parameters of a shader or
/// an effect. The resources refer to ranges of the elements and annotations.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderData {
    /// The size before the data; the C# reads by `data_length` instead.
    pub data_len: u32,
    pub resource_count: u16,
    pub element_count: u16,
    pub annotation_count: u16,
    pub data_length: u16,
    pub resources: Vec<ShaderResourceData>,
    pub elements: Vec<ShaderElementData>,
    pub annotations: Vec<ShaderAnnotation>,
    pub name_heap: Vec<u8>,
    pub default_values_buffer: Vec<u8>,
}

impl ShaderData {
    fn read(r: &mut Reader, hashes: &HashIndex) -> Result<Self> {
        let data_len = r.u32()?;
        let start = r.pos;
        let resource_count = r.u16()?;
        let element_count = r.u16()?;
        let annotation_count = r.u16()?;
        let data_length = r.u16()?;
        let mut resources: Vec<_> = (0..resource_count).map(|_| ShaderResourceData::read(r)).collect::<Result<_>>()?;
        let mut elements: Vec<_> = (0..element_count).map(|_| ShaderElementData::read(r)).collect::<Result<_>>()?;
        let mut annotations: Vec<_> =
            (0..annotation_count).map(|_| ShaderAnnotation::read(r)).collect::<Result<_>>()?;

        let mut defaults_size = 0i64;
        for e in &elements {
            if e.default_value_offset != 0 {
                defaults_size += e.byte_size()? as i64;
            }
        }
        let heap_size = data_length as i64 - (r.pos - start) as i64 - defaults_size;
        if heap_size < 0 {
            bail!("fxdb: shader data: name heap size {heap_size}");
        }
        let name_heap = r.bytes(heap_size as usize)?.to_vec();
        let default_values_buffer = r.bytes(defaults_size as usize)?.to_vec();

        // The offsets are relative to the field's own struct: take away the
        // structs that come after it to make them relative to the heap.
        const RES: i64 = ShaderResourceData::SIZE as i64;
        const ELE: i64 = ShaderElementData::SIZE as i64;
        const ANN: i64 = ShaderAnnotation::SIZE as i64;
        let (rc, ec, ac) = (resource_count as i64, element_count as i64, annotation_count as i64);
        for (i, res) in resources.iter_mut().enumerate() {
            let after = RES * (rc - i as i64) + ELE * ec + ANN * ac;
            res.name = heap_string(&name_heap, res.name_offset as i64 - after)?;
            hashes.ensure(&res.name.to_lowercase());
            if res.semantic_name_offset != 0 {
                let s = heap_string(&name_heap, res.semantic_name_offset as i64 - after)?;
                hashes.ensure(&s.to_lowercase());
                res.semantic_name = Some(s);
            }
            if res.element_count != 0 {
                let first = (res.elements_offset as i64 - RES * (rc - i as i64)) / ELE;
                res.elements = checked_range(first, res.element_count as usize, elements.len(), "elements")?;
            }
            if res.annotation_count != 0 {
                let first = (res.annotations_offset as i64 - RES * (rc - i as i64) - ELE * ec) / ANN;
                res.annotations =
                    checked_range(first, res.annotation_count as usize, annotations.len(), "annotations")?;
            }
        }
        for (i, ele) in elements.iter_mut().enumerate() {
            let after = ELE * (ec - i as i64) + ANN * ac;
            ele.name = heap_string(&name_heap, ele.name_offset as i64 - after)?;
            hashes.ensure(&ele.name.to_lowercase());
            if ele.semantic_name_offset != 0 {
                let s = heap_string(&name_heap, ele.semantic_name_offset as i64 - after)?;
                hashes.ensure(&s.to_lowercase());
                ele.semantic_name = Some(s);
            }
            if ele.default_value_offset != 0 {
                let off = ele.default_value_offset as i64 - after - heap_size;
                let size = ele.byte_size()?;
                let value = usize::try_from(off).ok().and_then(|o| default_values_buffer.get(o..o + size));
                let Some(value) = value else { bail!("fxdb: element {}: bad default value offset", ele.name) };
                ele.default_value = Some(value.to_vec());
            }
            if ele.annotation_count != 0 {
                let first = (ele.annotations_offset as i64 - ELE * (ec - i as i64)) / ANN;
                ele.annotations =
                    checked_range(first, ele.annotation_count as usize, annotations.len(), "annotations")?;
            }
        }
        for (i, ann) in annotations.iter_mut().enumerate() {
            if ann.annotation_type() == ShaderAnnotation::STRING {
                let off = ann.value as i32 as i64 - ANN * (ac - i as i64);
                ann.value_as_string = Some(heap_string(&name_heap, off)?);
            }
        }

        Ok(Self {
            data_len,
            resource_count,
            element_count,
            annotation_count,
            data_length,
            resources,
            elements,
            annotations,
            name_heap,
            default_values_buffer,
        })
    }

    /// The elements (variables) of a resource (a constant buffer).
    pub fn resource_elements(&self, res: &ShaderResourceData) -> &[ShaderElementData] {
        &self.elements[res.elements.clone()]
    }

    pub fn resource_annotations(&self, res: &ShaderResourceData) -> &[ShaderAnnotation] {
        &self.annotations[res.annotations.clone()]
    }

    pub fn element_annotations(&self, ele: &ShaderElementData) -> &[ShaderAnnotation] {
        &self.annotations[ele.annotations.clone()]
    }
}

/// `GetStringFromNameHeap` and `LookupString`: the zero-terminated string at
/// `offset`. Fails, like the C#, when there's no string there.
fn heap_string(heap: &[u8], offset: i64) -> Result<String> {
    let s = usize::try_from(offset).ok().and_then(|o| heap.get(o..));
    let Some(s) = s else { bail!("fxdb: string offset {offset} out of range") };
    let Some(end) = s.iter().position(|&b| b == 0) else { bail!("fxdb: string at {offset} has no end") };
    Ok(String::from_utf8_lossy(&s[..end]).into_owned())
}

fn checked_range(first: i64, count: usize, len: usize, what: &str) -> Result<Range<usize>> {
    match usize::try_from(first) {
        Ok(f) if f + count <= len => Ok(f..f + count),
        _ => bail!("fxdb: {what} {first}..+{count} out of range (of {len})"),
    }
}

/// `FxdbShaderResourceData` (`rage::sga::ShaderResourceData`): a constant
/// buffer, texture, sampler, UAV...
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderResourceData {
    /// Bits 0-3: [`resource_type`](Self::resource_type); 4-5:
    /// [`resource_class`](Self::resource_class); 6: [`unk_flag`](Self::unk_flag).
    pub resource_type_and_resource_class_and_unk_flag: u8,
    /// `FxdbImageDimension`.
    pub image_dimension: u8,
    pub bind_point: u8,
    pub annotation_count: u8,
    pub element_count: u8,
    /// Bits 0-5: [`unk`](Self::unk); 6-7: [`srt_index`](Self::srt_index).
    pub unk_and_srt_index: u8,
    pub elements_offset: u16,
    pub name_offset: u16,
    pub semantic_name_offset: u16,
    pub byte_size: u16,
    pub annotations_offset: u16,
    pub name_hash: u32,
    pub semantic_name_hash: u32,

    pub name: String,
    pub semantic_name: Option<String>,
    /// The resource's elements, in [`ShaderData::elements`]. Empty for none.
    pub elements: Range<usize>,
    /// The resource's annotations, in [`ShaderData::annotations`].
    pub annotations: Range<usize>,
}

impl ShaderResourceData {
    pub const SIZE: usize = 24;

    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            resource_type_and_resource_class_and_unk_flag: r.u8()?,
            image_dimension: r.u8()?,
            bind_point: r.u8()?,
            annotation_count: r.u8()?,
            element_count: r.u8()?,
            unk_and_srt_index: r.u8()?,
            elements_offset: r.u16()?,
            name_offset: r.u16()?,
            semantic_name_offset: r.u16()?,
            byte_size: r.u16()?,
            annotations_offset: r.u16()?,
            name_hash: r.u32()?,
            semantic_name_hash: r.u32()?,
            ..Default::default()
        })
    }

    /// `FxdbResourceType`: 0 `CBuffer`, 1 `Typed`, 2 `Texture`, 3 `Sampler`...
    pub fn resource_type(&self) -> u8 {
        self.resource_type_and_resource_class_and_unk_flag & 0xF
    }
    /// `FxdbResourceClass`: 0 `ShaderResource`, 1 `UnorderedAccess`, 2 `Sampler`, 3 `CBuffer`.
    pub fn resource_class(&self) -> u8 {
        (self.resource_type_and_resource_class_and_unk_flag >> 4) & 0x3
    }
    pub fn unk_flag(&self) -> bool {
        self.resource_type_and_resource_class_and_unk_flag & (1 << 6) != 0
    }
    /// `FxdbSrtIndex`.
    pub fn srt_index(&self) -> u8 {
        self.unk_and_srt_index >> 6
    }
    pub fn unk(&self) -> u8 {
        self.unk_and_srt_index & 0x3F
    }
}

/// `FxdbShaderElementData` (`rage::sga::ShaderElementData`): a variable in a
/// constant buffer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderElementData {
    /// `FxdbVariableType`: see [`variable_type_size`].
    pub type_: u8,
    pub annotation_count: u8,
    /// 0 for non-arrays.
    pub array_size: u16,
    /// The byte offset in the constant buffer.
    pub start_offset: u16,
    pub name_offset: u16,
    pub semantic_name_offset: u16,
    /// 0 for no default value.
    pub default_value_offset: u16,
    pub annotations_offset: u16,
    pub padding_0e: u16,
    pub name_hash: u32,
    pub semantic_name_hash: u32,

    pub name: String,
    pub semantic_name: Option<String>,
    /// The raw default value, [`byte_size`](Self::byte_size) bytes.
    pub default_value: Option<Vec<u8>>,
    /// The element's annotations, in [`ShaderData::annotations`].
    pub annotations: Range<usize>,
}

/// `VariableTypeByteSize`: the size of one value of an `FxdbVariableType`
/// (0 `Bool`, 1-4 `Int`-`Int4`, 5-8 `UInt`-`UInt4`, 9-12 `Float`-`Float4`,
/// 13 `Matrix34`, 14 `Matrix44`; see [`enum_names::variable_type`]).
pub fn variable_type_size(t: u8) -> Option<usize> {
    Some(match t {
        0 | 1 | 5 | 9 => 4,
        2 | 6 | 10 => 8,
        3 | 7 | 11 => 12,
        4 | 8 | 12 => 16,
        13 | 14 => 64,
        _ => return None,
    })
}

impl ShaderElementData {
    pub const SIZE: usize = 24;

    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            type_: r.u8()?,
            annotation_count: r.u8()?,
            array_size: r.u16()?,
            start_offset: r.u16()?,
            name_offset: r.u16()?,
            semantic_name_offset: r.u16()?,
            default_value_offset: r.u16()?,
            annotations_offset: r.u16()?,
            padding_0e: r.u16()?,
            name_hash: r.u32()?,
            semantic_name_hash: r.u32()?,
            ..Default::default()
        })
    }

    /// `CalcElementByteSize`: the size of the whole variable (all of an array).
    /// Fails for types with no size, as the C# throws.
    pub fn byte_size(&self) -> Result<usize> {
        let Some(size) = variable_type_size(self.type_) else {
            bail!("fxdb: variable type {} has no byte size", self.type_)
        };
        Ok(size * self.array_size.max(1) as usize)
    }
}

/// `FxdbShaderAnnotation` (`rage::sga::ShaderAnnotation`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderAnnotation {
    /// Bits 0-1: the type (`Int`, `Float`, `String`); 2-31: the name's hash
    /// without its top two bits.
    pub type_and_name_hash: u32,
    /// The int, the float's bits, or the string's offset.
    pub value: u32,
    /// The string, for string annotations.
    pub value_as_string: Option<String>,
}

impl ShaderAnnotation {
    pub const SIZE: usize = 8;
    pub const INT: u8 = 0;
    pub const FLOAT: u8 = 1;
    pub const STRING: u8 = 2;

    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self { type_and_name_hash: r.u32()?, value: r.u32()?, value_as_string: None })
    }

    /// [`INT`](Self::INT), [`FLOAT`](Self::FLOAT), [`STRING`](Self::STRING) or 3.
    pub fn annotation_type(&self) -> u8 {
        (self.type_and_name_hash & 3) as u8
    }
    /// The name's hash `& 0x3FFFFFFF`.
    pub fn name_hash(&self) -> u32 {
        self.type_and_name_hash >> 2
    }
    pub fn value_as_int(&self) -> i32 {
        self.value as i32
    }
    pub fn value_as_float(&self) -> f32 {
        f32::from_bits(self.value)
    }
}

/// `FxdbEffect` (`rage::sga::Effect`, though laid out differently in memory).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Effect {
    pub name: String,
    pub data_buffer_size: u32,
    /// Indices into the database's vertex shaders, used by the passes'
    /// `vertex_shader_index`. The same for the other stages.
    pub vertex_shader_indices: Vec<u32>,
    pub pixel_shader_indices: Vec<u32>,
    pub geometry_shader_indices: Vec<u32>,
    pub domain_shader_indices: Vec<u32>,
    pub hull_shader_indices: Vec<u32>,
    pub compute_shader_indices: Vec<u32>,
    /// 72 bytes.
    pub unk_struct: Vec<u8>,
    pub unk1: u64,
    pub rasterizer_states: Vec<RasterizerStateDesc>,
    pub depth_stencil_states: Vec<DepthStencilStateDesc>,
    pub blend_states: Vec<BlendStateDesc>,
    pub sampler_states: Vec<SamplerStateDesc>,
    pub techniques: Vec<TechniqueDesc>,
    /// The passes of all the techniques.
    pub render_shader_sets: Vec<RenderShaderSetDesc>,
    /// Hash lists, read from a list of (count, hashes...).
    pub unks: Vec<Vec<u32>>,
    pub sampler_state_refs: Vec<SamplerStateRef>,
    pub shader_data: ShaderData,
    /// Misc properties in the annotations of its one resource, `proplst`
    /// (`__rage_drawbucket`, `__rage_instanced`...).
    pub properties_shader_data: ShaderData,
    pub string_table: Vec<u8>,
}

impl Effect {
    fn read(r: &mut Reader, hashes: &HashIndex) -> Result<Self> {
        let name = r.string_1b()?;
        let data_buffer_size = r.u32()?;
        let vertex_shader_indices = r.struct_array(Reader::u32)?;
        let pixel_shader_indices = r.struct_array(Reader::u32)?;
        let geometry_shader_indices = r.struct_array(Reader::u32)?;
        let domain_shader_indices = r.struct_array(Reader::u32)?;
        let hull_shader_indices = r.struct_array(Reader::u32)?;
        let compute_shader_indices = r.struct_array(Reader::u32)?;
        let unk_struct = r.bytes(72)?.to_vec();
        let unk1 = r.u64()?;
        let rasterizer_states = r.struct_array(RasterizerStateDesc::read)?;
        let depth_stencil_states = r.struct_array(DepthStencilStateDesc::read)?;
        let blend_states = r.struct_array(BlendStateDesc::read)?;
        let sampler_states = r.struct_array(SamplerStateDesc::read)?;
        let mut techniques = r.struct_array(|r| Ok(TechniqueDesc { data: r.u32()?, name: String::new() }))?;
        let render_shader_sets = r.struct_array(RenderShaderSetDesc::read)?;
        let unks = parse_unks(&r.struct_array(Reader::u32)?)?;
        let mut sampler_state_refs =
            r.struct_array(|r| Ok(SamplerStateRef { name_offset: r.u8()?, sampler_state_index: r.u8()?, name: None }))?;
        let shader_data = ShaderData::read(r, hashes)?;
        let properties_shader_data = ShaderData::read(r, hashes)?;
        // Unused data, maybe `rage::sga::DescriptorTable`s; always empty.
        let count = r.u32()?;
        for _ in 0..count {
            let len = r.u32()? as usize;
            r.bytes(len)?;
        }
        let len = r.u32()? as usize;
        let string_table = r.bytes(len)?.to_vec();

        for t in &mut techniques {
            t.name = heap_string(&string_table, t.name_offset() as i64)?;
        }
        for s in &mut sampler_state_refs {
            s.name = heap_string(&string_table, s.name_offset as i64).ok();
        }
        let effect = Effect {
            name,
            data_buffer_size,
            vertex_shader_indices,
            pixel_shader_indices,
            geometry_shader_indices,
            domain_shader_indices,
            hull_shader_indices,
            compute_shader_indices,
            unk_struct,
            unk1,
            rasterizer_states,
            depth_stencil_states,
            blend_states,
            sampler_states,
            techniques,
            render_shader_sets,
            unks,
            sampler_state_refs,
            shader_data,
            properties_shader_data,
            string_table,
        };
        effect.check_xml()?;
        Ok(effect)
    }

    /// Fails where the C#'s XML writing would throw, so that writing can't.
    fn check_xml(&self) -> Result<()> {
        if !self.unks.is_empty() && (self.render_shader_sets.is_empty() || self.techniques.is_empty()) {
            bail!("fxdb: effect {}: hash lists without passes or techniques", self.name);
        }
        for pass in &self.render_shader_sets {
            if pass.sampler_state_count == 0 {
                continue;
            }
            let ok = self.pass_sampler_refs(pass).is_some_and(|refs| refs.iter().all(|s| s.name.is_some()));
            if !ok {
                bail!("fxdb: effect {}: bad sampler states in a pass", self.name);
            }
        }
        Ok(())
    }

    pub fn shader_indices(&self, stage: ShaderStage) -> &[u32] {
        match stage {
            ShaderStage::Vertex => &self.vertex_shader_indices,
            ShaderStage::Pixel => &self.pixel_shader_indices,
            ShaderStage::Geometry => &self.geometry_shader_indices,
            ShaderStage::Domain => &self.domain_shader_indices,
            ShaderStage::Hull => &self.hull_shader_indices,
            ShaderStage::Compute => &self.compute_shader_indices,
        }
    }

    /// `LookupString`: a string in the effect's string table.
    pub fn lookup_string(&self, offset: usize) -> Option<String> {
        heap_string(&self.string_table, offset as i64).ok()
    }

    /// The passes of a technique; `None` if its range is out of bounds.
    pub fn technique_passes(&self, t: &TechniqueDesc) -> Option<&[RenderShaderSetDesc]> {
        let first = t.render_shader_set_start_index() as usize;
        self.render_shader_sets.get(first..first + t.render_shader_set_count() as usize)
    }

    /// The samplers a pass binds (name and index in [`sampler_states`](Self::sampler_states)).
    pub fn pass_sampler_refs(&self, pass: &RenderShaderSetDesc) -> Option<&[SamplerStateRef]> {
        let first = pass.sampler_state_start_index as usize;
        self.sampler_state_refs.get(first..first + pass.sampler_state_count as usize)
    }
}

/// The C#'s `Unks` parsing: a count, that many hashes, the next count...
fn parse_unks(unks: &[u32]) -> Result<Vec<Vec<u32>>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < unks.len() {
        let count = unks[i] as usize;
        if count > 1 << 24 {
            bail!("fxdb: hash list of {count}");
        }
        // Like the C#, a list running past the end is padded with zeros.
        let mut list = vec![0u32; count];
        let avail = unks.len() - i - 1;
        list[..count.min(avail)].copy_from_slice(&unks[i + 1..i + 1 + count.min(avail)]);
        out.push(list);
        i += count + 1;
    }
    Ok(out)
}

/// `FxdbRasterizerStateDesc` (`rage::sga::RasterizerStateDesc`, like
/// `D3D12_RASTERIZER_DESC`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RasterizerStateDesc {
    /// `FxdbFillMode`: 2 `Wireframe`, 3 `Solid`.
    pub fill_mode: u8,
    /// `FxdbCullMode`: 1 `None`, 2 `Front`, 3 `Back`.
    pub cull_mode: u8,
    pub front_counter_clockwise: bool,
    pub depth_bias: i8,
    pub depth_bias_clamp: f32,
    pub slope_scaled_depth_bias: f32,
    pub depth_clip_enable: bool,
    pub unknown_0d: u8,
    pub multisample_enable: bool,
    pub antialiased_line_enable: bool,
}

impl RasterizerStateDesc {
    pub const SIZE: usize = 16;

    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            fill_mode: r.u8()?,
            cull_mode: r.u8()?,
            front_counter_clockwise: r.bool()?,
            depth_bias: r.u8()? as i8,
            depth_bias_clamp: r.f32()?,
            slope_scaled_depth_bias: r.f32()?,
            depth_clip_enable: r.bool()?,
            unknown_0d: r.u8()?,
            multisample_enable: r.bool()?,
            antialiased_line_enable: r.bool()?,
        })
    }
}

/// `FxdbStencilOpDesc` (like `D3D12_DEPTH_STENCILOP_DESC`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StencilOpDesc {
    /// `FxdbStencilOp` (`D3D12_STENCIL_OP`).
    pub stencil_fail_op: u8,
    pub stencil_depth_fail_op: u8,
    pub stencil_pass_op: u8,
    /// `FxdbComparisonFunc` (`D3D12_COMPARISON_FUNC`).
    pub stencil_func: u8,
}

impl StencilOpDesc {
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            stencil_fail_op: r.u8()?,
            stencil_depth_fail_op: r.u8()?,
            stencil_pass_op: r.u8()?,
            stencil_func: r.u8()?,
        })
    }
}

/// `FxdbDepthStencilStateDesc` (`rage::sga::DepthStencilStateDesc`, like
/// `D3D12_DEPTH_STENCIL_DESC1`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DepthStencilStateDesc {
    pub depth_enable: bool,
    pub depth_write_enable: bool,
    /// `FxdbComparisonFunc`.
    pub depth_func: u8,
    pub stencil_enable: bool,
    pub stencil_read_mask: u8,
    pub stencil_write_mask: u8,
    pub front_face: StencilOpDesc,
    pub back_face: StencilOpDesc,
    /// Vulkan only: whether the back face has its own ops and masks.
    pub has_different_back_face: bool,
    pub depth_bounds_test_enable: bool,
    pub unknown_10: u8,
    pub unknown_11: u8,
    pub back_face_stencil_read_mask: u8,
    pub back_face_stencil_write_mask: u8,
    pub unknown_14: i32,
}

impl DepthStencilStateDesc {
    pub const SIZE: usize = 24;

    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            depth_enable: r.bool()?,
            depth_write_enable: r.bool()?,
            depth_func: r.u8()?,
            stencil_enable: r.bool()?,
            stencil_read_mask: r.u8()?,
            stencil_write_mask: r.u8()?,
            front_face: StencilOpDesc::read(r)?,
            back_face: StencilOpDesc::read(r)?,
            has_different_back_face: r.bool()?,
            depth_bounds_test_enable: r.bool()?,
            unknown_10: r.u8()?,
            unknown_11: r.u8()?,
            back_face_stencil_read_mask: r.u8()?,
            back_face_stencil_write_mask: r.u8()?,
            unknown_14: r.u32()? as i32,
        })
    }
}

/// `FxdbBlendRenderTargetDesc` (like `D3D12_RENDER_TARGET_BLEND_DESC`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlendRenderTargetDesc {
    pub blend_enable: bool,
    /// `FxdbBlendFactor` (`D3D12_BLEND`).
    pub src_blend: u8,
    pub dest_blend: u8,
    /// `FxdbBlendOp` (`D3D12_BLEND_OP`).
    pub blend_op: u8,
    pub src_blend_alpha: u8,
    pub dest_blend_alpha: u8,
    pub blend_op_alpha: u8,
    /// `FxdbColorWriteEnable` flags: 1 red, 2 green, 4 blue, 8 alpha.
    pub render_target_write_mask: u8,
}

impl BlendRenderTargetDesc {
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            blend_enable: r.bool()?,
            src_blend: r.u8()?,
            dest_blend: r.u8()?,
            blend_op: r.u8()?,
            src_blend_alpha: r.u8()?,
            dest_blend_alpha: r.u8()?,
            blend_op_alpha: r.u8()?,
            render_target_write_mask: r.u8()?,
        })
    }
}

/// `FxdbBlendStateDesc` (`rage::sga::BlendStateDesc`, like `D3D12_BLEND_DESC`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlendStateDesc {
    pub alpha_to_coverage_enable: bool,
    pub independent_blend_enable: bool,
    pub unknown_02: u8,
    pub unknown_03: u8,
    pub render_targets: [BlendRenderTargetDesc; 8],
}

impl BlendStateDesc {
    pub const SIZE: usize = 68;

    fn read(r: &mut Reader) -> Result<Self> {
        let mut s = Self {
            alpha_to_coverage_enable: r.bool()?,
            independent_blend_enable: r.bool()?,
            unknown_02: r.u8()?,
            unknown_03: r.u8()?,
            render_targets: Default::default(),
        };
        for rt in &mut s.render_targets {
            *rt = BlendRenderTargetDesc::read(r)?;
        }
        Ok(s)
    }
}

/// `FxdbSamplerStateDesc` (`rage::sga::SamplerStateDesc`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SamplerStateDesc {
    /// `D3D12_FILTER`.
    pub filter: u8,
    /// `FxdbTextureAddressMode` (`D3D12_TEXTURE_ADDRESS_MODE`).
    pub address_u: u8,
    pub address_v: u8,
    pub address_w: u8,
    pub mip_lod_bias: f32,
    pub max_anisotropy: u8,
    /// `FxdbComparisonFunc`.
    pub comparison_func: u8,
    pub unknown_0a: u8,
    pub unknown_0b: u8,
    /// RGBA.
    pub border_colour: [f32; 4],
    pub min_lod: f32,
    pub max_lod: f32,
}

impl SamplerStateDesc {
    pub const SIZE: usize = 36;

    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            filter: r.u8()?,
            address_u: r.u8()?,
            address_v: r.u8()?,
            address_w: r.u8()?,
            mip_lod_bias: r.f32()?,
            max_anisotropy: r.u8()?,
            comparison_func: r.u8()?,
            unknown_0a: r.u8()?,
            unknown_0b: r.u8()?,
            border_colour: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
            min_lod: r.f32()?,
            max_lod: r.f32()?,
        })
    }
}

/// `FxdbTechniqueDesc`: a technique, the range of passes it runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TechniqueDesc {
    /// Bits 0-14: the name's offset in the string table; 15-23: the first pass;
    /// 24-31: the pass count.
    pub data: u32,
    /// The name, from the effect's string table.
    pub name: String,
}

impl TechniqueDesc {
    pub fn name_offset(&self) -> u32 {
        self.data & 0x7FFF
    }
    pub fn render_shader_set_start_index(&self) -> u32 {
        (self.data >> 15) & 0x1FF
    }
    pub fn render_shader_set_count(&self) -> u32 {
        self.data >> 24
    }
}

/// `FxdbRenderShaderSetFlags`.
pub mod render_shader_set_flags {
    pub const HAS_RASTERIZER_STATE: u8 = 1 << 0;
    pub const HAS_DEPTH_STENCIL_STATE: u8 = 1 << 1;
    pub const HAS_BLEND_STATE: u8 = 1 << 2;
    pub const HAS_UNKNOWN_09: u8 = 1 << 3;
    pub const UNK4: u8 = 1 << 4;
    pub const DEPTH_STENCIL_STATE_INDEX_EXTRA_BIT: u8 = 1 << 5;
    pub const BLEND_STATE_INDEX_EXTRA_BIT: u8 = 1 << 6;
}

/// `FxdbRenderShaderSetDesc`: a pass. The shader indices go through the
/// effect's index lists (see [`FxdbFile::pass_shader`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderShaderSetDesc {
    pub vertex_shader_index: u8,
    pub pixel_shader_index: u8,
    pub geometry_shader_index: u8,
    pub domain_shader_index: u8,
    pub hull_shader_index: u8,
    pub compute_shader_index: u8,
    pub rasterizer_state_index: u8,
    pub depth_stencil_state_index: u8,
    pub blend_state_index: u8,
    pub unknown_09: u8,
    /// [`render_shader_set_flags`].
    pub flags: u8,
    pub sampler_state_count: u8,
    /// The first of the pass's samplers in the effect's `sampler_state_refs`.
    pub sampler_state_start_index: u16,
    pub unknown_0e: u16,
    pub unknown_10: u8,
    pub unknown_11: u8,
    /// Always 0xFF: two indices into the effect's unused data.
    pub unknown_12_0: u8,
    pub unknown_12_1: u8,
}

impl RenderShaderSetDesc {
    pub const SIZE: usize = 20;

    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            vertex_shader_index: r.u8()?,
            pixel_shader_index: r.u8()?,
            geometry_shader_index: r.u8()?,
            domain_shader_index: r.u8()?,
            hull_shader_index: r.u8()?,
            compute_shader_index: r.u8()?,
            rasterizer_state_index: r.u8()?,
            depth_stencil_state_index: r.u8()?,
            blend_state_index: r.u8()?,
            unknown_09: r.u8()?,
            flags: r.u8()?,
            sampler_state_count: r.u8()?,
            sampler_state_start_index: r.u16()?,
            unknown_0e: r.u16()?,
            unknown_10: r.u8()?,
            unknown_11: r.u8()?,
            unknown_12_0: r.u8()?,
            unknown_12_1: r.u8()?,
        })
    }

    /// The index into the effect's index list for a stage.
    pub fn shader_index(&self, stage: ShaderStage) -> u8 {
        match stage {
            ShaderStage::Vertex => self.vertex_shader_index,
            ShaderStage::Pixel => self.pixel_shader_index,
            ShaderStage::Geometry => self.geometry_shader_index,
            ShaderStage::Domain => self.domain_shader_index,
            ShaderStage::Hull => self.hull_shader_index,
            ShaderStage::Compute => self.compute_shader_index,
        }
    }
}

/// `FxdbSamplerStateRef`: a named sampler slot of a pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SamplerStateRef {
    pub name_offset: u8,
    /// Into the effect's `sampler_states`.
    pub sampler_state_index: u8,
    /// The name, from the effect's string table (`None` if the offset is bad).
    pub name: Option<String>,
}

/// A little-endian reader, like the C#'s `DataReader` on a `MemoryStream`.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let Some(b) = self.pos.checked_add(n).and_then(|end| self.data.get(self.pos..end)) else {
            bail!("fxdb: {n} bytes at {} past the end ({})", self.pos, self.data.len())
        };
        self.pos += n;
        Ok(b)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }
    /// `ReadString1bLen`: a byte length, then UTF-8 with trailing zeros trimmed.
    fn string_1b(&mut self) -> Result<String> {
        let len = self.u8()? as usize;
        let s = String::from_utf8_lossy(self.bytes(len)?);
        Ok(s.trim_end_matches('\0').to_owned())
    }
    /// `ReadStructArray`: an i32 count, then the items. Empty (null in the C#)
    /// for a count of 0 or less.
    fn struct_array<T>(&mut self, mut item: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let count = self.u32()? as i32;
        if count <= 0 {
            return Ok(Vec::new());
        }
        (0..count).map(|_| item(self)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::XmlWriter;

    /// A shader data with one constant buffer, holding one `float4` with a
    /// default value and a string annotation. The offsets are relative to the
    /// struct they're in, as in the game files.
    fn shader_data() -> Vec<u8> {
        let heap = b"cb\0v\0hi\0";
        let mut d = Vec::new();
        let data_length = 8 + 24 + 24 + 8 + heap.len() + 16;
        d.extend((data_length as u32).to_le_bytes());
        for v in [1u16, 1, 1, data_length as u16] {
            d.extend(v.to_le_bytes());
        }
        // The resource, at 8: a CBuffer of Buffer dimension; the element is at
        // 32, the annotation at 56, the heap at 64.
        d.extend([0x30, 4, 2, 0, 1, 0x41]);
        for v in [24u16, 56, 0, 16, 0] {
            d.extend(v.to_le_bytes());
        }
        d.extend([0; 8]);
        // The element, at 32: a Float4 with its default value after the heap.
        d.extend([12, 1]);
        for v in [0u16, 0, 32 + 3, 0, 32 + heap.len() as u16, 24, 0] {
            d.extend(v.to_le_bytes());
        }
        d.extend([0; 8]);
        // The annotation, at 56: a string.
        d.extend(((0x1234u32 << 2) | 2).to_le_bytes());
        d.extend((8u32 + 5).to_le_bytes());
        d.extend(heap);
        for f in [1.0f32, 0.5, 0.0, -2.0] {
            d.extend(f.to_le_bytes());
        }
        d
    }

    #[test]
    fn shader_data_offsets_resolve() {
        let bytes = shader_data();
        let hashes = HashIndex::new();
        let mut r = Reader { data: &bytes, pos: 0 };
        let d = ShaderData::read(&mut r, &hashes).unwrap();
        assert_eq!(r.pos, bytes.len());
        let res = &d.resources[0];
        assert_eq!((res.name.as_str(), res.resource_type(), res.resource_class()), ("cb", 0, 3));
        assert_eq!((res.srt_index(), res.unk(), res.unk_flag()), (1, 1, false));
        assert!(d.resource_annotations(res).is_empty());
        let ele = &d.resource_elements(res)[0];
        assert_eq!(ele.name, "v");
        assert_eq!(ele.default_value.as_deref(), Some(&bytes[bytes.len() - 16..]));
        let ann = &d.element_annotations(ele)[0];
        assert_eq!((ann.name_hash(), ann.value_as_string.as_deref()), (0x1234, Some("hi")));
        // The names are added to the hashes, like the C#'s `JenkIndex.Ensure`.
        assert_eq!(hashes.get(crate::hash::joaat("cb")).as_deref(), Some("cb"));

        let xml = XmlWriter::document(&hashes, "Data", None, |w| xml::write_shader_data(w, &d));
        assert!(xml.contains("<ResourceType>CBuffer</ResourceType>"), "{xml}");
        assert!(xml.contains("<SrtIndex>Unk1</SrtIndex>"), "{xml}");
        assert!(xml.contains("<DefaultValue>1 0.5 0 -2</DefaultValue>"), "{xml}");
        assert!(xml.contains("<Value>hi</Value>"), "{xml}");
    }

    #[test]
    fn bad_offsets_fail_like_the_csharp() {
        let mut bytes = shader_data();
        // The element's name offset points past the heap.
        bytes[4 + 32 + 6] = 200;
        let mut r = Reader { data: &bytes, pos: 0 };
        assert!(ShaderData::read(&mut r, &HashIndex::new()).is_err());
    }
}
