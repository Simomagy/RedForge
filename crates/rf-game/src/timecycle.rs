//! A weather's lighting through the day, from the game's timecycle files
//! (`common:/data/timecycle/w_<weather>.xml`), as the C# RedForge's
//! `RDR2TimeCycle` reads them: each value has 13 keyframes a day, blended in
//! between, and the sun follows `common:/data/levels/rdr3/time.xml`'s path.

use std::collections::HashMap;

use rf_formats::rpf8::Rpf8FileSystem;
use rf_formats::xml::reader::XmlDocument;

/// The weathers there are timecycles for.
pub const WEATHERS: [&str; 22] = [
    "sunny",
    "highpressure",
    "clouds",
    "overcast",
    "overcastdark",
    "misty",
    "fog",
    "drizzle",
    "shower",
    "rain",
    "thunder",
    "thunderstorm",
    "hurricane",
    "sandstorm",
    "hail",
    "sleet",
    "snowlight",
    "snow",
    "snowclearing",
    "blizzard",
    "groundblizzard",
    "whiteout",
];

/// The keyframes' hours, and where the sun is then (radians from sunrise): it
/// rises at 7 and sets at 19, slowly around both. Unwrapped, so it keeps going
/// round through the night.
const HOURS: [f32; 13] = [0.0, 4.0, 5.0, 6.0, 7.0, 9.0, 12.0, 16.0, 18.0, 19.0, 20.0, 21.0, 22.0];
#[allow(clippy::approx_constant)] // 3.14 is time.xml's value, not pi
const SUN_ANGLES: [f32; 13] = [-1.57, -0.45, -0.22, -0.04, 0.00, 0.30, 1.57, 2.57, 3.09, 3.14, 3.19, 3.51, 3.86];

/// `RDR2Light.LuxScale`: the timecycle's light values to the engine's.
pub const LUX_SCALE: f32 = 0.01;

/// The lighting at an hour.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimeCycleState {
    /// Radians along the sun's path: 0 at sunrise, pi/2 at noon, pi at sunset.
    pub sun_angle: f32,
    /// The sun's (or moon's) light colour, times its multiplier.
    pub light_colour: [f32; 3],
    pub sky_sun_intensity: f32,
    pub sky_moon_intensity: f32,
    /// Ambient light added to the sky's.
    pub fill_ambient: [f32; 3],
    /// The night sky's own glow.
    pub sky_glow: [f32; 3],
    /// The moon's light, before the timecycle's colour.
    pub moon_intensity: f32,
    /// Added to the exposure (night is darker), in EV.
    pub exposure_ev: f32,
    /// Night vision: the blue grey colours fade to in the dark (a luminance
    /// of 1), and how much (0 by day).
    pub scotopic: [f32; 4],
    /// 0 by day, 1 at night (smooth through dusk and dawn).
    pub night: f32,
}

/// The C# RedForge's settings for the night (`RDR2.NightExposure`,
/// `RDR2.NightVision`, `RDR2.MoonLight`, `RDR2.SkyGlow`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NightSettings {
    /// How much darker night is than day, in EV.
    pub exposure: f32,
    /// Multiplies how much colours fade to blue grey in the dark.
    pub vision: f32,
    /// The moon's brightness before the timecycle's: the game's 1 (its
    /// `visualsettings.dat`). The C#'s default is 0.1, about a real full moon,
    /// which leaves RDR2's nights to the blue fill light.
    pub moon: f32,
    /// Multiplies the night sky's own glow.
    pub sky_glow: f32,
}

impl Default for NightSettings {
    fn default() -> Self {
        Self { exposure: -3.0, vision: 1.0, moon: 1.0, sky_glow: 1.0 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TimeCycle {
    pub weather: String,
    /// The `GLOBAL` region's values, by name.
    values: HashMap<String, [f32; 13]>,
}

impl TimeCycle {
    /// A weather's timecycle: the patched file if there is one. Empty (every
    /// value its default) when not found.
    pub fn load(fs: &Rpf8FileSystem, weather: &str) -> Self {
        let name = format!("w_{weather}.xml");
        let text =
            [format!("update_1.rpf/common/data/timecycle/{name}"), format!("common_0.rpf/data/timecycle/{name}")]
                .iter()
                .find_map(|p| fs.extract(fs.find(p)?).ok())
                .map(|b| String::from_utf8_lossy(&b).into_owned());
        let mut tc = TimeCycle { weather: weather.to_owned(), values: HashMap::new() };
        match text {
            Some(text) => tc.read(&text),
            None => log::warn!("timecycle {name} not found: using the default lighting"),
        }
        tc
    }

    fn read(&mut self, text: &str) {
        let doc = XmlDocument::new(text);
        let result =
            doc.with_root(None, |root| {
                let Some(region) = root.node.descendants().find(|n| {
                    n.is_element() && n.tag_name().name() == "region" && n.attribute("name") == Some("GLOBAL")
                }) else {
                    return;
                };
                for e in region.children().filter(|n| n.is_element()) {
                    let text: String = e.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect();
                    let values: Vec<f32> = text.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                    if let Ok(v) = <[f32; 13]>::try_from(values) {
                        self.values.insert(e.tag_name().name().to_owned(), v);
                    }
                }
            });
        if let Err(e) = result {
            log::warn!("timecycle {}: {e}", self.weather);
        }
    }

    /// The keyframes around an hour and how far between them.
    fn keyframes(hour: f32) -> (usize, usize, f32) {
        let hour = hour.rem_euclid(24.0);
        let mut i0 = HOURS.len() - 1; // after 22, towards 0 the next day
        while i0 > 0 && HOURS[i0] > hour {
            i0 -= 1;
        }
        let i1 = (i0 + 1) % HOURS.len();
        let (h0, h1) = (HOURS[i0], if i1 == 0 { 24.0 } else { HOURS[i1] });
        (i0, i1, (hour - h0) / (h1 - h0))
    }

    pub fn get(&self, name: &str, hour: f32, default: f32) -> f32 {
        let Some(v) = self.values.get(name) else { return default };
        let (i0, i1, t) = Self::keyframes(hour);
        v[i0] + (v[i1] - v[i0]) * t
    }

    fn rgb(&self, prefix: &str, hour: f32, default: f32) -> [f32; 3] {
        ["r", "g", "b"].map(|c| self.get(&format!("{prefix}_{c}"), hour, default))
    }

    /// `GetState`: the lighting at an hour (0-24).
    pub fn state(&self, hour: f32) -> TimeCycleState {
        self.state_with(hour, &NightSettings::default())
    }

    /// The lighting at an hour, as the C#'s `RDR2TimeCycle.GetState`.
    pub fn state_with(&self, hour: f32, night_settings: &NightSettings) -> TimeCycleState {
        let (i0, i1, t) = Self::keyframes(hour);
        let a1 = if i1 == 0 { SUN_ANGLES[0] + std::f32::consts::TAU } else { SUN_ANGLES[i1] };
        let sun_angle = SUN_ANGLES[i0] + (a1 - SUN_ANGLES[i0]) * t;
        let mult = self.get("light_dir_mult", hour, 1.0);
        let fill = self.get("local_atmosphere_fill_ambient_light_intensity", hour, 1.0)
            * self.get("ambient_sky_intensity", hour, 1.0)
            * LUX_SCALE;
        let glow = self.get("sky_air_glow_base_intensity", hour, 0.0) * LUX_SCALE * night_settings.sky_glow;
        // Dusk from the sun about 6° high, halfway at sunset, full night 6° below.
        let night = ((0.1 - sun_angle.sin()) / 0.2).clamp(0.0, 1.0);
        let night = night * night * (3.0 - 2.0 * night);
        TimeCycleState {
            sun_angle,
            light_colour: self.rgb("light_dir_col", hour, 1.0).map(|c| c * mult),
            sky_sun_intensity: self.get("sky_scattering_sun_intensity_multiplier", hour, 1.0),
            sky_moon_intensity: self.get("sky_scattering_moon_intensity_multiplier", hour, 1.0),
            fill_ambient: self.rgb("local_atmosphere_fill_ambient_light_color", hour, 0.0).map(|c| c * fill),
            sky_glow: self.rgb("sky_air_glow_base_color", hour, 0.0).map(|c| c * glow),
            moon_intensity: LUX_SCALE * night_settings.moon,
            exposure_ev: self.get("postfx_exposure", hour, 0.0) + night_settings.exposure * night,
            scotopic: {
                let [r, g, b] = [("r", 0.37), ("g", 0.54), ("b", 1.0)]
                    .map(|(c, d)| self.get(&format!("postfx_scotopic_col_{c}"), hour, d));
                let lum = (r * 0.2126 + g * 0.7152 + b * 0.0722).max(0.01);
                let amount =
                    self.get("postfx_scotopic_blue_shift_intensity", hour, 0.7) * night * night_settings.vision;
                [r / lum, g / lum, b / lum, amount]
            },
            night,
        }
    }

    /// The direction towards the sun at a sun angle, in the world (Z up): it
    /// rises in the east and stands in the south at noon.
    pub fn sun_direction(sun_angle: f32) -> [f32; 3] {
        // The C#'s light travels along +Y turned by the inverse of the planet's
        // orientation (sun_angle + 1.5 pi about Y, after 66.5° about X):
        // rot_y(-a) * rot_x(-tilt) * Y. Towards the sun is the other way.
        let (s, c) = (sun_angle + 1.5 * std::f32::consts::PI).sin_cos();
        let (st, ct) = 66.5f32.to_radians().sin_cos();
        let y = [0.0, ct, -st];
        let travel = [y[0] * c - y[2] * s, y[1], y[0] * s + y[2] * c];
        travel.map(|v| -v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_blend_between_keyframes() {
        let mut tc = TimeCycle::default();
        tc.read(
            r#"<timecycle_keyframe_data><cycle><region name="GLOBAL">
            <light_dir_mult>0 0 0 0 1 1 2 2 2 1 0 0 0</light_dir_mult>
            <too_short>1 2 3</too_short>
            </region></cycle></timecycle_keyframe_data>"#,
        );
        assert_eq!(tc.get("light_dir_mult", 12.0, -1.0), 2.0);
        assert_eq!(tc.get("light_dir_mult", 8.0, -1.0), 1.0);
        assert_eq!(tc.get("light_dir_mult", 10.5, -1.0), 1.5);
        assert_eq!(tc.get("too_short", 12.0, -1.0), -1.0);
        assert_eq!(tc.get("missing", 12.0, 7.0), 7.0);
        // After 22 it blends towards 0 the next day.
        assert_eq!(tc.get("light_dir_mult", 23.0, -1.0), 0.0);
    }

    #[test]
    fn the_sun_is_up_by_day_and_down_at_night() {
        let tc = TimeCycle::default();
        let up = |hour: f32| TimeCycle::sun_direction(tc.state(hour).sun_angle)[2];
        assert!(up(12.0) > 0.3, "noon {}", up(12.0));
        // East at sunrise, south at noon.
        assert!(TimeCycle::sun_direction(0.0)[0] > 0.9);
        assert!(TimeCycle::sun_direction(std::f32::consts::FRAC_PI_2)[1] < -0.3);
        assert!(up(0.0) < -0.3, "midnight {}", up(0.0));
        assert!(up(7.0).abs() < 0.05 && up(19.0).abs() < 0.1);
        assert!(tc.state(12.0).night == 0.0 && tc.state(1.0).night == 1.0);
    }
}
