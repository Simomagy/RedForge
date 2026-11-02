//! The game's global textures that shaders use but no model names: the
//! water's ripples ("waterbump2", in `graphics.ytd`).

use rf_render::TextureSource;
use rf_render::assets::DictionaryTextures;

use crate::game::Game;
use crate::viewers::Gpu;

const GRAPHICS: &str = "textures_0.rpf/textures/graphics.ytd";

/// Gives the renderer the global textures it uses.
pub fn load(game: &Game, gpu: &Gpu) {
    let Some(file) = game.fs.find(GRAPHICS) else {
        log::warn!("{GRAPHICS} not found: the water has no ripples");
        return;
    };
    let mut textures = DictionaryTextures::from_files(gpu.ctx.clone(), &game.fs, &game.hashes, &[file]);
    match textures.texture("waterbump2") {
        Some(t) => gpu.renderer.set_water_bump(t),
        None => log::warn!("waterbump2 not in {GRAPHICS}"),
    }
}
