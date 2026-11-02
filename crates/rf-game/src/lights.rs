//! The lights archetypes carry: their `CExtensionDefLightEffect` extensions,
//! read as the C# RedForge's `RDR2Light` reads them.

use rf_formats::formats::databag::DataBag;

/// How a light shines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    Point,
    Spot,
    /// Along a segment (`extents.y` either side of `position`, along `direction`).
    Capsule,
}

/// One light of an archetype (a `CLightAttrDef`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightEffect {
    pub kind: LightKind,
    /// In the archetype's space with the extension's offset, or its bone's when
    /// `bone_tag` names one.
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub tangent: [f32; 3],
    /// The colour times the intensity, in the game's units (a lantern is
    /// about 15; the moon 1).
    pub colour: [f32; 3],
    /// How far it reaches ("falloff"), and how sharply it fades there.
    pub range: f32,
    pub falloff_exponent: f32,
    /// A spot light's cone, half angles in radians.
    pub cone_inner: f32,
    pub cone_outer: f32,
    /// Half the size of a capsule.
    pub extents: [f32; 3],
    /// Bit `h` set when it's on at hour `h`.
    pub time_flags: u32,
    pub flags: u32,
    pub bone_tag: u16,
}

/// `CExtensionDefLightEffect`'s class name hash.
const LIGHT_EFFECT: u32 = 0x2792_2C43;

impl LightEffect {
    /// `IsActive`: on at an hour (0-23), unless disabled.
    pub fn active_at(&self, hour: u32) -> bool {
        self.flags & 0x8000 == 0 && (hour > 23 || (self.time_flags >> hour) & 1 == 1)
    }

    /// `RDR2Light`'s constructor.
    fn read(b: &DataBag, offset: [f32; 3]) -> Self {
        let posn = b.vec3(0x39A6_25E4);
        let [r, g, bl] = b.get_struct::<[u8; 3]>(0x5BDC_13E3); // colour
        let intensity = b.f32(0xEFCD_993D);
        let (inner, outer) = (b.f32(0x455C_3938), b.f32(0xF8D9_E344));
        let kind = match b.u8(0x1CBB_BE30) {
            2 => LightKind::Spot,
            4 => LightKind::Capsule,
            _ => LightKind::Point,
        };
        Self {
            kind,
            position: [posn[0] + offset[0], posn[1] + offset[1], posn[2] + offset[2]],
            direction: b.vec3(0x34DF_9828),
            tangent: b.vec3(0x8E6F_0BA9),
            colour: [r, g, bl].map(|c| c as f32 * intensity / 255.0),
            range: b.f32(0xCC68_06E8),
            falloff_exponent: b.f32(0x2BB3_42BA),
            cone_inner: inner.min(outer).to_radians(),
            cone_outer: inner.max(outer).to_radians(),
            extents: b.vec3(0x2D3F_79C0).map(|e| e * 0.5),
            time_flags: b.u32(0x8609_D52C),
            flags: b.u32(0x67D2_6872),
            bone_tag: b.i16(0x29D7_F7B0) as u16,
        }
    }
}

/// `EnsureLightInstances`: an archetype's lights, from its extensions.
pub fn archetype_lights(archetype: &DataBag) -> Vec<LightEffect> {
    let mut out = Vec::new();
    for ext in archetype.bags(0xF17E_7F28).into_iter().flatten().flatten() {
        if ext.class_name() != LIGHT_EFFECT {
            continue;
        }
        let offset = ext.vec3(0xD895_0703);
        for light in ext.bags(0x1057_9DF2).into_iter().flatten().flatten() {
            out.push(LightEffect::read(light, offset));
        }
    }
    out
}
