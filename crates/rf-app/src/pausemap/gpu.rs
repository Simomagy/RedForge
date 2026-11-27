//! The pause map on the GPU: each layer's detail family as a texture array
//! (with mipmaps for zooming out) and a grid lookup, drawn in egui's own
//! render pass by a paint callback.

use eframe::egui;
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;
use rf_editor::pausemap::compose::can_fill_stub;
use rf_editor::pausemap::families::{Family, Layer, WORLD_H, WORLD_W, detail, fit_of};
use rf_editor::pausemap::library::{FamilyData, Library};

/// What the view looks at.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Camera {
    /// The map point at the view's top-left.
    pub x: f64,
    pub y: f64,
    /// Points per map unit.
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { x: 0.0, y: 0.0, zoom: 0.2 }
    }
}

/// The map's colours around it, linear 0 to 1.
#[derive(Clone, Copy)]
pub struct Theme {
    pub backdrop: [f32; 3],
    pub grid: [f32; 3],
    pub seam: [f32; 3],
}

/// Which family each layer draws, in the shader's order.
const ORDER: [Layer; 4] = [Layer::Text, Layer::Lines, Layer::Water, Layer::Relief];

fn mips(size: u32) -> u32 {
    32 - size.leading_zeros()
}

/// Halves a square tile (2 x 2 box filter).
fn halve(src: &[u8], size: usize) -> Vec<u8> {
    let h = size / 2;
    let mut out = vec![0u8; h * h];
    for y in 0..h {
        for x in 0..h {
            let i = y * 2 * size + x * 2;
            let sum = src[i] as u32 + src[i + 1] as u32 + src[i + size] as u32 + src[i + size + 1] as u32;
            out[y * h + x] = ((sum + 2) >> 2) as u8;
        }
    }
    out
}

/// A tile and its smaller levels.
fn levels(pixels: &[u8], size: usize) -> Vec<Vec<u8>> {
    let mut out = vec![pixels.to_vec()];
    let mut s = size;
    while s > 1 {
        out.push(halve(out.last().unwrap(), s));
        s /= 2;
    }
    out
}

struct LayerGpu {
    family: &'static Family,
    tiles: wgpu::Texture,
    index: wgpu::Texture,
    /// Per cell: slot plus one (0 none), and the stub's value.
    cells: Vec<[u16; 2]>,
    /// The next slot free for a stub made a tile.
    free: u16,
    depth: u32,
}

/// The GPU's side, kept in egui's callback resources.
pub struct MapGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    layers: [Option<LayerGpu>; 4],
    bind_group: Option<wgpu::BindGroup>,
    /// Families too big for the GPU: what's wrong.
    pub problems: Vec<String>,
}

const UNIFORM_SIZE: u64 = 4 * 8 * 4 + 7 * 16;

impl MapGpu {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pause map"),
            source: wgpu::ShaderSource::Wgsl(include_str!("map.wgsl").into()),
        });
        let tex = |binding, dim, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: dim, multisampled: false },
            count: None,
        };
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        for i in 0..4 {
            entries.push(tex(
                2 + i,
                wgpu::TextureViewDimension::D2Array,
                wgpu::TextureSampleType::Float { filterable: true },
            ));
        }
        for i in 0..4 {
            entries.push(tex(6 + i, wgpu::TextureViewDimension::D2, wgpu::TextureSampleType::Uint));
        }
        let layout = device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("pause map"), entries: &entries });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pause map"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pause map"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pause map"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pause map"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        MapGpu {
            pipeline,
            layout,
            uniforms,
            sampler,
            layers: [None, None, None, None],
            bind_group: None,
            problems: Vec::new(),
        }
    }

    pub fn loaded(&self) -> bool {
        self.layers.iter().any(Option::is_some)
    }

    /// The game's tiles of each layer, once.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, lib: &Library) {
        let max_layers = device.limits().max_texture_array_layers;
        for (slot, layer) in ORDER.iter().enumerate() {
            let f = detail(*layer);
            let Some(data) = lib.get(f.name) else { continue };
            match upload_family(device, queue, f, data, max_layers) {
                Ok(l) => self.layers[slot] = Some(l),
                Err(e) => self.problems.push(e),
            }
        }
        self.bind(device);
    }

    /// One cell's tile in place of what it was (a stub gets a spare slot the
    /// first time). Mips are made on the CPU for that tile alone.
    pub fn update_tile(&mut self, queue: &wgpu::Queue, layer: Layer, row: u32, col: u32, pixels: &[u8]) {
        let Some(i) = ORDER.iter().position(|l| *l == layer) else { return };
        let Some(l) = &mut self.layers[i] else { return };
        let cell = (row * l.family.cols + col) as usize;
        if l.cells[cell][0] == 0 {
            if l.free as u32 > l.depth {
                return;
            }
            l.cells[cell][0] = l.free;
            l.free += 1;
            write_index(queue, l);
        }
        let slot = l.cells[cell][0] as u32 - 1;
        let size = l.family.tile as usize;
        for (mip, level) in levels(pixels, size).iter().enumerate() {
            let s = (size >> mip).max(1) as u32;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &l.tiles,
                    mip_level: mip as u32,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: slot },
                    aspect: wgpu::TextureAspect::All,
                },
                level,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(s), rows_per_image: Some(s) },
                wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
            );
        }
    }

    fn bind(&mut self, device: &wgpu::Device) {
        // A 1 x 1 stand-in for a layer without tiles.
        let dummy_tiles = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pause map none"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_index = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pause map none"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let array =
            wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() };
        let tiles: Vec<wgpu::TextureView> =
            self.layers.iter().map(|l| l.as_ref().map_or(&dummy_tiles, |l| &l.tiles).create_view(&array)).collect();
        let index: Vec<wgpu::TextureView> = self
            .layers
            .iter()
            .map(|l| l.as_ref().map_or(&dummy_index, |l| &l.index).create_view(&Default::default()))
            .collect();
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
        ];
        for (i, v) in tiles.iter().enumerate() {
            entries
                .push(wgpu::BindGroupEntry { binding: 2 + i as u32, resource: wgpu::BindingResource::TextureView(v) });
        }
        for (i, v) in index.iter().enumerate() {
            entries
                .push(wgpu::BindGroupEntry { binding: 6 + i as u32, resource: wgpu::BindingResource::TextureView(v) });
        }
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pause map"),
            layout: &self.layout,
            entries: &entries,
        }));
    }
}

fn write_index(queue: &wgpu::Queue, l: &LayerGpu) {
    let bytes: Vec<u8> = l.cells.iter().flat_map(|[a, b]| [a.to_le_bytes(), b.to_le_bytes()]).flatten().collect();
    queue.write_texture(
        l.index.as_image_copy(),
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(l.family.cols * 4),
            rows_per_image: Some(l.family.rows),
        },
        wgpu::Extent3d { width: l.family.cols, height: l.family.rows, depth_or_array_layers: 1 },
    );
}

fn upload_family(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    f: &'static Family,
    d: &FamilyData,
    max_layers: u32,
) -> Result<LayerGpu, String> {
    let count = d.tile_count() as u32;
    // Families whose stubs can be filled get a spare slot for each.
    let depth =
        if can_fill_stub(f) { count + d.index.iter().filter(|&&i| i == 0).count() as u32 } else { count }.max(1);
    if depth > max_layers {
        return Err(format!("the GPU can't hold {depth} tiles of {} (at most {max_layers})", f.name));
    }
    let size = f.tile;
    let mip_count = mips(size);
    let tiles = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(f.name),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: depth },
        mip_level_count: mip_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // Every tile's levels, a level at a time for all of them.
    let px = (size * size) as usize;
    let all: Vec<Vec<Vec<u8>>> =
        (0..count as usize).map(|i| levels(&d.tiles[i * px..(i + 1) * px], size as usize)).collect();
    for mip in 0..mip_count {
        let s = (size >> mip).max(1);
        let mut bytes: Vec<u8> = all.iter().flat_map(|t| t[mip as usize].iter().copied()).collect();
        // The spare slots start empty (paper).
        bytes.resize((s * s * depth) as usize, 255);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tiles,
                mip_level: mip,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(s), rows_per_image: Some(s) },
            wgpu::Extent3d { width: s, height: s, depth_or_array_layers: depth },
        );
    }
    let index = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(f.name),
        size: wgpu::Extent3d { width: f.cols, height: f.rows, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg16Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let cells = d.index.iter().zip(&d.stub_value).map(|(&slot, &stub)| [slot, stub as u16]).collect();
    let l = LayerGpu { family: f, tiles, index, cells, free: count as u16 + 1, depth };
    write_index(queue, &l);
    Ok(l)
}

/// One frame of the map, drawn in a rect of the screen.
pub struct Paint {
    pub camera: Camera,
    /// Text, lines, water, relief.
    pub show: [bool; 4],
    pub grid: bool,
    pub theme: Theme,
    /// The rect's top-left, in points.
    pub origin: egui::Pos2,
}

impl CallbackTrait for Paint {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = resources.get::<MapGpu>() else { return Vec::new() };
        let ppp = screen.pixels_per_point as f64;
        let mut v: Vec<f32> = Vec::with_capacity(UNIFORM_SIZE as usize / 4);
        for (i, layer) in ORDER.iter().enumerate() {
            let f = detail(*layer);
            let fit = fit_of(f);
            let loaded = gpu.layers[i].is_some();
            v.extend([f.cols as f32, f.rows as f32, f.tile as f32, f.stride() as f32]);
            v.extend([fit.sx as f32, fit.sy as f32, fit.ox as f32, fit.oy as f32]);
            let _ = loaded;
        }
        let shown = |i: usize| (self.show[i] && gpu.layers[i].is_some()) as u8 as f32;
        v.extend([shown(0), shown(1), shown(2), shown(3)]);
        v.extend([self.camera.x as f32, self.camera.y as f32, (self.camera.zoom * ppp) as f32, self.grid as u8 as f32]);
        v.extend([
            (self.origin.x as f64 * ppp) as f32,
            (self.origin.y as f64 * ppp) as f32,
            WORLD_W as f32,
            WORLD_H as f32,
        ]);
        let seam = detail(Layer::Text).stride() as f32;
        v.extend([seam, 0.0, 0.0, 0.0]);
        for c in [self.theme.backdrop, self.theme.grid, self.theme.seam] {
            v.extend([c[0], c[1], c[2], 1.0]);
        }
        let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
        queue.write_buffer(&gpu.uniforms, 0, &bytes);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(gpu) = resources.get::<MapGpu>() else { return };
        let Some(bind) = &gpu.bind_group else { return };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// The paint callback for a rect.
pub fn callback(rect: egui::Rect, paint: Paint) -> egui::Shape {
    egui::Shape::Callback(egui_wgpu::Callback::new_paint_callback(rect, paint))
}
