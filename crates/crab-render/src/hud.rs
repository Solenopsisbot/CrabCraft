//! 2D HUD overlay: a coloured-quad pass (crosshair, hotbar, health/food bars)
//! plus a textured-quad pass (item icons sampled from an item atlas).
//!
//! Geometry building ([`hud_geometry`]) is pure and unit-tested; the pipelines
//! drive both the live window and the headless [`render_hud_to_png`] used to
//! verify the overlay without a display.

use std::error::Error;
use std::path::Path;

use wgpu::util::DeviceExt;

use crate::renderer::{upload_texture, ATLAS_FORMAT, DEPTH_FORMAT};

/// Coloured 2D vertex: NDC position (xy) + RGB. Matches `COLOR_WGSL`.
pub type ColorVertex = [f32; 5];
/// Textured 2D vertex: NDC position (xy) + atlas UV. Matches `TEX_WGSL`.
pub type TexVertex = [f32; 4];

const COLOR_WGSL: &str = "
struct VsIn { @location(0) pos: vec2<f32>, @location(1) color: vec3<f32> };
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec3<f32> };
@vertex fn vs(in: VsIn) -> VsOut {
    var o: VsOut; o.clip = vec4<f32>(in.pos, 0.0, 1.0); o.color = in.color; return o;
}
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> { return vec4<f32>(in.color, 1.0); }
";

const TEX_WGSL: &str = "
@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
struct VsIn { @location(0) pos: vec2<f32>, @location(1) uv: vec2<f32> };
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(in: VsIn) -> VsOut {
    var o: VsOut; o.clip = vec4<f32>(in.pos, 0.0, 1.0); o.uv = in.uv; return o;
}
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(atlas, samp, in.uv);
    if (c.a < 0.01) { discard; }
    return c;
}
";

/// The two HUD pipelines plus the bind-group layout for the item atlas.
pub struct HudPipelines {
    pub color: wgpu::RenderPipeline,
    pub textured: wgpu::RenderPipeline,
    pub atlas_layout: wgpu::BindGroupLayout,
}

fn hud_depth_stencil() -> wgpu::DepthStencilState {
    // Drawn on top of everything: depth test always passes, never writes.
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: false,
        depth_compare: wgpu::CompareFunction::Always,
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

/// Builds the coloured + textured HUD pipelines for a given colour target.
pub fn build_hud_pipelines(device: &wgpu::Device, format: wgpu::TextureFormat) -> HudPipelines {
    let color_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("hud color shader"),
        source: wgpu::ShaderSource::Wgsl(COLOR_WGSL.into()),
    });
    let tex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("hud tex shader"),
        source: wgpu::ShaderSource::Wgsl(TEX_WGSL.into()),
    });

    let empty_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("hud color layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("hud atlas layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let tex_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("hud tex layout"),
        bind_group_layouts: &[&atlas_layout],
        push_constant_ranges: &[],
    });

    const COLOR_ATTRS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x3];
    const TEX_ATTRS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];

    let target = |blend| {
        Some(wgpu::ColorTargetState {
            format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
        })
    };

    let color = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hud color pipeline"),
        layout: Some(&empty_layout),
        vertex: wgpu::VertexState {
            module: &color_shader,
            entry_point: "vs",
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 20,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &COLOR_ATTRS,
            }],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &color_shader,
            entry_point: "fs",
            targets: &[target(Some(wgpu::BlendState::REPLACE))],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(hud_depth_stencil()),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    let textured = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hud tex pipeline"),
        layout: Some(&tex_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &tex_shader,
            entry_point: "vs",
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &TEX_ATTRS,
            }],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &tex_shader,
            entry_point: "fs",
            targets: &[target(Some(wgpu::BlendState::ALPHA_BLENDING))],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(hud_depth_stencil()),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    HudPipelines {
        color,
        textured,
        atlas_layout,
    }
}

fn push_color_quad(v: &mut Vec<ColorVertex>, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 3]) {
    for [px, py] in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]] {
        v.push([px, py, c[0], c[1], c[2]]);
    }
}

/// `y0` is the bottom edge, `y1` the top; `uv` is `[u0(left), v0(top), u1, v1]`.
fn push_tex_quad(v: &mut Vec<TexVertex>, x0: f32, y0: f32, x1: f32, y1: f32, uv: [f32; 4]) {
    let (u0, v0, u1, v1) = (uv[0], uv[1], uv[2], uv[3]);
    let tl = [x0, y1, u0, v0];
    let tr = [x1, y1, u1, v0];
    let br = [x1, y0, u1, v1];
    let bl = [x0, y0, u0, v1];
    for q in [tl, tr, br, tl, br, bl] {
        v.push(q);
    }
}

/// Builds the HUD geometry. `hotbar` holds up to 9 optional item-icon atlas UVs
/// (slot 0..8); `selected` is the highlighted hotbar index. Returns
/// `(coloured_verts, textured_verts)`.
#[must_use]
pub fn hud_geometry(
    health: f32,
    food: i32,
    selected: usize,
    hotbar: &[Option<[f32; 4]>],
    aspect: f32,
) -> (Vec<ColorVertex>, Vec<TexVertex>) {
    let a = aspect.max(0.01);
    let mut c = Vec::new();
    let mut t = Vec::new();
    let white = [0.9, 0.9, 0.9];

    // Crosshair.
    let (arm, thick) = (0.018, 0.0022);
    push_color_quad(&mut c, -arm / a, -thick, arm / a, thick, white);
    push_color_quad(&mut c, -thick / a, -arm, thick / a, arm, white);

    // Hotbar: 9 *square* slots centred along the bottom (slot width in NDC is
    // the height divided by the aspect so the cells aren't squished); the
    // selected slot is brighter.
    let slot_h = 0.11;
    let sw = slot_h / a;
    let gap = sw * 0.14;
    let total = 9.0 * sw + 8.0 * gap;
    let (y0, y1) = (-0.97, -0.97 + slot_h);
    let mut x = -total / 2.0;
    for i in 0..9 {
        let col = if i == selected {
            [0.85, 0.85, 0.85]
        } else {
            [0.30, 0.30, 0.33]
        };
        push_color_quad(&mut c, x, y0, x + sw, y1, col);
        if let Some(Some(uv)) = hotbar.get(i) {
            let ix = sw * 0.12;
            let iy = (y1 - y0) * 0.12;
            push_tex_quad(&mut t, x + ix, y0 + iy, x + sw - ix, y1 - iy, *uv);
        }
        x += sw + gap;
    }

    // Health (left, red) and food (right, brown) bars over dark backgrounds.
    let (by0, by1) = (-0.83, -0.80);
    push_color_quad(&mut c, -0.5, by0, -0.02, by1, [0.12, 0.12, 0.12]);
    let hp = (health / 20.0).clamp(0.0, 1.0);
    push_color_quad(&mut c, -0.5, by0, -0.5 + 0.48 * hp, by1, [0.85, 0.15, 0.15]);
    push_color_quad(&mut c, 0.02, by0, 0.5, by1, [0.12, 0.12, 0.12]);
    let fd = (food as f32 / 20.0).clamp(0.0, 1.0);
    push_color_quad(&mut c, 0.5 - 0.48 * fd, by0, 0.5, by1, [0.55, 0.40, 0.15]);

    (c, t)
}

/// Headlessly renders the HUD over a solid background to a PNG (verification).
#[allow(clippy::too_many_arguments)]
pub fn render_hud_to_png(
    color_verts: &[ColorVertex],
    tex_verts: &[TexVertex],
    atlas_rgba: &[u8],
    atlas_w: u32,
    atlas_h: u32,
    width: u32,
    height: u32,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let pixels = pollster::block_on(render_hud_to_rgba(
        color_verts,
        tex_verts,
        atlas_rgba,
        atlas_w,
        atlas_h,
        width,
        height,
    ))?;
    let img = image::RgbaImage::from_raw(width, height, pixels).ok_or("hud buffer wrong size")?;
    img.save(path)?;
    Ok(())
}

async fn render_hud_to_rgba(
    color_verts: &[ColorVertex],
    tex_verts: &[TexVertex],
    atlas_rgba: &[u8],
    atlas_w: u32,
    atlas_h: u32,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .ok_or("no suitable GPU adapter found")?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default(), None)
        .await?;

    let format = ATLAS_FORMAT;
    let pipelines = build_hud_pipelines(&device, format);
    let atlas_bg = upload_texture(
        &device,
        &queue,
        &pipelines.atlas_layout,
        atlas_rgba,
        atlas_w,
        atlas_h,
    );

    let color_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("hud color verts"),
        contents: bytemuck::cast_slice(color_verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let tex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("hud tex verts"),
        contents: bytemuck::cast_slice(tex_verts),
        usage: wgpu::BufferUsages::VERTEX,
    });

    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hud target"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hud depth"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

    let bpp = 4u32;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let unpadded_bpr = width * bpp;
    let padded_bpr = unpadded_bpr.div_ceil(align) * align;
    let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hud readback"),
        size: u64::from(padded_bpr) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("hud encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hud pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.10,
                        g: 0.12,
                        b: 0.16,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipelines.color);
        pass.set_vertex_buffer(0, color_buf.slice(..));
        pass.draw(0..color_verts.len() as u32, 0..1);
        if !tex_verts.is_empty() {
            pass.set_pipeline(&pipelines.textured);
            pass.set_bind_group(0, &atlas_bg, &[]);
            pass.set_vertex_buffer(0, tex_buf.slice(..));
            pass.draw(0..tex_verts.len() as u32, 0..1);
        }
    }

    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &output_buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(padded_bpr),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    queue.submit(Some(encoder.finish()));

    let slice = output_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()??;

    let data = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((unpadded_bpr * height) as usize);
    for row in 0..height {
        let start = (row * padded_bpr) as usize;
        pixels.extend_from_slice(&data[start..start + unpadded_bpr as usize]);
    }
    drop(data);
    output_buffer.unmap();
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_has_icons_only_for_filled_slots() {
        let uv = [0.0, 0.0, 0.5, 0.5];
        let hotbar = [Some(uv), None, Some(uv), None, None, None, None, None, None];
        let (color, tex) = hud_geometry(20.0, 20, 2, &hotbar, 1.0);
        // 2 filled slots -> 2 textured quads -> 12 vertices.
        assert_eq!(tex.len(), 12);
        // Colour geometry: crosshair(2) + 9 slots + 4 bars = 15 quads = 90 verts.
        assert_eq!(color.len(), 90);
    }

    #[test]
    fn tex_quad_uv_is_upright() {
        let hotbar = [
            Some([0.1, 0.2, 0.3, 0.4]),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ];
        let (_c, t) = hud_geometry(20.0, 20, 0, &hotbar, 1.0);
        // First vertex is the top-left corner -> (u0, v0).
        assert_eq!([t[0][2], t[0][3]], [0.1, 0.2]);
        // Third vertex is bottom-right -> (u1, v1).
        assert_eq!([t[2][2], t[2][3]], [0.3, 0.4]);
    }
}
