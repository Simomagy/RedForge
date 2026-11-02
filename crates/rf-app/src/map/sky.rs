//! The map sky's moon and stars, from the game's skydome textures.

use rf_render::TextureSource;
use rf_render::assets::DictionaryTextures;

use crate::game::Game;
use crate::viewers::Gpu;

const SKYDOME: &str = "textures_0.rpf/textures/skydome.ytd";

/// Gives the world sky the skydome's moon (`moon-new`) and stars (`stars`).
pub fn load_textures(game: &Game, gpu: &Gpu) {
    let Some(file) = game.fs.find(SKYDOME) else {
        log::warn!("{SKYDOME} not found: the sky has no moon or stars");
        return;
    };
    let mut textures = DictionaryTextures::from_files(gpu.ctx.clone(), &game.fs, &game.hashes, &[file]);
    let moon = textures.texture("moon-new");
    let stars = textures.texture("stars");
    gpu.renderer.set_sky_textures(&gpu.ctx, moon.as_deref(), stars.as_deref());
}
