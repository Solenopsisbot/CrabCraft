//! Live windowed renderer: a winit window + wgpu surface that draws the world
//! from [`Shared`] as a first-person player, with cached per-chunk meshes that
//! rebuild only when a chunk changes (drained from `Shared::dirty_chunks`).
//!
//! NOTE: this needs a display to actually run; it is compile-verified here but
//! not click-tested in the headless build environment.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crab_assets::{Atlas, EntityAtlas, GuiAtlas, ItemAtlas};
use crab_render::{
    box_mesh, build_block_pipeline, build_hud_pipelines, entity_mesh, hud_geometry,
    inventory_geometry, mesh_region, upload_atlas, upload_texture, CameraUniform, HudPipelines,
    Vertex, DEPTH_FORMAT,
};
use glam::Vec3;
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::client::Shared;

/// Max chunk columns re-meshed per frame (bounds per-frame CPU during loads).
const REMESH_BUDGET: usize = 4;
const LOOK_SPEED: f32 = 110.0; // degrees/sec (arrow-key look)
const EYE_HEIGHT: f32 = 1.62;

/// First-person camera: eye at the player's head, looking along yaw/pitch
/// (Minecraft convention, degrees). Position comes from the player; this just
/// holds the look angles.
fn first_person_camera(
    player_pos: Vec3,
    yaw_deg: f32,
    pitch_deg: f32,
    aspect: f32,
) -> crab_render::Camera {
    let eye = player_pos + Vec3::new(0.0, EYE_HEIGHT, 0.0);
    let (yaw, pitch) = (yaw_deg.to_radians(), pitch_deg.to_radians());
    let dir = Vec3::new(
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    );
    crab_render::Camera {
        eye,
        target: eye + dir,
        up: Vec3::Y,
        aspect,
        fovy_radians: 70f32.to_radians(),
        znear: 0.1,
        zfar: 1000.0,
    }
}

/// A camera-facing quad (billboard) of an item icon at `pos`, for dropped items.
fn push_item_billboard(out: &mut Vec<Vertex>, pos: [f32; 3], uv: [f32; 4], yaw_deg: f32) {
    let s = 0.2;
    let yr = yaw_deg.to_radians();
    let r = [yr.cos() * s, 0.0, yr.sin() * s];
    let up = [0.0, s, 0.0];
    let c = [pos[0], pos[1] + 0.25, pos[2]];
    let n = [yr.sin(), 0.4, -yr.cos()];
    let [u0, v0, u1, v1] = uv;
    let corner = |sx: f32, sy: f32, u, v| {
        (
            [
                c[0] + sx * r[0] + sy * up[0],
                c[1] + sy * up[1],
                c[2] + sx * r[2] + sy * up[2],
            ],
            [u, v],
        )
    };
    let tl = corner(-1.0, 1.0, u0, v0);
    let tr = corner(1.0, 1.0, u1, v0);
    let br = corner(1.0, -1.0, u1, v1);
    let bl = corner(-1.0, -1.0, u0, v1);
    for (p, t) in [tl, tr, br, tl, br, bl] {
        out.push(Vertex {
            position: p,
            normal: n,
            uv: t,
            tint: [1.0, 1.0, 1.0],
        });
    }
}

fn box_color(type_id: i32) -> [f32; 3] {
    let h = (type_id as u32).wrapping_mul(2_654_435_761);
    [
        0.4 + ((h >> 16) & 0xff) as f32 / 255.0 * 0.5,
        0.4 + ((h >> 8) & 0xff) as f32 / 255.0 * 0.5,
        0.4 + (h & 0xff) as f32 / 255.0 * 0.5,
    ]
}

/// Builds the 9 hotbar item-icon UVs from the player's inventory (slots 36..44).
fn hotbar_icons(shared: &Shared, item_atlas: &ItemAtlas) -> Vec<Option<[f32; 4]>> {
    let inv = shared.inventory.lock().unwrap();
    (0..9)
        .map(|i| {
            inv.get(36 + i).and_then(|s| *s).and_then(|it| {
                let id = u32::try_from(it.item_id).ok()?;
                item_atlas.icon(crab_registry::item_name(id)?)
            })
        })
        .collect()
}

fn push_color_quad(v: &mut Vec<[f32; 5]>, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 3]) {
    for [px, py] in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]] {
        v.push([px, py, c[0], c[1], c[2]]);
    }
}

/// Pushes a textured 2D quad (item-atlas UV) into a HUD textured stream.
fn push_tex2d(v: &mut Vec<[f32; 4]>, x0: f32, y0: f32, x1: f32, y1: f32, uv: [f32; 4]) {
    let [u0, v0, u1, v1] = uv;
    for q in [
        [x0, y1, u0, v0],
        [x1, y1, u1, v0],
        [x1, y0, u1, v1],
        [x0, y1, u0, v0],
        [x1, y0, u1, v1],
        [x0, y0, u0, v1],
    ] {
        v.push(q);
    }
}

/// Builds chat geometry: recent log lines (and the input line when open) as
/// dark bars + bitmap text near the bottom-left. Returns `(color, text)`.
fn chat_geometry(
    shared: &Shared,
    gui: &GuiAtlas,
    chat_open: bool,
    buffer: &str,
    aspect: f32,
) -> (Vec<[f32; 5]>, Vec<[f32; 4]>) {
    let mut color = Vec::new();
    let mut text = Vec::new();
    let line_h = 0.038;
    let x = -0.96;
    let y_in = -0.52; // input-line top (above the hotbar)
    let hscale = (line_h / 8.0) / aspect.max(0.01);
    let bar = |color: &mut Vec<[f32; 5]>, s: &str, y: f32| {
        let w = (gui.text_width(s) * hscale).max(0.05);
        push_color_quad(
            color,
            x - 0.006,
            y - line_h,
            x + w + 0.006,
            y,
            [0.05, 0.05, 0.05],
        );
    };

    let log = shared.chat_log.lock().unwrap();
    let shown = if chat_open { 10 } else { 7 };
    for (i, line) in log.iter().rev().take(shown).enumerate() {
        let y = y_in - (i as f32 + 1.0) * (line_h * 1.25);
        bar(&mut color, line, y);
        crab_render::push_text(&mut text, gui, line, x, y, line_h, aspect);
    }
    if chat_open {
        let s = format!("> {buffer}");
        bar(&mut color, &s, y_in);
        crab_render::push_text(&mut text, gui, &s, x, y_in, line_h, aspect);
    }
    (color, text)
}

/// Builds the stack-size number text (font quads) for the hotbar, and for the
/// inventory grid when it's open. Counts of 1 are not shown.
fn count_text(shared: &Shared, gui: &GuiAtlas, aspect: f32, inv_open: bool) -> Vec<[f32; 4]> {
    let inv = shared.inventory.lock().unwrap();
    let mut out = Vec::new();
    let mut push_count = |count: i8, rect: (f32, f32, f32, f32)| {
        if count <= 1 {
            return;
        }
        let s = count.to_string();
        let (_x0, y0, x1, y1) = rect;
        let h = (y1 - y0) * 0.5;
        let w = gui.text_width(&s) * (h / 8.0) / aspect.max(0.01);
        crab_render::push_text(&mut out, gui, &s, x1 - w, y0 + h, h, aspect);
    };
    for i in 0..9 {
        if let Some(it) = inv.get(36 + i).and_then(|s| *s) {
            push_count(it.count, crab_render::hotbar_slot_rect(aspect, i));
        }
    }
    if inv_open {
        let rect = crab_render::inventory_rect(aspect);
        for slot in 0..46 {
            if let Some(it) = inv.get(slot).and_then(|s| *s) {
                push_count(it.count, crab_render::inventory_slot_rect(rect, slot));
            }
        }
    }
    out
}

/// Builds the item-icon UVs for all 46 player-inventory window slots.
fn inventory_icons(shared: &Shared, item_atlas: &ItemAtlas) -> Vec<Option<[f32; 4]>> {
    let inv = shared.inventory.lock().unwrap();
    (0..46)
        .map(|i| {
            inv.get(i).and_then(|s| *s).and_then(|it| {
                let id = u32::try_from(it.item_id).ok()?;
                item_atlas.icon(crab_registry::item_name(id)?)
            })
        })
        .collect()
}

/// GPU + window resources, created once the event loop is `resumed`.
struct Graphics {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    atlas_bind_group: wgpu::BindGroup,
    depth_view: wgpu::TextureView,
    /// Cached vertex buffer per chunk column.
    chunk_meshes: HashMap<(i32, i32), (wgpu::Buffer, u32)>,
    hud: HudPipelines,
    hud_color_buffer: Option<(wgpu::Buffer, u32)>,
    /// HUD GUI-sprite verts (hotbar/inventory backgrounds, gui atlas).
    hud_gui_buffer: Option<(wgpu::Buffer, u32)>,
    /// HUD item-icon verts (item atlas).
    hud_item_buffer: Option<(wgpu::Buffer, u32)>,
    /// HUD text verts (font glyphs, gui atlas) drawn last.
    hud_text_buffer: Option<(wgpu::Buffer, u32)>,
    /// Item-icon atlas bound for the HUD's textured pass.
    item_atlas_bind_group: wgpu::BindGroup,
    /// GUI sprite + font atlas bound for the HUD's gui/text passes.
    gui_atlas_bind_group: wgpu::BindGroup,
    /// Entity texture atlas (for 3D entity models).
    entity_atlas_bind_group: wgpu::BindGroup,
    /// Item atlas bound in the *world* pass for dropped-item billboards.
    item_world_bind_group: wgpu::BindGroup,
    /// Per-frame box mesh for entities lacking a model (block atlas texture).
    box_entity_buffer: Option<(wgpu::Buffer, u32)>,
    /// Per-frame 3D model mesh for entities with a model (entity atlas texture).
    model_entity_buffer: Option<(wgpu::Buffer, u32)>,
    /// Per-frame dropped-item billboards (item atlas texture).
    item_entity_buffer: Option<(wgpu::Buffer, u32)>,
}

impl Graphics {
    fn new(
        window: Arc<Window>,
        atlas: &Atlas,
        entity_atlas: &EntityAtlas,
        item_atlas: &ItemAtlas,
        gui_atlas: &GuiAtlas,
    ) -> Self {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("no adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("crabcraft device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let (pipeline, camera_bgl, texture_bgl) = build_block_pipeline(&device, config.format);
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::cast_slice(&[CameraUniform {
                view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let atlas_bind_group = upload_atlas(&device, &queue, &texture_bgl, atlas);
        let entity_atlas_bind_group = upload_texture(
            &device,
            &queue,
            &texture_bgl,
            &entity_atlas.rgba,
            entity_atlas.width,
            entity_atlas.height,
        );

        // Item atlas bound in the world pass (block-pipeline layout) for
        // dropped-item billboards.
        let item_world_bind_group = upload_texture(
            &device,
            &queue,
            &texture_bgl,
            &item_atlas.rgba,
            item_atlas.width,
            item_atlas.height,
        );

        let hud = build_hud_pipelines(&device, config.format);
        let item_atlas_bind_group = upload_texture(
            &device,
            &queue,
            &hud.atlas_layout,
            &item_atlas.rgba,
            item_atlas.width,
            item_atlas.height,
        );
        let gui_atlas_bind_group = upload_texture(
            &device,
            &queue,
            &hud.atlas_layout,
            &gui_atlas.rgba,
            gui_atlas.width,
            gui_atlas.height,
        );

        let depth_view = create_depth(&device, width, height);

        Self {
            window,
            surface,
            device,
            queue,
            config,
            pipeline,
            camera_buffer,
            camera_bind_group,
            atlas_bind_group,
            depth_view,
            chunk_meshes: HashMap::new(),
            hud,
            hud_color_buffer: None,
            hud_gui_buffer: None,
            hud_item_buffer: None,
            hud_text_buffer: None,
            item_atlas_bind_group,
            gui_atlas_bind_group,
            entity_atlas_bind_group,
            item_world_bind_group,
            box_entity_buffer: None,
            model_entity_buffer: None,
            item_entity_buffer: None,
        }
    }

    fn set_hud(
        &mut self,
        color_verts: &[[f32; 5]],
        gui_verts: &[[f32; 4]],
        item_verts: &[[f32; 4]],
        text_verts: &[[f32; 4]],
    ) {
        let tex_buf = |device: &wgpu::Device, label: &str, verts: &[[f32; 4]]| {
            (!verts.is_empty()).then(|| {
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytemuck::cast_slice(verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                (buffer, verts.len() as u32)
            })
        };
        self.hud_gui_buffer = tex_buf(&self.device, "hud gui", gui_verts);
        self.hud_item_buffer = tex_buf(&self.device, "hud item", item_verts);
        self.hud_text_buffer = tex_buf(&self.device, "hud text", text_verts);
        self.hud_color_buffer = (!color_verts.is_empty()).then(|| {
            let buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("hud color"),
                    contents: bytemuck::cast_slice(color_verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            (buffer, color_verts.len() as u32)
        });
    }

    fn make_vertex_buffer(&self, vertices: &[Vertex]) -> Option<(wgpu::Buffer, u32)> {
        if vertices.is_empty() {
            return None;
        }
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("entity vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        Some((buffer, vertices.len() as u32))
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth(&self.device, width, height);
    }

    fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height as f32
    }

    fn upload_chunk(&mut self, coord: (i32, i32), vertices: &[Vertex]) {
        if vertices.is_empty() {
            self.chunk_meshes.remove(&coord);
            return;
        }
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        self.chunk_meshes
            .insert(coord, (buffer, vertices.len() as u32));
    }

    fn render(&mut self, camera: &crab_render::Camera) {
        let uniform = CameraUniform::new(camera);
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&[uniform]));

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.45,
                            g: 0.62,
                            b: 0.92,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(1, &self.atlas_bind_group, &[]);
            for (buffer, count) in self.chunk_meshes.values() {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            // Box entities (no model) — block atlas still bound at group 1.
            if let Some((buffer, count)) = &self.box_entity_buffer {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            // 3D model entities — rebind group 1 to the entity texture atlas.
            if let Some((buffer, count)) = &self.model_entity_buffer {
                pass.set_bind_group(1, &self.entity_atlas_bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            // Dropped-item billboards — rebind group 1 to the item atlas.
            if let Some((buffer, count)) = &self.item_entity_buffer {
                pass.set_bind_group(1, &self.item_world_bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            // HUD overlay: coloured quads, then GUI sprites (gui atlas), item
            // icons (item atlas), and text (gui atlas) — in that order so text
            // sits on top of icons which sit on top of backgrounds.
            if let Some((buf, count)) = &self.hud_color_buffer {
                pass.set_pipeline(&self.hud.color);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..*count, 0..1);
            }
            pass.set_pipeline(&self.hud.textured);
            for (buffer, bind) in [
                (&self.hud_gui_buffer, &self.gui_atlas_bind_group),
                (&self.hud_item_buffer, &self.item_atlas_bind_group),
                (&self.hud_text_buffer, &self.gui_atlas_bind_group),
            ] {
                if let Some((buf, count)) = buffer {
                    pass.set_bind_group(0, bind, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}

fn create_depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

struct App {
    shared: Arc<Shared>,
    atlas: Arc<Atlas>,
    entity_atlas: Arc<EntityAtlas>,
    item_atlas: Arc<ItemAtlas>,
    gui_atlas: Arc<GuiAtlas>,
    /// Finished chunk meshes from the background mesher thread.
    mesh_rx: Receiver<((i32, i32), Vec<Vertex>)>,
    gfx: Option<Graphics>,
    /// Look angles in degrees (Minecraft convention).
    yaw: f32,
    pitch: f32,
    look_init: bool,
    keys: HashSet<KeyCode>,
    last_frame: Instant,
    /// Selected hotbar slot (0..=8), driven by number keys / scroll.
    selected_slot: u8,
    /// Whether the inventory panel is open (E toggles).
    inventory_open: bool,
    /// Chat input state: open + the line being typed.
    chat_open: bool,
    chat_buffer: String,
    /// Cursor position in NDC (for inventory slot hit-testing).
    cursor: (f32, f32),
    /// Per-entity smoothed render state (interpolation + walk animation).
    entity_anim: HashMap<i32, EntityAnim>,
    /// Smoothed camera eye position (eases toward the player's stepped pos).
    render_eye: Option<Vec3>,
}

/// Smoothed render state for one entity.
#[derive(Clone, Copy)]
struct EntityAnim {
    pos: [f32; 3],
    /// Accumulated walk-cycle phase (radians).
    phase: f32,
    /// Smoothed limb-swing amplitude (0 = still).
    amount: f32,
}

impl App {
    fn new(
        shared: Arc<Shared>,
        atlas: Arc<Atlas>,
        entity_atlas: Arc<EntityAtlas>,
        item_atlas: Arc<ItemAtlas>,
        gui_atlas: Arc<GuiAtlas>,
        mesh_rx: Receiver<((i32, i32), Vec<Vertex>)>,
    ) -> Self {
        Self {
            shared,
            atlas,
            entity_atlas,
            item_atlas,
            gui_atlas,
            mesh_rx,
            gfx: None,
            yaw: 0.0,
            pitch: 0.0,
            look_init: false,
            keys: HashSet::new(),
            last_frame: Instant::now(),
            selected_slot: 0,
            inventory_open: false,
            chat_open: false,
            chat_buffer: String::new(),
            cursor: (0.0, 0.0),
            entity_anim: HashMap::new(),
            render_eye: None,
        }
    }

    /// Advances per-entity interpolation + walk animation by `dt` and builds the
    /// entity meshes: `(box, model, item-billboard)`.
    fn step_entities(&mut self, dt: f32) -> (Vec<Vertex>, Vec<Vertex>, Vec<Vertex>) {
        let entities = self.shared.entities.lock().unwrap();
        let ease = 1.0 - (-dt * 12.0).exp();
        let mut alive = HashSet::new();
        for (&id, e) in entities.iter() {
            alive.insert(id);
            let target = [e.x as f32, e.y as f32, e.z as f32];
            let a = self.entity_anim.entry(id).or_insert(EntityAnim {
                pos: target,
                phase: 0.0,
                amount: 0.0,
            });
            let before = a.pos;
            for (k, &t) in target.iter().enumerate() {
                a.pos[k] += (t - a.pos[k]) * ease;
            }
            let (dx, dz) = (a.pos[0] - before[0], a.pos[2] - before[2]);
            let moved = (dx * dx + dz * dz).sqrt();
            a.phase += moved * 2.2;
            let target_amount = (moved / dt.max(1e-3) * 0.10).min(0.7);
            a.amount += (target_amount - a.amount) * ease;
        }
        self.entity_anim.retain(|id, _| alive.contains(id));

        let white = self.atlas.white_uv();
        let dims = [
            self.entity_atlas.width as f32,
            self.entity_atlas.height as f32,
        ];
        let (mut box_v, mut model_v, mut item_v) = (Vec::new(), Vec::new(), Vec::new());
        for (&id, e) in entities.iter() {
            let a = self.entity_anim[&id];
            // Dropped item: a camera-facing billboard of its icon.
            if let Some(item_id) = e.item {
                if let Some(uv) = u32::try_from(item_id)
                    .ok()
                    .and_then(crab_registry::item_name)
                    .and_then(|n| self.item_atlas.icon(n))
                {
                    push_item_billboard(&mut item_v, a.pos, uv, self.yaw);
                    continue;
                }
            }
            if let Some(m) = self.entity_atlas.models.get(&e.type_id) {
                model_v.extend(entity_mesh(
                    &m.geo,
                    a.pos,
                    [m.atlas_x, m.atlas_y],
                    dims,
                    a.phase,
                    a.amount,
                    e.scale,
                ));
            } else {
                let hw = e.half_width;
                let min = [a.pos[0] - hw, a.pos[1], a.pos[2] - hw];
                let max = [a.pos[0] + hw, a.pos[1] + e.height, a.pos[2] + hw];
                box_v.extend(box_mesh(min, max, white, box_color(e.type_id)));
            }
        }
        (box_v, model_v, item_v)
    }

    /// Hit-tests the cursor against the inventory grid and queues a click on the
    /// matching inventory slot (panel slot p -> inventory slot 9+p).
    fn inventory_click(&self, button: i8) {
        let aspect = self.gfx.as_ref().map_or(1.0, Graphics::aspect);
        let rect = crab_render::inventory_rect(aspect);
        let (cx, cy) = self.cursor;
        for slot in 0..46usize {
            let (x0, y0, x1, y1) = crab_render::inventory_slot_rect(rect, slot);
            if cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1 {
                self.shared
                    .click_outbox
                    .lock()
                    .unwrap()
                    .push((slot as i16, button));
                return;
            }
        }
    }

    /// Opens/closes the inventory, freeing or recapturing the cursor.
    fn set_inventory_open(&mut self, open: bool) {
        self.inventory_open = open;
        if let Some(gfx) = self.gfx.as_ref() {
            if open {
                let _ = gfx.window.set_cursor_grab(CursorGrabMode::None);
                gfx.window.set_cursor_visible(true);
            } else {
                let _ = gfx
                    .window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| gfx.window.set_cursor_grab(CursorGrabMode::Confined));
                gfx.window.set_cursor_visible(false);
            }
        }
    }

    /// Updates look from arrow keys and publishes movement intent to the shared
    /// `Controls` for the network thread to apply via physics.
    fn update_input(&mut self, dt: f32) {
        // While the inventory or chat is open, freeze movement/look input.
        if self.inventory_open || self.chat_open {
            let mut controls = self.shared.controls.lock().unwrap();
            controls.forward = 0.0;
            controls.strafe = 0.0;
            controls.jump = false;
            return;
        }
        let pressed = |c: KeyCode| self.keys.contains(&c);
        if pressed(KeyCode::ArrowLeft) {
            self.yaw -= LOOK_SPEED * dt;
        }
        if pressed(KeyCode::ArrowRight) {
            self.yaw += LOOK_SPEED * dt;
        }
        if pressed(KeyCode::ArrowUp) {
            self.pitch = (self.pitch - LOOK_SPEED * dt).clamp(-89.0, 89.0);
        }
        if pressed(KeyCode::ArrowDown) {
            self.pitch = (self.pitch + LOOK_SPEED * dt).clamp(-89.0, 89.0);
        }

        // Number keys 1..9 select a hotbar slot.
        const DIGITS: [KeyCode; 9] = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ];
        for (i, key) in DIGITS.iter().enumerate() {
            if pressed(*key) {
                self.selected_slot = i as u8;
            }
        }

        let axis = |pos: KeyCode, neg: KeyCode| (pressed(pos) as i32 - pressed(neg) as i32) as f32;
        let mut controls = self.shared.controls.lock().unwrap();
        controls.forward = axis(KeyCode::KeyW, KeyCode::KeyS);
        controls.strafe = axis(KeyCode::KeyD, KeyCode::KeyA);
        controls.jump = pressed(KeyCode::Space);
        controls.yaw = self.yaw;
        controls.pitch = self.pitch;
        controls.selected_slot = self.selected_slot;
    }

    /// Uploads chunk meshes finished by the background mesher (GPU upload only;
    /// the actual meshing happens off the render thread, so frames stay smooth).
    fn process_meshes(&mut self) {
        let Some(gfx) = self.gfx.as_mut() else {
            return;
        };
        for _ in 0..REMESH_BUDGET {
            match self.mesh_rx.try_recv() {
                Ok((coord, verts)) => gfx.upload_chunk(coord, &verts),
                Err(_) => break,
            }
        }
    }
}

/// Background thread: drains dirty chunks, meshes them (skipping air-only
/// sections), and ships the vertices to the render thread. Keeps the heavy
/// meshing work off the frame loop.
fn mesher_loop(shared: Arc<Shared>, atlas: Arc<Atlas>, tx: Sender<((i32, i32), Vec<Vertex>)>) {
    while shared.running.load(Ordering::SeqCst) {
        let batch: Vec<(i32, i32)> = {
            let mut dirty = shared.dirty_chunks.lock().unwrap();
            let take: Vec<_> = dirty.iter().take(8).copied().collect();
            for c in &take {
                dirty.remove(c);
            }
            take
        };
        if batch.is_empty() {
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }
        for (cx, cz) in batch {
            let verts = {
                let world = shared.world.lock().unwrap();
                match world.occupied_y_bounds(cx, cz) {
                    Some((min_y, max_y)) => {
                        mesh_region(
                            &world,
                            &atlas,
                            [cx * 16, min_y, cz * 16],
                            [cx * 16 + 15, max_y, cz * 16 + 15],
                        )
                        .vertices
                    }
                    None => Vec::new(),
                }
            };
            if tx.send(((cx, cz), verts)).is_err() {
                return;
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Crabcraft")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        // Capture the cursor for mouse-look (Locked on macOS; Confined elsewhere).
        let _ = window
            .set_cursor_grab(CursorGrabMode::Locked)
            .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
        window.set_cursor_visible(false);
        self.gfx = Some(Graphics::new(
            window,
            &self.atlas,
            &self.entity_atlas,
            &self.item_atlas,
            &self.gui_atlas,
        ));
        self.last_frame = Instant::now();
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if self.inventory_open || self.chat_open {
            return; // don't turn the view while a UI is focused
        }
        if let DeviceEvent::MouseMotion { delta } = event {
            const SENSITIVITY: f32 = 0.12; // degrees per pixel
            self.yaw += delta.0 as f32 * SENSITIVITY;
            self.pitch = (self.pitch + delta.1 as f32 * SENSITIVITY).clamp(-89.0, 89.0);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gfx) = self.gfx.as_mut() {
                    gfx.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat,
                        text,
                        ..
                    },
                ..
            } => {
                let pressed = state == ElementState::Pressed;
                // Chat input swallows all keys while open.
                if self.chat_open {
                    if pressed {
                        match code {
                            KeyCode::Escape => {
                                self.chat_open = false;
                                self.chat_buffer.clear();
                            }
                            KeyCode::Enter | KeyCode::NumpadEnter => {
                                let msg = std::mem::take(&mut self.chat_buffer);
                                if !msg.trim().is_empty() {
                                    self.shared.chat_outbox.lock().unwrap().push(msg);
                                }
                                self.chat_open = false;
                            }
                            KeyCode::Backspace => {
                                self.chat_buffer.pop();
                            }
                            _ => {
                                if let Some(t) = &text {
                                    for ch in t.chars() {
                                        if !ch.is_control() && self.chat_buffer.len() < 200 {
                                            self.chat_buffer.push(ch);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return;
                }
                if code == KeyCode::Escape {
                    // Esc closes the inventory first, otherwise quits.
                    if self.inventory_open {
                        self.set_inventory_open(false);
                    } else {
                        event_loop.exit();
                    }
                } else if pressed {
                    if !self.inventory_open && !repeat && code == KeyCode::KeyT {
                        self.chat_open = true;
                        self.chat_buffer.clear();
                        return;
                    }
                    if !self.inventory_open && !repeat && code == KeyCode::Slash {
                        self.chat_open = true;
                        self.chat_buffer = "/".to_string();
                        return;
                    }
                    if code == KeyCode::KeyE && !repeat {
                        let open = !self.inventory_open;
                        self.set_inventory_open(open);
                    }
                    self.keys.insert(code);
                } else {
                    self.keys.remove(&code);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(gfx) = self.gfx.as_ref() {
                    let (w, h) = (
                        f64::from(gfx.config.width.max(1)),
                        f64::from(gfx.config.height.max(1)),
                    );
                    self.cursor = (
                        (position.x / w * 2.0 - 1.0) as f32,
                        (1.0 - position.y / h * 2.0) as f32,
                    );
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                // While the inventory is open, clicks move items, not the world.
                if self.inventory_open {
                    if pressed {
                        match button {
                            MouseButton::Left => self.inventory_click(0),
                            MouseButton::Right => self.inventory_click(1),
                            _ => {}
                        }
                    }
                    return;
                }
                let mut controls = self.shared.controls.lock().unwrap();
                match button {
                    // Left mouse is held: attack stays true until release so the
                    // network thread can run hold-to-dig / continuous attack.
                    MouseButton::Left => controls.attack = pressed,
                    // Right mouse places on press (edge-triggered).
                    MouseButton::Right if pressed => controls.use_item = true,
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Scroll cycles the hotbar slot (up = previous, down = next).
                let dy = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                if dy.abs() > 0.01 {
                    let step = if dy > 0.0 { 8 } else { 1 }; // +8 == -1 (mod 9)
                    self.selected_slot = (self.selected_slot + step) % 9;
                }
            }
            WindowEvent::RedrawRequested => {
                // Adopt the server's spawn look the first time we're placed.
                let player = *self.shared.player.lock().unwrap();
                if !self.look_init && player.spawned {
                    self.yaw = player.yaw;
                    self.pitch = player.pitch;
                    self.look_init = true;
                }

                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;
                self.update_input(dt);
                self.process_meshes();

                let (box_v, model_v, item_v) = self.step_entities(dt);
                let hotbar = hotbar_icons(&self.shared, &self.item_atlas);
                let inv_icons = self
                    .inventory_open
                    .then(|| inventory_icons(&self.shared, &self.item_atlas));
                let selected = player.selected_slot as usize;
                // Smooth the camera toward the 20 Hz-stepped player position.
                let target_eye = Vec3::new(player.x as f32, player.y as f32, player.z as f32);
                let eye = match self.render_eye {
                    Some(prev) if player.spawned => {
                        prev + (target_eye - prev) * (1.0 - (-dt * 22.0).exp())
                    }
                    _ => target_eye,
                };
                self.render_eye = Some(eye);
                if let Some(gfx) = self.gfx.as_mut() {
                    let aspect = gfx.aspect();
                    let camera = first_person_camera(eye, self.yaw, self.pitch, aspect);
                    gfx.box_entity_buffer = gfx.make_vertex_buffer(&box_v);
                    gfx.model_entity_buffer = gfx.make_vertex_buffer(&model_v);
                    gfx.item_entity_buffer = gfx.make_vertex_buffer(&item_v);
                    let gui = &self.gui_atlas;
                    let (mut hud_c, mut hud_g, mut hud_i) =
                        hud_geometry(gui, player.health, player.food, selected, &hotbar, aspect);
                    let mut hud_text = count_text(&self.shared, gui, aspect, self.inventory_open);
                    let (chat_c, chat_t) =
                        chat_geometry(&self.shared, gui, self.chat_open, &self.chat_buffer, aspect);
                    hud_c.extend(chat_c);
                    hud_text.extend(chat_t);
                    if let Some(inv) = &inv_icons {
                        let (ic, ig, ii) = inventory_geometry(gui, inv, aspect);
                        hud_c.extend(ic);
                        hud_g.extend(ig);
                        hud_i.extend(ii);
                        // Item held on the cursor, drawn at the mouse position.
                        if let Some(it) = *self.shared.carried.lock().unwrap() {
                            if let Some(uv) = u32::try_from(it.item_id)
                                .ok()
                                .and_then(crab_registry::item_name)
                                .and_then(|n| self.item_atlas.icon(n))
                            {
                                let (cx, cy) = self.cursor;
                                let s = 0.055;
                                let hw = s / aspect;
                                push_tex2d(&mut hud_i, cx - hw, cy - s, cx + hw, cy + s, uv);
                            }
                        }
                    }
                    gfx.set_hud(&hud_c, &hud_g, &hud_i, &hud_text);
                    gfx.render(&camera);
                }

                if !self
                    .shared
                    .running
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gfx) = &self.gfx {
            gfx.window.request_redraw();
        }
    }
}

/// Runs the windowed renderer (blocking; must be called on the main thread).
pub fn run(
    shared: Arc<Shared>,
    atlas: Atlas,
    entity_atlas: EntityAtlas,
    item_atlas: ItemAtlas,
    gui_atlas: GuiAtlas,
) -> anyhow::Result<()> {
    let atlas = Arc::new(atlas);
    let entity_atlas = Arc::new(entity_atlas);
    let item_atlas = Arc::new(item_atlas);
    let gui_atlas = Arc::new(gui_atlas);
    // Spawn the background mesher.
    let (mesh_tx, mesh_rx) = std::sync::mpsc::channel();
    {
        let shared = Arc::clone(&shared);
        let atlas = Arc::clone(&atlas);
        std::thread::spawn(move || mesher_loop(shared, atlas, mesh_tx));
    }

    let event_loop = EventLoop::new()?;
    let mut app = App::new(shared, atlas, entity_atlas, item_atlas, gui_atlas, mesh_rx);
    event_loop.run_app(&mut app)?;
    Ok(())
}
