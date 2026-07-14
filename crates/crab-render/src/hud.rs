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
    let (ppx, ppy) = ((bar.2 - bar.0) / 182.0, (bar.3 - bar.1) / 22.0);
    // Sprite at bar-pixel x, `y_up` px above the hotbar top, size (w,h) px.
    // X and Y need separate NDC scales on non-square framebuffers.
    let spr = |buf: &mut Vec<TexVertex>, px: f32, y_up: f32, w: f32, h: f32, uv: [f32; 4]| {
        let x0 = bar.0 + px * ppx;
        let y0 = bar.3 + y_up * ppy;
        push_tex_quad(buf, x0, y0, x0 + w * ppx, y0 + h * ppy, uv);
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
        let th = 7.0 * ppy; // ~7px tall
        let tw = gui.text_width(&s) * ppx;
        let cx = bar.0 + (91.0 * ppx) - tw / 2.0;
        let top = bar.3 + 11.0 * ppy;
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
    let (lx, ty) = px_to_ndc(rect, 176.0, 166.0, px, py);
    let (rx, by) = px_to_ndc(rect, 176.0, 166.0, px + 16.0, py + 16.0);
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

/// NDC rect of menu button `i` of `n`, stacked + centred on screen (200x20 px).
#[must_use]
pub fn menu_button_rect(aspect: f32, i: usize, n: usize) -> (f32, f32, f32, f32) {
    let bh = 0.09;
    let bw = bh * (200.0 / 20.0) / aspect.max(0.01);
    let gap = bh * 0.5;
    let total = n as f32 * bh + (n.saturating_sub(1)) as f32 * gap;
    let y1 = total / 2.0 - i as f32 * (bh + gap);
    (-bw / 2.0, y1 - bh, bw / 2.0, y1)
}

/// Builds a centred button menu over a dimming overlay. `hovered` highlights one
/// button. Returns `(colour, gui_tex, _)`; labels render into the gui stream.
#[must_use]
pub fn menu_geometry(
    gui: &GuiAtlas,
    labels: &[&str],
    hovered: Option<usize>,
    aspect: f32,
) -> (Vec<ColorVertex>, Vec<TexVertex>, Vec<TexVertex>) {
    let mut c = Vec::new();
    let mut g = Vec::new();
    // Dimming background over the whole screen.
    push_color_quad(&mut c, -1.0, -1.0, 1.0, 1.0, [0.08, 0.08, 0.10]);
    for (i, label) in labels.iter().enumerate() {
        let r = menu_button_rect(aspect, i, labels.len());
        let sprite = if hovered == Some(i) {
            gui.sprite("button_hover")
        } else {
            gui.sprite("button")
        };
        match sprite {
            Some(uv) => push_tex_quad(&mut g, r.0, r.1, r.2, r.3, uv),
            None => push_color_quad(&mut c, r.0, r.1, r.2, r.3, [0.35, 0.35, 0.38]),
        }
        let th = (r.3 - r.1) * 0.42;
        let tw = gui.text_width(label) * (th / 8.0) / aspect.max(0.01);
        push_text(
            &mut g,
            gui,
            label,
            -tw / 2.0,
            (r.1 + r.3) / 2.0 + th / 2.0,
            th,
            aspect,
        );
    }
    (c, g, Vec::new())
}

/// Draws active vanilla status-effect icons down the top-right of the HUD.
#[must_use]
pub fn status_effect_geometry(gui: &GuiAtlas, effect_ids: &[i32], aspect: f32) -> Vec<TexVertex> {
    const NAMES: [&str; 33] = [
        "speed",
        "slowness",
        "haste",
        "mining_fatigue",
        "strength",
        "instant_health",
        "instant_damage",
        "jump_boost",
        "nausea",
        "regeneration",
        "resistance",
        "fire_resistance",
        "water_breathing",
        "invisibility",
        "blindness",
        "night_vision",
        "hunger",
        "weakness",
        "poison",
        "wither",
        "health_boost",
        "absorption",
        "saturation",
        "glowing",
        "levitation",
        "luck",
        "unluck",
        "slow_falling",
        "conduit_power",
        "dolphins_grace",
        "bad_omen",
        "hero_of_the_village",
        "darkness",
    ];
    let mut vertices = Vec::new();
    let size = 0.09;
    let width = size / aspect.max(0.01);
    for (visible, &id) in effect_ids.iter().enumerate() {
        let Some(name) = usize::try_from(id - 1).ok().and_then(|i| NAMES.get(i)) else {
            continue;
        };
        let Some(uv) = gui.sprite(name) else {
            continue;
        };
        let column = visible / 8;
        let row = visible % 8;
        let x1 = 0.97 - column as f32 * (width + 0.02);
        let y1 = 0.97 - row as f32 * (size + 0.02);
        push_tex_quad(&mut vertices, x1 - width, y1 - size, x1, y1, uv);
    }
    vertices
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

/// NDC bounds for a generic 9-column container with `rows` storage rows.
#[must_use]
pub fn container_rect(rows: usize, aspect: f32) -> (f32, f32, f32, f32) {
    let height = 114.0 + rows.clamp(1, 6) as f32 * 18.0;
    sprite_rect(
        0.0,
        0.0,
        0.78 * height / 166.0,
        176.0,
        height,
        aspect.max(0.01),
    )
}

/// NDC bounds of a slot in a generic 9-column container. Container slots come
/// first, followed by the player's 27 main slots and 9 hotbar slots.
#[must_use]
pub fn container_slot_rect(
    rect: (f32, f32, f32, f32),
    rows: usize,
    slot: usize,
) -> (f32, f32, f32, f32) {
    let rows = rows.clamp(1, 6);
    let storage = rows * 9;
    let (px, py) = if slot < storage {
        (
            8.0 + (slot % 9) as f32 * 18.0,
            18.0 + (slot / 9) as f32 * 18.0,
        )
    } else {
        let i = slot - storage;
        if i < 27 {
            (
                8.0 + (i % 9) as f32 * 18.0,
                32.0 + rows as f32 * 18.0 + (i / 9) as f32 * 18.0,
            )
        } else {
            (
                8.0 + (i.min(35) - 27) as f32 * 18.0,
                90.0 + rows as f32 * 18.0,
            )
        }
    };
    let height = 114.0 + rows as f32 * 18.0;
    let (lx, ty) = px_to_ndc(rect, 176.0, height, px, py);
    let (rx, by) = px_to_ndc(rect, 176.0, height, px + 16.0, py + 16.0);
    (lx, by, rx, ty)
}

fn crop_uv_dims(
    uv: [f32; 4],
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    source_w: f32,
    source_h: f32,
) -> [f32; 4] {
    let du = uv[2] - uv[0];
    let dv = uv[3] - uv[1];
    [
        uv[0] + du * x / source_w,
        uv[1] + dv * y / source_h,
        uv[0] + du * (x + w) / source_w,
        uv[1] + dv * (y + h) / source_h,
    ]
}

fn crop_uv(uv: [f32; 4], x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
    crop_uv_dims(uv, x, y, w, h, 176.0, 222.0)
}

/// Builds a vanilla generic-container panel (chests, barrels, ender chests and
/// shulker boxes) and its item icons.
#[must_use]
pub fn container_geometry(
    gui: &GuiAtlas,
    items: &[Option<[f32; 4]>],
    rows: usize,
    aspect: f32,
) -> (Vec<ColorVertex>, Vec<TexVertex>, Vec<TexVertex>) {
    let rows = rows.clamp(1, 6);
    let mut c = Vec::new();
    let mut g = Vec::new();
    let mut t = Vec::new();
    let rect = container_rect(rows, aspect);
    let height = 114.0 + rows as f32 * 18.0;
    let top_h = rows as f32 * 18.0 + 17.0;

    if let Some(uv) = gui.sprite("generic_54") {
        let (_, split_y) = px_to_ndc(rect, 176.0, height, 0.0, top_h);
        push_tex_quad(
            &mut g,
            rect.0,
            split_y,
            rect.2,
            rect.3,
            crop_uv(uv, 0.0, 0.0, 176.0, top_h),
        );
        let bottom_h = 96.0;
        let (_, bottom_y) = px_to_ndc(rect, 176.0, height, 0.0, top_h + bottom_h);
        push_tex_quad(
            &mut g,
            rect.0,
            bottom_y,
            rect.2,
            split_y,
            crop_uv(uv, 0.0, 126.0, 176.0, bottom_h),
        );
    } else {
        push_color_quad(&mut c, rect.0, rect.1, rect.2, rect.3, [0.14, 0.14, 0.17]);
    }
    for (slot, uv) in items.iter().enumerate().take(rows * 9 + 36) {
        if let Some(uv) = uv {
            let (x0, y0, x1, y1) = container_slot_rect(rect, rows, slot);
            push_tex_quad(&mut t, x0, y0, x1, y1, *uv);
        }
    }
    (c, g, t)
}

/// NDC bounds of a furnace-family menu slot. Slots 0/1/2 are input, fuel and
/// output; slots 3..29 are the player main inventory and 30..38 the hotbar.
#[must_use]
pub fn furnace_slot_rect(rect: (f32, f32, f32, f32), slot: usize) -> (f32, f32, f32, f32) {
    let (px, py) = match slot {
        0 => (56.0, 17.0),
        1 => (56.0, 53.0),
        2 => (116.0, 35.0),
        3..=29 => {
            let i = slot - 3;
            (8.0 + (i % 9) as f32 * 18.0, 84.0 + (i / 9) as f32 * 18.0)
        }
        _ => (8.0 + (slot.min(38) - 30) as f32 * 18.0, 142.0),
    };
    let (lx, ty) = px_to_ndc(rect, 176.0, 166.0, px, py);
    let (rx, by) = px_to_ndc(rect, 176.0, 166.0, px + 16.0, py + 16.0);
    (lx, by, rx, ty)
}

/// NDC bounds for dispenser/dropper, brewing-stand and hopper menus.
#[must_use]
pub fn simple_container_rect(texture: &str, aspect: f32) -> (f32, f32, f32, f32) {
    let height = if texture == "hopper" { 133.0 } else { 166.0 };
    sprite_rect(
        0.0,
        0.0,
        0.78 * height / 166.0,
        176.0,
        height,
        aspect.max(0.01),
    )
}

/// Slot bounds for the simple workstation/container families.
#[must_use]
pub fn simple_container_slot_rect(
    rect: (f32, f32, f32, f32),
    texture: &str,
    slot: usize,
) -> (f32, f32, f32, f32) {
    let (container_slots, panel_h, main_y, hotbar_y) = match texture {
        "dispenser" => (9, 166.0, 84.0, 142.0),
        "crafting_table" => (10, 166.0, 84.0, 142.0),
        "enchanting_table" => (2, 166.0, 84.0, 142.0),
        "anvil" | "grindstone" | "cartography_table" => (3, 166.0, 84.0, 142.0),
        "smithing" => (4, 166.0, 84.0, 142.0),
        "loom" => (4, 166.0, 84.0, 142.0),
        "stonecutter" => (2, 166.0, 84.0, 142.0),
        "hopper" => (5, 133.0, 51.0, 109.0),
        _ => (5, 166.0, 84.0, 142.0),
    };
    let (px, py) = if slot < container_slots {
        match texture {
            "dispenser" => (
                62.0 + (slot % 3) as f32 * 18.0,
                17.0 + (slot / 3) as f32 * 18.0,
            ),
            "hopper" => (44.0 + slot as f32 * 18.0, 20.0),
            "brewing_stand" => {
                const BREWING: [(f32, f32); 5] = [
                    (56.0, 51.0),
                    (79.0, 58.0),
                    (102.0, 51.0),
                    (79.0, 17.0),
                    (17.0, 17.0),
                ];
                BREWING[slot]
            }
            "crafting_table" => {
                if slot == 0 {
                    (124.0, 35.0)
                } else {
                    let grid = slot - 1;
                    (
                        30.0 + (grid % 3) as f32 * 18.0,
                        17.0 + (grid / 3) as f32 * 18.0,
                    )
                }
            }
            "enchanting_table" => [(15.0, 47.0), (35.0, 47.0)][slot],
            "anvil" => [(27.0, 47.0), (76.0, 47.0), (134.0, 47.0)][slot],
            "grindstone" => [(49.0, 19.0), (49.0, 40.0), (129.0, 34.0)][slot],
            "smithing" => [(8.0, 48.0), (26.0, 48.0), (44.0, 48.0), (98.0, 48.0)][slot],
            "cartography_table" => [(15.0, 15.0), (15.0, 52.0), (145.0, 39.0)][slot],
            "loom" => [(13.0, 26.0), (33.0, 26.0), (23.0, 45.0), (143.0, 57.0)][slot],
            "stonecutter" => [(20.0, 33.0), (143.0, 33.0)][slot],
            _ => (8.0, 8.0),
        }
    } else {
        let player = slot - container_slots;
        if player < 27 {
            (
                8.0 + (player % 9) as f32 * 18.0,
                main_y + (player / 9) as f32 * 18.0,
            )
        } else {
            (8.0 + (player.min(35) - 27) as f32 * 18.0, hotbar_y)
        }
    };
    panel_pixel_rect(rect, 176.0, panel_h, px, py, 16.0, 16.0)
}

/// Clickable bounds of enchanting offer `0..=2`.
#[must_use]
pub fn enchantment_option_rect(rect: (f32, f32, f32, f32), option: usize) -> (f32, f32, f32, f32) {
    panel_pixel_rect(
        rect,
        176.0,
        166.0,
        60.0,
        14.0 + option.min(2) as f32 * 19.0,
        108.0,
        19.0,
    )
}

/// Builds dispenser/dropper, brewing-stand, or hopper menu geometry.
#[must_use]
pub fn simple_container_geometry(
    gui: &GuiAtlas,
    items: &[Option<[f32; 4]>],
    texture: &str,
    aspect: f32,
) -> (Vec<ColorVertex>, Vec<TexVertex>, Vec<TexVertex>) {
    let mut color = Vec::new();
    let mut gui_vertices = Vec::new();
    let mut item_vertices = Vec::new();
    let rect = simple_container_rect(texture, aspect);
    if let Some(uv) = gui.sprite(texture) {
        push_tex_quad(&mut gui_vertices, rect.0, rect.1, rect.2, rect.3, uv);
    } else {
        push_color_quad(
            &mut color,
            rect.0,
            rect.1,
            rect.2,
            rect.3,
            [0.14, 0.14, 0.17],
        );
    }
    for (slot, uv) in items.iter().enumerate() {
        if let Some(uv) = uv {
            let (x0, y0, x1, y1) = simple_container_slot_rect(rect, texture, slot);
            push_tex_quad(&mut item_vertices, x0, y0, x1, y1, *uv);
        }
    }
    (color, gui_vertices, item_vertices)
}

fn panel_pixel_rect(
    rect: (f32, f32, f32, f32),
    panel_w: f32,
    panel_h: f32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> (f32, f32, f32, f32) {
    let (x0, y1) = px_to_ndc(rect, panel_w, panel_h, x, y);
    let (x1, y0) = px_to_ndc(rect, panel_w, panel_h, x + w, y + h);
    (x0, y0, x1, y1)
}

/// Builds a furnace, blast-furnace or smoker panel with server-driven flame and
/// cook-arrow progress. `properties` are remaining burn, total burn, cook, total cook.
#[must_use]
pub fn furnace_geometry(
    gui: &GuiAtlas,
    items: &[Option<[f32; 4]>],
    texture: &str,
    properties: [i16; 4],
    aspect: f32,
) -> (Vec<ColorVertex>, Vec<TexVertex>, Vec<TexVertex>) {
    let mut c = Vec::new();
    let mut g = Vec::new();
    let mut t = Vec::new();
    let rect = inventory_rect(aspect);
    if let Some(uv) = gui.sprite(texture) {
        push_tex_quad(&mut g, rect.0, rect.1, rect.2, rect.3, uv);
    } else {
        push_color_quad(&mut c, rect.0, rect.1, rect.2, rect.3, [0.14, 0.14, 0.17]);
    }

    let full_name = format!("{texture}_full");
    if let Some(full) = gui.sprite(&full_name) {
        let (lit_pixels, arrow_pixels) = furnace_progress_pixels(properties);
        if lit_pixels > 0 {
            let lit = lit_pixels as f32;
            let dest =
                panel_pixel_rect(rect, 176.0, 166.0, 56.0, 36.0 + 13.0 - lit, 14.0, lit + 1.0);
            let uv = crop_uv_dims(full, 176.0, 13.0 - lit, 14.0, lit + 1.0, 256.0, 256.0);
            push_tex_quad(&mut g, dest.0, dest.1, dest.2, dest.3, uv);
        }
        if arrow_pixels > 0 {
            let width = arrow_pixels as f32;
            let dest = panel_pixel_rect(rect, 176.0, 166.0, 79.0, 34.0, width, 16.0);
            let uv = crop_uv_dims(full, 176.0, 14.0, width, 16.0, 256.0, 256.0);
            push_tex_quad(&mut g, dest.0, dest.1, dest.2, dest.3, uv);
        }
    }
    for (slot, uv) in items.iter().enumerate().take(39) {
        if let Some(uv) = uv {
            let (x0, y0, x1, y1) = furnace_slot_rect(rect, slot);
            push_tex_quad(&mut t, x0, y0, x1, y1, *uv);
        }
    }
    (c, g, t)
}

fn furnace_progress_pixels(properties: [i16; 4]) -> (u8, u8) {
    let burn = f32::from(properties[0].max(0));
    let burn_total = f32::from(properties[1].max(0));
    let lit = if burn > 0.0 && burn_total > 0.0 {
        ((burn * 13.0 / burn_total).ceil() as i32).clamp(1, 13) as u8
    } else {
        0
    };
    let cook = f32::from(properties[2].max(0));
    let cook_total = f32::from(properties[3].max(0));
    let arrow = if cook > 0.0 && cook_total > 0.0 {
        (((cook * 24.0 / cook_total).floor() as i32).clamp(0, 24) + 1) as u8
    } else {
        0
    };
    (lit, arrow)
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
        let (_color, _g, item) = hud_geometry(&gui, 20.0, 20, 0.0, 0, 2, &hotbar, 1.0);
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
        let (_c, _g, t) = hud_geometry(&gui, 20.0, 20, 0.0, 0, 0, &hotbar, 1.0);
        // First vertex is the top-left corner -> (u0, v0).
        assert_eq!([t[0][2], t[0][3]], [0.1, 0.2]);
        // Third vertex is bottom-right -> (u1, v1).
        assert_eq!([t[2][2], t[2][3]], [0.3, 0.4]);
    }

    #[test]
    fn hotbar_pixels_remain_square_on_widescreen() {
        let aspect = 16.0 / 9.0;
        let bar = sprite_rect(0.0, -0.915, 0.13, 182.0, 22.0, aspect);
        let ppx = (bar.2 - bar.0) / 182.0;
        let ppy = (bar.3 - bar.1) / 22.0;
        // NDC X spans `aspect` times as many physical pixels as NDC Y.
        assert!((ppx * aspect - ppy).abs() < 1e-6);
    }

    #[test]
    fn inventory_icons_start_at_vanilla_slot_coordinates() {
        let panel = inventory_rect(1.0);
        let expected_top_left = px_to_ndc(panel, 176.0, 166.0, 8.0, 84.0);
        let expected_bottom_right = px_to_ndc(panel, 176.0, 166.0, 24.0, 100.0);
        let rect = inventory_slot_rect(panel, 9);
        assert_eq!((rect.0, rect.3), expected_top_left);
        assert_eq!((rect.2, rect.1), expected_bottom_right);
    }

    #[test]
    fn generic_container_geometry_places_all_slot_groups() {
        let gui = crab_assets::GuiAtlas::empty();
        let uv = [0.1, 0.2, 0.3, 0.4];
        let mut items = vec![None; 27 + 36];
        items[0] = Some(uv);
        items[27] = Some(uv);
        items[62] = Some(uv);
        let (_color, _gui, item) = container_geometry(&gui, &items, 3, 1.0);
        assert_eq!(item.len(), 18);

        let panel = container_rect(3, 1.0);
        for slot in [0, 26, 27, 53, 54, 62] {
            let rect = container_slot_rect(panel, 3, slot);
            assert!(rect.0 >= panel.0 && rect.2 <= panel.2);
            assert!(rect.1 >= panel.1 && rect.3 <= panel.3);
        }
    }

    #[test]
    fn furnace_layout_and_progress_are_vanilla_scaled() {
        let panel = inventory_rect(1.0);
        for slot in [0, 1, 2, 3, 29, 30, 38] {
            let rect = furnace_slot_rect(panel, slot);
            assert!(rect.0 >= panel.0 && rect.2 <= panel.2);
            assert!(rect.1 >= panel.1 && rect.3 <= panel.3);
        }
        assert_eq!(furnace_progress_pixels([100, 200, 50, 200]), (7, 7));
        assert_eq!(furnace_progress_pixels([0, 200, 0, 200]), (0, 0));

        let gui = crab_assets::GuiAtlas::empty();
        let mut items = vec![None; 39];
        items[0] = Some([0.0, 0.0, 1.0, 1.0]);
        items[38] = Some([0.0, 0.0, 1.0, 1.0]);
        let (_, _, item) = furnace_geometry(&gui, &items, "furnace", [0; 4], 1.0);
        assert_eq!(item.len(), 12);
    }

    #[test]
    fn simple_container_layouts_include_workstation_and_player_slots() {
        let gui = GuiAtlas::empty();
        for (texture, slots) in [
            ("dispenser", 45),
            ("brewing_stand", 41),
            ("crafting_table", 46),
            ("enchanting_table", 38),
            ("anvil", 39),
            ("grindstone", 39),
            ("smithing", 40),
            ("cartography_table", 39),
            ("loom", 40),
            ("stonecutter", 38),
            ("hopper", 41),
        ] {
            let items = vec![Some([0.0, 0.0, 1.0, 1.0]); slots];
            let (_, _, item) = simple_container_geometry(&gui, &items, texture, 1.0);
            assert_eq!(item.len(), slots * 6);
            let rect = simple_container_rect(texture, 1.0);
            let first = simple_container_slot_rect(rect, texture, 0);
            let last = simple_container_slot_rect(rect, texture, slots - 1);
            assert!(
                first.1 > last.1,
                "container slots should be above the hotbar"
            );
        }
    }

    #[test]
    fn status_effect_hud_skips_icons_missing_from_the_atlas() {
        assert!(status_effect_geometry(&GuiAtlas::empty(), &[1, 2, 33], 1.0).is_empty());
    }
}
