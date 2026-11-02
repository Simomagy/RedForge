//! The game's global textures that shaders use but no model names: the
//! water's ripples ("waterbump2", in `graphics.ytd`), and the sky's moon and
//! stars (`moon-new` and `stars`, in `skydome.ytd`).

use rf_render::TextureSource;
use rf_render::assets::DictionaryTextures;

use crate::game::Game;
use crate::viewers::Gpu;

const GRAPHICS: &str = "textures_0.rpf/textures/graphics.ytd";
const SKYDOME: &str = "textures_0.rpf/textures/skydome.ytd";

/// Gives the renderer the global textures it uses.
pub fn load(game: &Game, gpu: &Gpu) {
    let dictionary = |path: &str, without: &str| {
        let file = game.fs.find(path);
        if file.is_none() {
            log::warn!("{path} not found: {without}");
        }
        file.map(|f| DictionaryTextures::from_files(gpu.ctx.clone(), &game.fs, &game.hashes, &[f]))
    };
    if let Some(mut textures) = dictionary(GRAPHICS, "the water has no ripples") {
        match textures.texture("waterbump2") {
            Some(t) => gpu.renderer.set_water_bump(t),
            None => log::warn!("waterbump2 not in {GRAPHICS}"),
        }
    }
    if let Some(mut textures) = dictionary(SKYDOME, "the sky has no moon or stars") {
        let moon = textures.texture("moon-new");
        let stars = textures.texture("stars");
        gpu.renderer.set_sky_textures(&gpu.ctx, moon.as_deref(), stars.as_deref());
    }
}
