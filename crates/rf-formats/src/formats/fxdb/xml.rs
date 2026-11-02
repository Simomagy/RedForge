//! The fxdb XML, as the C#'s `Write(MetaNodeWriter)` methods write it.

use super::*;
use crate::xml::XmlWriter;

/// The names .NET gives the fxdb enums (`Enum.ToString`), for the XML.
pub mod enum_names {
    /// `Enum.ToString` of a plain enum: the name, or the number for values
    /// with no name.
    pub fn plain(name: Option<&str>, v: u8) -> String {
        name.map_or_else(|| v.to_string(), str::to_owned)
    }

    /// `Enum.ToString` of a `[Flags]` enum whose bit `n` is `names[n]` (and with
    /// no name for 0): the set flags' names, or the number when a bit has no name.
    pub fn flags(v: u8, names: &[&str]) -> String {
        if v == 0 || (v as u32) >> names.len() != 0 {
            return v.to_string();
        }
        let set: Vec<&str> = names.iter().enumerate().filter(|(i, _)| v & (1 << i) != 0).map(|(_, n)| *n).collect();
        set.join(", ")
    }

    fn at(names: &[&'static str], first: u8, v: u8) -> Option<&'static str> {
        let n = *names.get(v.checked_sub(first)? as usize)?;
        (!n.is_empty()).then_some(n)
    }

    const SEMANTICS: [&str; 0x5C] = [
        "POSITION",
        "POSITION1",
        "POSITION2",
        "POSITION3",
        "NORMAL",
        "NORMAL1",
        "NORMAL2",
        "NORMAL3",
        "TANGENT",
        "TANGENT1",
        "TANGENT2",
        "TANGENT3",
        "BINORMAL",
        "BINORMAL1",
        "BINORMAL2",
        "BINORMAL3",
        "BLENDWEIGHT",
        "BLENDWEIGHT1",
        "BLENDWEIGHT2",
        "BLENDWEIGHT3",
        "BLENDINDICES",
        "BLENDINDICES1",
        "BLENDINDICES2",
        "BLENDINDICES3",
        "COLOR",
        "COLOR1",
        "COLOR2",
        "COLOR3",
        "TEXCOORD",
        "TEXCOORD1",
        "TEXCOORD2",
        "TEXCOORD3",
        "TEXCOORD4",
        "TEXCOORD5",
        "TEXCOORD6",
        "TEXCOORD7",
        "TEXCOORD8",
        "TEXCOORD9",
        "TEXCOORD10",
        "TEXCOORD11",
        "TEXCOORD12",
        "TEXCOORD13",
        "TEXCOORD14",
        "TEXCOORD15",
        "TEXCOORD16",
        "TEXCOORD17",
        "TEXCOORD18",
        "TEXCOORD19",
        "TEXCOORD20",
        "TEXCOORD21",
        "TEXCOORD22",
        "TEXCOORD23",
        "SV_Position",
        "SV_InstanceID",
        "SV_PrimitiveID",
        "SV_VertexID",
        "SV_Depth",
        "SV_ViewportArrayIndex",
        "SV_SampleIndex",
        "SV_RenderTargetArrayIndex",
        "SV_TessFactor",
        "SV_InsideTessFactor",
        "SV_DomainLocation",
        "SV_IsFrontFace",
        "SV_GSInstanceID",
        "SV_OutputControlPointID",
        "SV_Coverage",
        "SV_DispatchThreadID",
        "SV_GroupID",
        "SV_GroupIndex",
        "SV_GroupThreadID",
        "SV_DepthGreaterEqual",
        "SV_DepthLessEqual",
        "",
        "",
        "",
        "SV_ClipDistance",
        "SV_ClipDistance1",
        "SV_ClipDistance2",
        "SV_ClipDistance3",
        "SV_CullDistance",
        "SV_CullDistance1",
        "SV_CullDistance2",
        "SV_CullDistance3",
        "SV_Target",
        "SV_Target1",
        "SV_Target2",
        "SV_Target3",
        "SV_Target4",
        "SV_Target5",
        "SV_Target6",
        "SV_Target7",
    ];

    /// `FxdbIndexedSemantic` (`rage::sga::IndexedSemantic`).
    pub fn indexed_semantic(v: u8) -> Option<&'static str> {
        at(&SEMANTICS, 0, v)
    }

    /// `FxdbBufferFormat` (`rage::sga::BufferFormat`).
    pub fn buffer_format(v: u8) -> Option<&'static str> {
        Some(match v {
            0 => "NONE",
            1 => "R32G32B32A32_TYPELESS",
            2 => "R32G32B32A32_FLOAT",
            3 => "R32G32B32A32_UINT",
            4 => "R32G32B32A32_SINT",
            5 => "R32G32B32_TYPELESS",
            6 => "R32G32B32_FLOAT",
            7 => "R32G32B32_UINT",
            8 => "R32G32B32_SINT",
            9 => "R16G16B16A16_TYPELESS",
            10 => "R16G16B16A16_FLOAT",
            11 => "R16G16B16A16_UNORM",
            12 => "R16G16B16A16_UINT",
            13 => "R16G16B16A16_SNORM",
            14 => "R16G16B16A16_SINT",
            15 => "R32G32_TYPELESS",
            16 => "R32G32_FLOAT",
            17 => "R32G32_UINT",
            18 => "R32G32_SINT",
            20 => "D32_FLOAT_S8X24_UINT",
            21 => "B10G10R10A2_UNORM",
            22 => "R10G10B10A2_SNORM",
            23 => "R10G10B10A2_TYPELESS",
            24 => "R10G10B10A2_UNORM",
            25 => "R10G10B10A2_UINT",
            26 => "R11G11B10_FLOAT",
            27 => "R8G8B8A8_TYPELESS",
            28 => "R8G8B8A8_UNORM",
            29 => "R8G8B8A8_UNORM_SRGB",
            30 => "R8G8B8A8_UINT",
            31 => "R8G8B8A8_SNORM",
            32 => "R8G8B8A8_SINT",
            33 => "R16G16_TYPELESS",
            34 => "R16G16_FLOAT",
            35 => "R16G16_UNORM",
            36 => "R16G16_UINT",
            37 => "R16G16_SNORM",
            38 => "R16G16_SINT",
            39 => "R32_TYPELESS",
            40 => "D32_FLOAT",
            41 => "R32_FLOAT",
            42 => "R32_UINT",
            43 => "R32_SINT",
            48 => "R8G8_TYPELESS",
            49 => "R8G8_UNORM",
            50 => "R8G8_UINT",
            51 => "R8G8_SNORM",
            52 => "R8G8_SINT",
            53 => "R16_TYPELESS",
            54 => "R16_FLOAT",
            55 => "D16_UNORM",
            56 => "R16_UNORM",
            57 => "R16_UINT",
            58 => "R16_SNORM",
            59 => "R16_SINT",
            60 => "R8_TYPELESS",
            61 => "R8_UNORM",
            62 => "R8_UINT",
            63 => "R8_SNORM",
            64 => "R8_SINT",
            65 => "A8_UNORM",
            67 => "R9G9B9E5_SHAREDEXP",
            70 => "BC1_TYPELESS",
            71 => "BC1_UNORM",
            72 => "BC1_UNORM_SRGB",
            73 => "BC2_TYPELESS",
            74 => "BC2_UNORM",
            75 => "BC2_UNORM_SRGB",
            76 => "BC3_TYPELESS",
            77 => "BC3_UNORM",
            78 => "BC3_UNORM_SRGB",
            79 => "BC4_TYPELESS",
            80 => "BC4_UNORM",
            81 => "BC4_SNORM",
            82 => "BC5_TYPELESS",
            83 => "BC5_UNORM",
            84 => "BC5_SNORM",
            85 => "B5G6R5_UNORM",
            86 => "B5G5R5A1_UNORM",
            87 => "B8G8R8A8_UNORM",
            90 => "B8G8R8A8_TYPELESS",
            91 => "B8G8R8A8_UNORM_SRGB",
            94 => "BC6H_TYPELESS",
            95 => "BC6H_UF16",
            96 => "BC6H_SF16",
            97 => "BC7_TYPELESS",
            98 => "BC7_UNORM",
            99 => "BC7_UNORM_SRGB",
            103 => "NV12",
            115 => "B4G4R4A4_UNORM",
            118 => "D16_UNORM_S8_UINT",
            119 => "R16_UNORM_X8_TYPELESS",
            120 => "X16_TYPELESS_G8_UINT",
            121 => "ETC1",
            122 => "ETC1_SRGB",
            123 => "ETC1A",
            124 => "ETC1A_SRGB",
            127 => "R4G4_UNORM",
            _ => return None,
        })
    }

    /// `FxdbResourceType` (`rage::sga::ResourceType`).
    pub fn resource_type(v: u8) -> Option<&'static str> {
        const N: [&str; 14] = [
            "CBuffer",
            "Typed",
            "Texture",
            "Sampler",
            "UavRwTyped",
            "Structured",
            "UavRwStructured",
            "ByteAddress",
            "UavRwByteAddress",
            "UavAppendStructured",
            "UavConsumeStructured",
            "UavRwStructuredWithCounter",
            "UavRwTexture",
            "Invalid",
        ];
        at(&N, 0, v)
    }

    /// `FxdbResourceClass` (`rage::sga::ResourceClass`).
    pub fn resource_class(v: u8) -> Option<&'static str> {
        at(&["ShaderResource", "UnorderedAccess", "Sampler", "CBuffer"], 0, v)
    }

    /// `FxdbSrtIndex` (`rage::sga::SrtIndex`).
    pub fn srt_index(v: u8) -> Option<&'static str> {
        at(&["Unk0", "Unk1"], 0, v)
    }

    /// `FxdbImageDimension` (`rage::sga::ImageDimension`).
    pub fn image_dimension(v: u8) -> Option<&'static str> {
        at(&["Texture1D", "Texture2D", "TextureCUBE", "Texture3D", "Buffer", "Invalid"], 0, v)
    }

    /// `FxdbVariableType` (`rage::sga::VariableType`).
    pub fn variable_type(v: u8) -> Option<&'static str> {
        const N: [&str; 16] = [
            "Bool", "Int", "Int2", "Int3", "Int4", "UInt", "UInt2", "UInt3", "UInt4", "Float", "Float2", "Float3",
            "Float4", "Matrix34", "Matrix44", "Invalid",
        ];
        at(&N, 0, v)
    }

    /// `FxdbShaderAnnotationType`.
    pub fn annotation_type(v: u8) -> Option<&'static str> {
        at(&["Int", "Float", "String"], 0, v)
    }

    /// `FxdbFillMode` (`D3D12_FILL_MODE`).
    pub fn fill_mode(v: u8) -> Option<&'static str> {
        at(&["Wireframe", "Solid"], 2, v)
    }

    /// `FxdbCullMode` (`D3D12_CULL_MODE`).
    pub fn cull_mode(v: u8) -> Option<&'static str> {
        at(&["None", "Front", "Back"], 1, v)
    }

    /// `FxdbComparisonFunc` (`D3D12_COMPARISON_FUNC`).
    pub fn comparison_func(v: u8) -> Option<&'static str> {
        at(&["Never", "Less", "Equal", "LessEqual", "Greater", "NotEqual", "GreaterEqual", "Always"], 1, v)
    }

    /// `FxdbStencilOp` (`D3D12_STENCIL_OP`).
    pub fn stencil_op(v: u8) -> Option<&'static str> {
        const N: [&str; 8] = [
            "Keep",
            "Zero",
            "Replace",
            "IncrementAndClamp",
            "DecrementAndClamp",
            "Invert",
            "IncrementAndWrap",
            "DecrementAndWrap",
        ];
        at(&N, 1, v)
    }

    /// `FxdbBlendOp` (`D3D12_BLEND_OP`).
    pub fn blend_op(v: u8) -> Option<&'static str> {
        at(&["Add", "Subtract", "ReverseSubtract", "Min", "Max"], 1, v)
    }

    /// `FxdbBlendFactor` (`D3D12_BLEND`).
    pub fn blend_factor(v: u8) -> Option<&'static str> {
        const N: [&str; 21] = [
            "Zero",
            "One",
            "SrcColor",
            "InvSrcColor",
            "SrcAlpha",
            "InvSrcAlpha",
            "DestAlpha",
            "InvDestAlpha",
            "DestColor",
            "InvDestColor",
            "SrcAlphaSat",
            "",
            "",
            "BlendFactor",
            "InvBlendFactor",
            "Src1Color",
            "InvSrc1Color",
            "Src1Alpha",
            "InvSrc1Alpha",
            "AlphaFactor",
            "InvAlphaFactor",
        ];
        at(&N, 1, v)
    }

    /// `FxdbTextureAddressMode` (`D3D12_TEXTURE_ADDRESS_MODE`).
    pub fn texture_address_mode(v: u8) -> Option<&'static str> {
        at(&["Wrap", "Mirror", "Clamp", "Border", "MirrorOnce"], 1, v)
    }

    /// The bits of `FxdbColorWriteEnable`, for [`flags`].
    pub const COLOR_WRITE_ENABLE: [&str; 4] = ["Red", "Green", "Blue", "Alpha"];

    /// The bits of `FxdbRenderShaderSetFlags`, for [`flags`].
    pub const RENDER_SHADER_SET_FLAGS: [&str; 7] = [
        "HasRasterizerState",
        "HasDepthStencilState",
        "HasBlendState",
        "HasUnknown09",
        "Unk4",
        "DepthStencilStateIndexExtraBit",
        "BlendStateIndexExtraBit",
    ];
}

use enum_names as names;

/// `XmlMetaNodeWriter.GetXml("RDR2EffectsDatabase", fxdb)`.
pub fn to_xml_string(db: &FxdbFile, hashes: &HashIndex) -> String {
    XmlWriter::document(hashes, "RDR2EffectsDatabase", None, |w| {
        w.u32("@version", 0);
        for (id, shaders) in [
            ("VertexShaders", &db.vertex_shaders),
            ("PixelShaders", &db.pixel_shaders),
            ("GeometryShaders", &db.geometry_shaders),
            ("DomainShaders", &db.domain_shaders),
            ("HullShaders", &db.hull_shaders),
            ("ComputeShaders", &db.compute_shaders),
        ] {
            w.node_array(id, Some(shaders), write_shader);
        }
        w.node_array("Effects", Some(&db.effects), |w, e| write_effect(w, db, e));
    })
}

/// `WriteEnum` of a plain enum.
fn enum_(w: &mut XmlWriter, id: &str, name: fn(u8) -> Option<&'static str>, v: u8) {
    w.string(id, &names::plain(name(v), v));
}

/// `WriteNodeArray` of a C# array that is null when empty.
fn nonempty<T>(v: &[T]) -> Option<&[T]> {
    (!v.is_empty()).then_some(v)
}

fn write_shader(w: &mut XmlWriter, s: &Shader) {
    w.string("Name", &s.name);
    w.u8("FeatureLevel", s.feature_level);
    w.node("Usage", |w| {
        w.node("Inputs", |w| write_usage_mask(w, &s.usage.inputs));
        w.node("Outputs", |w| write_usage_mask(w, &s.usage.outputs));
    });
    w.node("Data", |w| write_shader_data(w, &s.data));
}

fn write_usage_mask(w: &mut XmlWriter, m: &ShaderUsageMask) {
    let semantics: Vec<String> =
        m.semantics().into_iter().map(|s| names::plain(names::indexed_semantic(s), s)).collect();
    w.string("Semantics", &semantics.join(", "));
    let formats: Vec<String> =
        m.semantics_formats.iter().filter(|&&f| f != 0).map(|&f| names::plain(names::buffer_format(f), f)).collect();
    w.string("Formats", &formats.join(", "));
}

pub(super) fn write_shader_data(w: &mut XmlWriter, d: &ShaderData) {
    // Elements are written in their resources, annotations in both.
    w.node_array("Resources", Some(&d.resources), |w, res| {
        w.string("Name", &res.name);
        w.opt_string("SemanticName", res.semantic_name.as_deref());
        enum_(w, "ResourceType", names::resource_type, res.resource_type());
        enum_(w, "ResourceClass", names::resource_class, res.resource_class());
        enum_(w, "ImageDimension", names::image_dimension, res.image_dimension);
        w.u8("BindPoint", res.bind_point);
        enum_(w, "SrtIndex", names::srt_index, res.srt_index());
        w.u8("Unk", res.unk());
        w.bool("UnkFlag", res.unk_flag());
        if res.byte_size != 0 {
            w.u16("ByteSize", res.byte_size);
        }
        w.node_array("Elements", nonempty(d.resource_elements(res)), |w, e| write_element(w, d, e));
        w.node_array("Annotations", nonempty(d.resource_annotations(res)), write_annotation);
    });
}

fn write_element(w: &mut XmlWriter, d: &ShaderData, e: &ShaderElementData) {
    w.string("Name", &e.name);
    w.opt_string("SemanticName", e.semantic_name.as_deref());
    enum_(w, "Type", names::variable_type, e.type_);
    if e.array_size != 0 {
        w.u16("ArraySize", e.array_size);
    }
    w.u16("StartOffset", e.start_offset);
    if let Some(v) = &e.default_value {
        let words = v.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c));
        match e.type_ {
            0..=4 => w.i32_array("DefaultValue", Some(&words.map(|x| x as i32).collect::<Vec<_>>())),
            5..=8 => w.u32_array("DefaultValue", Some(&words.collect::<Vec<_>>())),
            9..=14 => w.f32_array("DefaultValue", Some(&words.map(f32::from_bits).collect::<Vec<_>>())),
            _ => w.byte_array("DefaultValue", Some(v)),
        }
    }
    w.node_array("Annotations", nonempty(d.element_annotations(e)), write_annotation);
}

fn write_annotation(w: &mut XmlWriter, a: &ShaderAnnotation) {
    w.u32("NameHash", a.name_hash());
    enum_(w, "Type", names::annotation_type, a.annotation_type());
    match a.annotation_type() {
        ShaderAnnotation::INT => w.i32("Value", a.value_as_int()),
        ShaderAnnotation::FLOAT => w.f32("Value", a.value_as_float()),
        ShaderAnnotation::STRING => w.string("Value", a.value_as_string.as_deref().unwrap_or("")),
        _ => {}
    }
}

fn write_effect(w: &mut XmlWriter, db: &FxdbFile, e: &Effect) {
    w.string("Name", &e.name);
    w.node_array("RasterizerStates", nonempty(&e.rasterizer_states), write_rasterizer_state);
    w.node_array("DepthStencilStates", nonempty(&e.depth_stencil_states), write_depth_stencil_state);
    w.node_array("BlendStates", nonempty(&e.blend_states), write_blend_state);
    w.node_array("SamplerStates", nonempty(&e.sampler_states), write_sampler_state);
    w.node_array("RenderShaderSets", nonempty(&e.render_shader_sets), |w, p| write_render_shader_set(w, db, e, p));
    w.node_array("Techniques", nonempty(&e.techniques), |w, t| {
        w.string("Name", &t.name);
        w.u32("RenderShaderSetStartIndex", t.render_shader_set_start_index());
        w.u32("RenderShaderSetCount", t.render_shader_set_count());
    });
    w.node("ShaderData", |w| write_shader_data(w, &e.shader_data));
    w.node("PropertiesShaderData", |w| write_shader_data(w, &e.properties_shader_data));

    w.byte_array("UnkStruct", Some(&e.unk_struct));
    w.string("Unk1", &format!("0x{:016X}", e.unk1));
    if !e.unks.is_empty() {
        w.i32("RenderShaderSetsCount", e.render_shader_sets.len() as i32);
        w.i32("TechniquesCount", e.techniques.len() as i32);
        w.i32("UnksParsedCount", e.unks.len() as i32);
        let hashes = w.hashes;
        let lists: Vec<String> =
            e.unks.iter().map(|list| list.iter().map(|&h| hashes.xml_name(h)).collect::<Vec<_>>().join(" ")).collect();
        w.string_array("UnksParsed", Some(&lists));
    }
}

fn write_rasterizer_state(w: &mut XmlWriter, s: &RasterizerStateDesc) {
    enum_(w, "FillMode", names::fill_mode, s.fill_mode);
    enum_(w, "CullMode", names::cull_mode, s.cull_mode);
    w.bool("FrontCounterClockwise", s.front_counter_clockwise);
    w.i8("DepthBias", s.depth_bias);
    w.f32("DepthBiasClamp", s.depth_bias_clamp);
    w.f32("SlopeScaledDepthBias", s.slope_scaled_depth_bias);
    w.bool("DepthClipEnable", s.depth_clip_enable);
    w.u8("Unknown0D", s.unknown_0d);
    w.bool("MultisampleEnable", s.multisample_enable);
    w.bool("AntialiasedLineEnable", s.antialiased_line_enable);
}

fn write_stencil_op(w: &mut XmlWriter, s: &StencilOpDesc) {
    enum_(w, "StencilFailOp", names::stencil_op, s.stencil_fail_op);
    enum_(w, "StencilDepthFailOp", names::stencil_op, s.stencil_depth_fail_op);
    enum_(w, "StencilPassOp", names::stencil_op, s.stencil_pass_op);
    enum_(w, "StencilFunc", names::comparison_func, s.stencil_func);
}

fn write_depth_stencil_state(w: &mut XmlWriter, s: &DepthStencilStateDesc) {
    w.bool("DepthEnable", s.depth_enable);
    w.bool("DepthWriteEnable", s.depth_write_enable);
    enum_(w, "DepthFunc", names::comparison_func, s.depth_func);
    w.bool("StencilEnable", s.stencil_enable);
    w.u8("StencilReadMask", s.stencil_read_mask);
    w.u8("StencilWriteMask", s.stencil_write_mask);
    w.node("FrontFace", |w| write_stencil_op(w, &s.front_face));
    w.node("BackFace", |w| write_stencil_op(w, &s.back_face));
    w.bool("HasDifferentBackFace", s.has_different_back_face);
    w.bool("DepthBoundsTestEnable", s.depth_bounds_test_enable);
    w.u8("Unknown10", s.unknown_10);
    w.u8("Unknown11", s.unknown_11);
    w.u8("BackFaceStencilReadMask", s.back_face_stencil_read_mask);
    w.u8("BackFaceStencilWriteMask", s.back_face_stencil_write_mask);
    w.i32("Unknown14", s.unknown_14);
}

fn write_blend_state(w: &mut XmlWriter, s: &BlendStateDesc) {
    w.bool("AlphaToCoverageEnable", s.alpha_to_coverage_enable);
    w.bool("IndependentBlendEnable", s.independent_blend_enable);
    w.u8("Unknown02", s.unknown_02);
    w.u8("Unknown03", s.unknown_03);
    w.node_array("RenderTargets", Some(&s.render_targets), |w, rt| {
        w.bool("BlendEnable", rt.blend_enable);
        enum_(w, "SrcBlend", names::blend_factor, rt.src_blend);
        enum_(w, "DestBlend", names::blend_factor, rt.dest_blend);
        enum_(w, "BlendOp", names::blend_op, rt.blend_op);
        enum_(w, "SrcBlendAlpha", names::blend_factor, rt.src_blend_alpha);
        enum_(w, "DestBlendAlpha", names::blend_factor, rt.dest_blend_alpha);
        enum_(w, "BlendOpAlpha", names::blend_op, rt.blend_op_alpha);
        w.string("RenderTargetWriteMask", &names::flags(rt.render_target_write_mask, &names::COLOR_WRITE_ENABLE));
    });
}

fn write_sampler_state(w: &mut XmlWriter, s: &SamplerStateDesc) {
    w.u8("Filter", s.filter);
    enum_(w, "AddressU", names::texture_address_mode, s.address_u);
    enum_(w, "AddressV", names::texture_address_mode, s.address_v);
    enum_(w, "AddressW", names::texture_address_mode, s.address_w);
    w.f32("MipLodBias", s.mip_lod_bias);
    w.u8("MaxAnisotropy", s.max_anisotropy);
    enum_(w, "ComparisonFunc", names::comparison_func, s.comparison_func);
    w.u8("Unknown0A", s.unknown_0a);
    w.u8("Unknown0B", s.unknown_0b);
    w.colourf("BorderColour", s.border_colour);
    w.f32("MinLod", s.min_lod);
    w.f32("MaxLod", s.max_lod);
}

fn write_render_shader_set(w: &mut XmlWriter, db: &FxdbFile, e: &Effect, p: &RenderShaderSetDesc) {
    for (id, stage) in [
        ("VertexShader", ShaderStage::Vertex),
        ("PixelShader", ShaderStage::Pixel),
        ("GeometryShader", ShaderStage::Geometry),
        ("DomainShader", ShaderStage::Domain),
        ("HullShader", ShaderStage::Hull),
        ("ComputeShader", ShaderStage::Compute),
    ] {
        w.string(id, db.pass_shader(e, p, stage).map_or("", |s| &s.name));
    }
    w.u8("RasterizerStateIndex", p.rasterizer_state_index);
    w.u8("DepthStencilStateIndex", p.depth_stencil_state_index);
    w.u8("BlendStateIndex", p.blend_state_index);
    w.u8("Unknown09", p.unknown_09);
    w.string("Flags", &names::flags(p.flags, &names::RENDER_SHADER_SET_FLAGS));
    // Checked when reading: the refs are there and have names.
    let refs = if p.sampler_state_count == 0 { None } else { e.pass_sampler_refs(p) };
    w.node_array("SamplerStates", refs, |w, s| {
        w.string("@name", s.name.as_deref().unwrap_or(""));
        w.u8("@index", s.sampler_state_index);
    });
    w.u16("Unknown0E", p.unknown_0e);
    w.u8("Unknown10", p.unknown_10);
    w.u8("Unknown11", p.unknown_11);
}

#[cfg(test)]
mod tests {
    use super::enum_names::*;
    use super::*;

    #[test]
    fn enums_format_like_dotnet() {
        assert_eq!(plain(indexed_semantic(0x34), 0x34), "SV_Position");
        assert_eq!(plain(indexed_semantic(0x4A), 0x4A), "74");
        assert_eq!(plain(indexed_semantic(0x5B), 0x5B), "SV_Target7");
        assert_eq!(plain(blend_factor(14), 14), "BlendFactor");
        assert_eq!(plain(blend_factor(12), 12), "12");
        assert_eq!(plain(cull_mode(0), 0), "0");
        assert_eq!(flags(15, &COLOR_WRITE_ENABLE), "Red, Green, Blue, Alpha");
        assert_eq!(flags(0, &COLOR_WRITE_ENABLE), "0");
        assert_eq!(flags(0x15, &COLOR_WRITE_ENABLE), "21");
        assert_eq!(flags(5, &RENDER_SHADER_SET_FLAGS), "HasRasterizerState, HasBlendState");
        assert_eq!(flags(0x81, &RENDER_SHADER_SET_FLAGS), "129");
    }

    #[test]
    fn semantics_pad_unknown_bits_with_position() {
        let m = ShaderUsageMask { semantics_mask: (1 << 4) | (1 << 60), sv_semantics_mask: 1, ..Default::default() };
        assert_eq!(m.semantics(), [4, SV_FIRST, 0]);
    }

    #[test]
    fn hash_lists_parse_like_the_csharp() {
        assert_eq!(parse_unks(&[2, 10, 11, 0, 1, 12]).unwrap(), [vec![10, 11], vec![], vec![12]]);
        // A list running past the end is padded.
        assert_eq!(parse_unks(&[3, 10]).unwrap(), [vec![10, 0, 0]]);
    }
}
