//! 2D HUD overlay: a coloured-quad pass (crosshair, hotbar, health/food bars)
//! plus a textured-quad pass (item icons sampled from an item atlas).
//!
//! Geometry building ([`hud_geometry`]) is pure and unit-tested; the pipelines
//! drive both the live window and the headless [`render_hud_to_png`] used to
//! verify the overlay without a display.

use std::error::Error;
use std::path::Path;

use crab_assets::GuiAtlas;
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

/// Centred NDC rect for a sprite of pixel size `(pw, ph)` drawn at on-screen
/// height `h_ndc`, keeping square pixels for `aspect`. Returns `(x0,y0,x1,y1)`.
fn sprite_rect(cx: f32, cy: f32, h_ndc: f32, pw: f32, ph: f32, a: f32) -> (f32, f32, f32, f32) {
    let w_ndc = h_ndc * (pw / ph) / a;
    (
        cx - w_ndc / 2.0,
        cy - h_ndc / 2.0,
        cx + w_ndc / 2.0,
        cy + h_ndc / 2.0,
    )
}

/// Maps a pixel coordinate `(px, py)` (py from the sprite's TOP) within a sprite
/// of pixel size `(pw, ph)` placed at NDC `rect` to an NDC point.
fn px_to_ndc(rect: (f32, f32, f32, f32), pw: f32, ph: f32, px: f32, py: f32) -> (f32, f32) {
    let (x0, y0, x1, y1) = rect;
    (x0 + (px / pw) * (x1 - x0), y1 - (py / ph) * (y1 - y0))
}

/// Appends glyph quads for `text` (sampling the GUI/font atlas) with the
/// top-left at `(x, y)` and glyph-cell height `h_ndc`; returns the end x.
pub fn push_text(
    out: &mut Vec<TexVertex>,
    gui: &GuiAtlas,
    text: &str,
    x: f32,
    y: f32,
    h_ndc: f32,
    aspect: f32,
) -> f32 {
    let scale = h_ndc / 8.0; // NDC per font pixel (vertical)
    let hscale = scale / aspect.max(0.01); // NDC per pixel (horizontal, square)
    let mut cx = x;
    for ch in text.chars() {
        let glyph = gui.glyph(ch);
        if glyph.width > 0.0 {
            push_tex_quad(out, cx, y - h_ndc, cx + glyph.width * hscale, y, glyph.uv);
        }
        cx += glyph.advance * hscale;
    }
    cx
}

/// Builds the HUD geometry from the vanilla widget sprites. `hotbar` holds up to
/// 9 optional item-icon atlas UVs; `selected` is the highlighted slot. Returns
/// `(colour, gui_tex, item_tex)` — gui_tex samples `gui`, item_tex the items.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn hud_geometry(
    gui: &GuiAtlas,
    health: f32,
    food: i32,
    xp_bar: f32,
    xp_level: i32,
    selected: usize,
    hotbar: &[Option<[f32; 4]>],
    aspect: f32,
) -> (Vec<ColorVertex>, Vec<TexVertex>, Vec<TexVertex>) {
    let a = aspect.max(0.01);
    let mut c = Vec::new();
    let mut g = Vec::new();
    let mut t = Vec::new();
    let white = [0.9, 0.9, 0.9];

    // Crosshair.
    let (arm, thick) = (0.018, 0.0022);
    push_color_quad(&mut c, -arm / a, -thick, arm / a, thick, white);
    push_color_quad(&mut c, -thick / a, -arm, thick / a, arm, white);

    // Hotbar bar (widgets sprite, 182x22) centred along the bottom.
    let bar_h = 0.13;
    let bar = sprite_rect(0.0, -1.0 + bar_h / 2.0 + 0.02, bar_h, 182.0, 22.0, a);
    if let Some(uv) = gui.sprite("hotbar") {
        push_tex_quad(&mut g, bar.0, bar.1, bar.2, bar.3, uv);
    } else {
        push_color_quad(&mut c, bar.0, bar.1, bar.2, bar.3, [0.2, 0.2, 0.22]);
    }
    // Selection box (24x24) over the selected slot.
    if let Some(uv) = gui.sprite("selection") {
        let sx = selected as f32 * 20.0 - 1.0;
        let (lx, ty) = px_to_ndc(bar, 182.0, 22.0, sx, -1.0);
        let (rx, by) = px_to_ndc(bar, 182.0, 22.0, sx + 24.0, 23.0);
        push_tex_quad(&mut g, lx, by, rx, ty, uv);
    }
    // Item icons at the slot positions (3 + i*20, 3, 16x16) in bar pixels.
    for (i, slot) in hotbar.iter().enumerate().take(9) {
        if let Some(uv) = slot {
            let px = 3.0 + i as f32 * 20.0;
            let (lx, ty) = px_to_ndc(bar, 182.0, 22.0, px, 3.0);
            let (rx, by) = px_to_ndc(bar, 182.0, 22.0, px + 16.0, 19.0);
            push_tex_quad(&mut t, lx, by, rx, ty, *uv);
        }
    }

    // Status bars above the hotbar, drawn from icons.png if available.
    let ppx = (bar.2 - bar.0) / 182.0; // NDC per bar pixel
                                       // sprite at bar-pixel x, `y_up` px above the hotbar top, size (w,h) px.
    let spr = |buf: &mut Vec<TexVertex>, px: f32, y_up: f32, w: f32, h: f32, uv: [f32; 4]| {
        let x0 = bar.0 + px * ppx;
        let y0 = bar.3 + y_up * ppx;
        push_tex_quad(buf, x0, y0, x0 + w * ppx, y0 + h * ppx, uv);
    };

    if let (Some(xbg), Some(xfg)) = (gui.sprite("xp_bg"), gui.sprite("xp_full")) {
        spr(&mut g, 0.0, 4.0, 182.0, 5.0, xbg);
        let frac = xp_bar.clamp(0.0, 1.0);
        if frac > 0.0 {
            let cuv = [xfg[0], xfg[1], xfg[0] + (xfg[2] - xfg[0]) * frac, xfg[3]];
            spr(&mut g, 0.0, 4.0, 182.0 * frac, 5.0, cuv);
        }
    }
    if let (Some(hbg), Some(hfull), Some(hhalf)) = (
        gui.sprite("heart_bg"),
        gui.sprite("heart_full"),
        gui.sprite("heart_half"),
    ) {
        for i in 0..10 {
            let x = i as f32 * 8.0;
            spr(&mut g, x, 12.0, 9.0, 9.0, hbg);
            let level = health - i as f32 * 2.0;
            if level >= 2.0 {
                spr(&mut g, x, 12.0, 9.0, 9.0, hfull);
            } else if level >= 1.0 {
                spr(&mut g, x, 12.0, 9.0, 9.0, hhalf);
            }
        }
    } else {
        // No jar icons: fall back to a coloured health bar.
        let (by0, by1) = (bar.3 + 0.012, bar.3 + 0.042);
        let hp = (health / 20.0).clamp(0.0, 1.0);
        push_color_quad(&mut c, -0.5, by0, -0.5 + 0.48 * hp, by1, [0.85, 0.15, 0.15]);
    }
    if let (Some(fbg), Some(ffull), Some(fhalf)) = (
        gui.sprite("food_bg"),
        gui.sprite("food_full"),
        gui.sprite("food_half"),
    ) {
        for i in 0..10 {
            let x = 169.0 - i as f32 * 8.0;
            spr(&mut g, x, 12.0, 9.0, 9.0, fbg);
            let level = food as f32 - i as f32 * 2.0;
            if level >= 2.0 {
                spr(&mut g, x, 12.0, 9.0, 9.0, ffull);
            } else if level >= 1.0 {
                spr(&mut g, x, 12.0, 9.0, 9.0, fhalf);
            }
        }
    } else {
        let (by0, by1) = (bar.3 + 0.012, bar.3 + 0.042);
        let fd = (food as f32 / 20.0).clamp(0.0, 1.0);
        push_color_quad(&mut c, 0.5 - 0.48 * fd, by0, 0.5, by1, [0.55, 0.40, 0.15]);
    }

    // XP level number, centred just above the xp bar (green, in the gui/font).
    if xp_level > 0 && gui.sprite("xp_full").is_some() {
        let s = xp_level.to_string();
        let th = 7.0 * ppx; // ~7px tall
        let tw = gui.text_width(&s) * ppx;
        let cx = bar.0 + (91.0 * ppx) - tw / 2.0;
        let top = bar.3 + 11.0 * ppx;
        push_text(&mut g, gui, &s, cx, top, th, aspect);
    }

    (c, g, t)
}

/// Pixel cell of a window slot in the vanilla inventory texture. Slot numbering
/// matches the server player-inventory window: 0 = crafting result, 1-4 =
/// crafting grid, 5-8 = armour, 9-35 = main, 36-44 = hotbar, 45 = offhand.
fn slot_px(slot: usize) -> (f32, f32) {
    match slot {
        0 => (154.0, 28.0),
        1 => (98.0, 18.0),
        2 => (116.0, 18.0),
        3 => (98.0, 36.0),
        4 => (116.0, 36.0),
        5..=8 => (8.0, 8.0 + (slot - 5) as f32 * 18.0),
        45 => (77.0, 62.0),
        9..=35 => {
            let i = slot - 9;
            (8.0 + (i % 9) as f32 * 18.0, 84.0 + (i / 9) as f32 * 18.0)
        }
        _ => (8.0 + (slot.min(44) - 36) as f32 * 18.0, 142.0),
    }
}

/// NDC rect of a window slot's 16x16 icon within the inventory background at
/// `rect` (176x166 px). `slot` is the server window-slot index (0..46).
#[must_use]
pub fn inventory_slot_rect(rect: (f32, f32, f32, f32), slot: usize) -> (f32, f32, f32, f32) {
    let (px, py) = slot_px(slot);
    let (lx, ty) = px_to_ndc(rect, 176.0, 166.0, px + 1.0, py + 1.0);
    let (rx, by) = px_to_ndc(rect, 176.0, 166.0, px + 17.0, py + 17.0);
    (lx, by, rx, ty)
}

/// NDC rect of the inventory background (176x166) centred on screen.
#[must_use]
pub fn inventory_rect(aspect: f32) -> (f32, f32, f32, f32) {
    sprite_rect(0.0, 0.0, 0.78, 176.0, 166.0, aspect.max(0.01))
}

/// NDC rect of hotbar slot `i`'s 16x16 icon (matches [`hud_geometry`]).
#[must_use]
pub fn hotbar_slot_rect(aspect: f32, i: usize) -> (f32, f32, f32, f32) {
    let bar = sprite_rect(
        0.0,
        -1.0 + 0.13 / 2.0 + 0.02,
        0.13,
        182.0,
        22.0,
        aspect.max(0.01),
    );
    let px = 3.0 + i as f32 * 20.0;
    let (lx, ty) = px_to_ndc(bar, 182.0, 22.0, px, 3.0);
    let (rx, by) = px_to_ndc(bar, 182.0, 22.0, px + 16.0, 19.0);
    (lx, by, rx, ty)
}

/// Builds the open-inventory panel from the vanilla container background plus
/// item icons. `items` is indexed by server window slot (0..46). Returns
/// `(colour, gui_tex, item_tex)`.
#[must_use]
pub fn inventory_geometry(
    gui: &GuiAtlas,
    items: &[Option<[f32; 4]>],
    aspect: f32,
) -> (Vec<ColorVertex>, Vec<TexVertex>, Vec<TexVertex>) {
    let mut c = Vec::new();
    let mut g = Vec::new();
    let mut t = Vec::new();
    let rect = inventory_rect(aspect);

    if let Some(uv) = gui.sprite("inventory") {
        push_tex_quad(&mut g, rect.0, rect.1, rect.2, rect.3, uv);
    } else {
        push_color_quad(&mut c, rect.0, rect.1, rect.2, rect.3, [0.14, 0.14, 0.17]);
    }
    for (slot, uv) in items.iter().enumerate().take(46) {
        if let Some(uv) = uv {
            let (x0, y0, x1, y1) = inventory_slot_rect(rect, slot);
            push_tex_quad(&mut t, x0, y0, x1, y1, *uv);
        }
    }
    (c, g, t)
}

/// All vertex streams for one HUD frame: flat-coloured quads, GUI-atlas sprites
/// (backgrounds/widgets), item-atlas icons, and GUI-atlas text (drawn on top).
pub struct HudFrame<'a> {
    pub color: &'a [ColorVertex],
    pub gui: &'a [TexVertex],
    pub item: &'a [TexVertex],
    pub text: &'a [TexVertex],
}

/// Headlessly renders a HUD frame over a solid background to a PNG.
#[allow(clippy::too_many_arguments)]
pub fn render_hud_to_png(
    frame: &HudFrame,
    gui_rgba: &[u8],
    gui_w: u32,
    gui_h: u32,
    item_rgba: &[u8],
    item_w: u32,
    item_h: u32,
    width: u32,
    height: u32,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let pixels = pollster::block_on(render_hud_to_rgba(
        frame, gui_rgba, gui_w, gui_h, item_rgba, item_w, item_h, width, height,
    ))?;
    let img = image::RgbaImage::from_raw(width, height, pixels).ok_or("hud buffer wrong size")?;
    img.save(path)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn render_hud_to_rgba(
    frame: &HudFrame<'_>,
    gui_rgba: &[u8],
    gui_w: u32,
    gui_h: u32,
    item_rgba: &[u8],
    item_w: u32,
    item_h: u32,
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
    let gui_bg = upload_texture(
        &device,
        &queue,
        &pipelines.atlas_layout,
        gui_rgba,
        gui_w,
        gui_h,
    );
    let item_bg = upload_texture(
        &device,
        &queue,
        &pipelines.atlas_layout,
        item_rgba,
        item_w,
        item_h,
    );

    let vbuf = |label, verts: &[[f32; 4]]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(verts),
            usage: wgpu::BufferUsages::VERTEX,
        })
    };
    let color_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("hud color verts"),
        contents: bytemuck::cast_slice(frame.color),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let gui_buf = vbuf("hud gui verts", frame.gui);
    let item_buf = vbuf("hud item verts", frame.item);
    let text_buf = vbuf("hud text verts", frame.text);

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
        pass.draw(0..frame.color.len() as u32, 0..1);
        // GUI backgrounds, then item icons, then text — each its own atlas.
        for (verts, buf, bg) in [
            (frame.gui, &gui_buf, &gui_bg),
            (frame.item, &item_buf, &item_bg),
            (frame.text, &text_buf, &gui_bg),
        ] {
            if !verts.is_empty() {
                pass.set_pipeline(&pipelines.textured);
                pass.set_bind_group(0, bg, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..verts.len() as u32, 0..1);
            }
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
        let gui = crab_assets::GuiAtlas::empty();
        let uv = [0.0, 0.0, 0.5, 0.5];
        let hotbar = [Some(uv), None, Some(uv), None, None, None, None, None, None];
        let (_color, _g, item) = hud_geometry(&gui, 20.0, 20, 2, &hotbar, 1.0);
        // 2 filled slots -> 2 textured quads -> 12 vertices.
        assert_eq!(item.len(), 12);
    }

    #[test]
    fn tex_quad_uv_is_upright() {
        let gui = crab_assets::GuiAtlas::empty();
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
        let (_c, _g, t) = hud_geometry(&gui, 20.0, 20, 0, &hotbar, 1.0);
        // First vertex is the top-left corner -> (u0, v0).
        assert_eq!([t[0][2], t[0][3]], [0.1, 0.2]);
        // Third vertex is bottom-right -> (u1, v1).
        assert_eq!([t[2][2], t[2][3]], [0.3, 0.4]);
    }
}
