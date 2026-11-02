//! Runs the HDR post-processing on synthetic images and saves PNGs, to see the
//! bloom, the flares, the curves and the eye adaptation without the app:
//!
//! ```sh
//! cargo run -p rf-render --example post -- [out dir]
//! ```

use std::path::{Path, PathBuf};

use anyhow::Result;
use rf_render::{GpuContext, OUTPUT_FORMAT, Post, PostSettings, PostTargets, Tonemap};

const W: u32 = 960;
const H: u32 = 540;

/// A sky over the ground with a band of colours and a few very bright spots:
/// a sun, scaled with the rest by `scale`, and two lamps, which aren't.
fn scene(scale: f32) -> Vec<[f32; 4]> {
    let mut pixels = Vec::with_capacity((W * H) as usize);
    for y in 0..H {
        for x in 0..W {
            let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
            let mut c = if v < 0.6 {
                let t = v / 0.6;
                [0.3 + 0.7 * t, 0.5 + 0.4 * t, 1.0 - 0.2 * t].map(|c| c * 1.5)
            } else {
                // The ground darkens left to right, over 6 stops.
                let k = 0.4 * (-u * 6.0).exp2();
                [0.35 * k, 0.28 * k, 0.2 * k]
            };
            if (0.42..0.5).contains(&v) {
                const BARS: [[f32; 3]; 6] =
                    [[0.8, 0.05, 0.05], [0.05, 0.6, 0.05], [0.05, 0.1, 0.8], [0.8, 0.7, 0.1], [0.5; 3], [0.18; 3]];
                c = BARS[(u * 6.0) as usize];
            }
            c = c.map(|c| c * scale);
            let spot = |cx: f32, cy: f32, r: f32| ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() < r;
            if spot(0.8 * W as f32, 0.18 * H as f32, 9.0) {
                c = [600.0 * scale, 520.0 * scale, 400.0 * scale];
            } else if spot(0.2 * W as f32, 0.72 * H as f32, 4.0) || spot(0.5 * W as f32, 0.68 * H as f32, 3.0) {
                c = [90.0, 60.0, 25.0];
            }
            pixels.push([c[0], c[1], c[2], 1.0]);
        }
    }
    pixels
}

/// The f16 bits of a value (truncated).
fn f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    match exp {
        ..-10 => sign,
        // Subnormal.
        -10..=0 => sign | ((mantissa | 0x80_0000) >> (14 - exp)) as u16,
        31.. => sign | 0x7c00,
        _ => sign | ((exp as u16) << 10) | (mantissa >> 13) as u16,
    }
}

struct Images {
    hdr: wgpu::Texture,
    hdr_view: wgpu::TextureView,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
}

impl Images {
    fn new(ctx: &GpuContext) -> Self {
        let texture = |label, format, usage| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let hdr = texture(
            "hdr",
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let output = texture(
            "output",
            OUTPUT_FORMAT.remove_srgb_suffix(),
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let (hdr_view, output_view) = (hdr.create_view(&Default::default()), output.create_view(&Default::default()));
        Self { hdr, hdr_view, output, output_view }
    }

    fn upload(&self, ctx: &GpuContext, pixels: &[[f32; 4]]) {
        let halves: Vec<u16> = pixels.iter().flatten().map(|&v| f16_bits(v)).collect();
        ctx.queue.write_texture(
            self.hdr.as_image_copy(),
            bytemuck::cast_slice(&halves),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 8), rows_per_image: Some(H) },
            wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        );
    }

    /// One frame of `post`, read back as sRGB bytes.
    fn frame(&self, ctx: &GpuContext, post: &Post, targets: &PostTargets, settings: &PostSettings) -> Vec<u8> {
        let row = (W * 4).div_ceil(256) * 256;
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * H) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        post.run(ctx, &mut encoder, targets, &self.output_view, settings);
        encoder.copy_texture_to_buffer(
            self.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(H) },
            },
            wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        );
        ctx.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
        let mapped = slice.get_mapped_range().expect("mapped readback buffer");
        (0..H as usize).flat_map(|y| &mapped[y * row as usize..][..W as usize * 4]).copied().collect()
    }
}

/// The average sRGB value of the colour channels, 0-255.
fn mean(pixels: &[u8]) -> f32 {
    let sum: u64 = pixels.chunks(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum();
    sum as f32 / (pixels.len() / 4 * 3) as f32
}

fn save(dir: &Path, name: &str, pixels: &[u8], width: u32, height: u32) -> Result<()> {
    let path = dir.join(name);
    image::save_buffer(&path, pixels, width, height, image::ColorType::Rgba8)?;
    println!("saved {}", path.display());
    Ok(())
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&dir)?;
    let ctx = pollster::block_on(GpuContext::headless())?;
    let post = Post::new(&ctx);
    let images = Images::new(&ctx);
    let (day, night) = (scene(1.0), scene(0.01));
    let lit = PostSettings::default();

    // Each image snaps to its own luminance on its first frame.
    images.upload(&ctx, &day);
    let targets = post.targets(&ctx, &images.hdr_view, W, H);
    let day_px = images.frame(&ctx, &post, &targets, &lit);
    save(&dir, "post_day.png", &day_px, W, H)?;
    let plain = images.frame(&ctx, &post, &targets, &PostSettings { bloom: false, ..lit });
    save(&dir, "post_day_no_bloom.png", &plain, W, H)?;
    let raw = images.frame(&ctx, &post, &targets, &PostSettings { passthrough: true, ..lit });
    save(&dir, "post_day_passthrough.png", &raw, W, H)?;

    images.upload(&ctx, &night);
    let night_targets = post.targets(&ctx, &images.hdr_view, W, H);
    let night_px = images.frame(&ctx, &post, &night_targets, &lit);
    save(&dir, "post_night.png", &night_px, W, H)?;
    let blue = PostSettings { scotopic: [0.8, 0.95, 1.6, 0.7], ..lit };
    let scotopic = images.frame(&ctx, &post, &night_targets, &blue);
    save(&dir, "post_night_scotopic.png", &scotopic, W, H)?;
    println!("mean: day {:.1}, night {:.1} (sRGB, 0-255)", mean(&day_px), mean(&night_px));

    // Adapted to the day, then 3 seconds of night at 10 frames a second.
    images.upload(&ctx, &day);
    let mut targets = post.targets(&ctx, &images.hdr_view, W, H);
    images.frame(&ctx, &post, &targets, &lit);
    images.upload(&ctx, &night);
    let step = PostSettings { elapsed: 0.1, ..lit };
    for frame in 1..=30 {
        let px = images.frame(&ctx, &post, &targets, &step);
        if [1, 2, 5, 10, 15, 20, 30].contains(&frame) {
            println!("night, frame {frame}: mean {:.1}", mean(&px));
        }
    }
    // A resize keeps the adaptation.
    post.resize(&ctx, &mut targets, &images.hdr_view, W, H);
    println!("night, resized: mean {:.1}", mean(&images.frame(&ctx, &post, &targets, &step)));

    // The curves, at half size in a 5×2 grid.
    images.upload(&ctx, &day);
    let (cw, ch) = (W / 2, H / 2);
    let mut grid = vec![0u8; (cw * 5 * ch * 2 * 4) as usize];
    for (i, tonemap) in Tonemap::ALL.into_iter().enumerate() {
        let px = images.frame(&ctx, &post, &targets, &PostSettings { tonemap, ..lit });
        println!("{}: mean {:.1}", tonemap.name(), mean(&px));
        let (gx, gy) = (i as u32 % 5 * cw, i as u32 / 5 * ch);
        for y in 0..ch {
            for x in 0..cw {
                let from = (((y * 2) * W + x * 2) * 4) as usize;
                let to = (((gy + y) * cw * 5 + gx + x) * 4) as usize;
                grid[to..to + 4].copy_from_slice(&px[from..from + 4]);
            }
        }
    }
    save(&dir, "post_tonemaps.png", &grid, cw * 5, ch * 2)?;
    Ok(())
}
