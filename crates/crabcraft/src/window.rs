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

use crab_assets::{Atlas, EntityAtlas, ItemAtlas};
use crab_render::{
    box_mesh, build_block_pipeline, build_hud_pipelines, entity_mesh, hud_geometry, mesh_region,
    upload_atlas, upload_texture, CameraUniform, HudPipelines, Vertex, DEPTH_FORMAT,
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

/// Builds vertices for tracked entities, split into (box, model): entities with
/// a loaded 3D model go in `model` (entity atlas), the rest in `box` (block
/// atlas white tile + colour).
fn build_entity_meshes(
    shared: &Shared,
    atlas: &Atlas,
    entity_atlas: &EntityAtlas,
) -> (Vec<Vertex>, Vec<Vertex>) {
    let entities = shared.entities.lock().unwrap();
    let white = atlas.white_uv();
    let dims = [entity_atlas.width as f32, entity_atlas.height as f32];
    let (mut box_v, mut model_v) = (Vec::new(), Vec::new());
    for e in entities.values() {
        if let Some(m) = entity_atlas.models.get(&e.type_id) {
            model_v.extend(entity_mesh(
                &m.geo,
                [e.x as f32, e.y as f32, e.z as f32],
                [m.atlas_x, m.atlas_y],
                dims,
            ));
        } else {
            let hw = f64::from(e.half_width);
            let min = [(e.x - hw) as f32, e.y as f32, (e.z - hw) as f32];
            let max = [
                (e.x + hw) as f32,
                (e.y + f64::from(e.height)) as f32,
                (e.z + hw) as f32,
            ];
            box_v.extend(box_mesh(min, max, white, box_color(e.type_id)));
        }
    }
    (box_v, model_v)
}

fn box_color(type_id: i32) -> [f32; 3] {
    let h = (type_id as u32).wrapping_mul(2_654_435_761);
    [
        0.4 + ((h >> 16) & 0xff) as f32 / 255.0 * 0.5,
        0.4 + ((h >> 8) & 0xff) as f32 / 255.0 * 0.5,
        0.4 + (h & 0xff) as f32 / 255.0 * 0.5,
    ]
}

/// Builds the 9 hotbar item-icon UVs from the player's inventory (slots 36..44)
/// using the item atlas, for `hud_geometry`.
fn hotbar_icons(shared: &Shared, item_atlas: &ItemAtlas) -> Vec<Option<[f32; 4]>> {
    let inv = shared.inventory.lock().unwrap();
    (0..9)
        .map(|i| {
            inv.get(36 + i).and_then(|slot| *slot).and_then(|it| {
                let id = u32::try_from(it.item_id).ok()?;
                let name = crab_registry::item_name(id)?;
                item_atlas.icon(name)
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
    hud_tex_buffer: Option<(wgpu::Buffer, u32)>,
    /// Item-icon atlas bound for the HUD's textured pass.
    item_atlas_bind_group: wgpu::BindGroup,
    /// Entity texture atlas (for 3D entity models).
    entity_atlas_bind_group: wgpu::BindGroup,
    /// Per-frame box mesh for entities lacking a model (block atlas texture).
    box_entity_buffer: Option<(wgpu::Buffer, u32)>,
    /// Per-frame 3D model mesh for entities with a model (entity atlas texture).
    model_entity_buffer: Option<(wgpu::Buffer, u32)>,
}

impl Graphics {
    fn new(
        window: Arc<Window>,
        atlas: &Atlas,
        entity_atlas: &EntityAtlas,
        item_atlas: &ItemAtlas,
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

        let hud = build_hud_pipelines(&device, config.format);
        let item_atlas_bind_group = upload_texture(
            &device,
            &queue,
            &hud.atlas_layout,
            &item_atlas.rgba,
            item_atlas.width,
            item_atlas.height,
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
            hud_tex_buffer: None,
            item_atlas_bind_group,
            entity_atlas_bind_group,
            box_entity_buffer: None,
            model_entity_buffer: None,
        }
    }

    fn set_hud(&mut self, color_verts: &[[f32; 5]], tex_verts: &[[f32; 4]]) {
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
        self.hud_tex_buffer = (!tex_verts.is_empty()).then(|| {
            let buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("hud tex"),
                    contents: bytemuck::cast_slice(tex_verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            (buffer, tex_verts.len() as u32)
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
            // HUD overlay: coloured pass (crosshair + hotbar + bars) then the
            // textured pass (item icons sampled from the item atlas).
            if let Some((buf, count)) = &self.hud_color_buffer {
                pass.set_pipeline(&self.hud.color);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..*count, 0..1);
            }
            if let Some((buf, count)) = &self.hud_tex_buffer {
                pass.set_pipeline(&self.hud.textured);
                pass.set_bind_group(0, &self.item_atlas_bind_group, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..*count, 0..1);
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
    /// Finished chunk meshes from the background mesher thread.
    mesh_rx: Receiver<((i32, i32), Vec<Vertex>)>,
    gfx: Option<Graphics>,
    /// Look angles in degrees (Minecraft convention).
    yaw: f32,
    pitch: f32,
    look_init: bool,
    keys: HashSet<KeyCode>,
    last_frame: Instant,
}

impl App {
    fn new(
        shared: Arc<Shared>,
        atlas: Arc<Atlas>,
        entity_atlas: Arc<EntityAtlas>,
        item_atlas: Arc<ItemAtlas>,
        mesh_rx: Receiver<((i32, i32), Vec<Vertex>)>,
    ) -> Self {
        Self {
            shared,
            atlas,
            entity_atlas,
            item_atlas,
            mesh_rx,
            gfx: None,
            yaw: 0.0,
            pitch: 0.0,
            look_init: false,
            keys: HashSet::new(),
            last_frame: Instant::now(),
        }
    }

    /// Updates look from arrow keys and publishes movement intent to the shared
    /// `Controls` for the network thread to apply via physics.
    fn update_input(&mut self, dt: f32) {
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

        let axis = |pos: KeyCode, neg: KeyCode| (pressed(pos) as i32 - pressed(neg) as i32) as f32;
        let mut controls = self.shared.controls.lock().unwrap();
        controls.forward = axis(KeyCode::KeyW, KeyCode::KeyS);
        controls.strafe = axis(KeyCode::KeyD, KeyCode::KeyA);
        controls.jump = pressed(KeyCode::Space);
        controls.yaw = self.yaw;
        controls.pitch = self.pitch;
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
        ));
        self.last_frame = Instant::now();
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
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
                        ..
                    },
                ..
            } => {
                if code == KeyCode::Escape {
                    event_loop.exit();
                } else if state == ElementState::Pressed {
                    self.keys.insert(code);
                } else {
                    self.keys.remove(&code);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
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

                let (box_v, model_v) =
                    build_entity_meshes(&self.shared, &self.atlas, &self.entity_atlas);
                let hotbar = hotbar_icons(&self.shared, &self.item_atlas);
                let selected = player.selected_slot as usize;
                if let Some(gfx) = self.gfx.as_mut() {
                    let aspect = gfx.aspect();
                    let eye = Vec3::new(player.x as f32, player.y as f32, player.z as f32);
                    let camera = first_person_camera(eye, self.yaw, self.pitch, aspect);
                    gfx.box_entity_buffer = gfx.make_vertex_buffer(&box_v);
                    gfx.model_entity_buffer = gfx.make_vertex_buffer(&model_v);
                    let (hud_c, hud_t) =
                        hud_geometry(player.health, player.food, selected, &hotbar, aspect);
                    gfx.set_hud(&hud_c, &hud_t);
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
) -> anyhow::Result<()> {
    let atlas = Arc::new(atlas);
    let entity_atlas = Arc::new(entity_atlas);
    let item_atlas = Arc::new(item_atlas);
    // Spawn the background mesher.
    let (mesh_tx, mesh_rx) = std::sync::mpsc::channel();
    {
        let shared = Arc::clone(&shared);
        let atlas = Arc::clone(&atlas);
        std::thread::spawn(move || mesher_loop(shared, atlas, mesh_tx));
    }

    let event_loop = EventLoop::new()?;
    let mut app = App::new(shared, atlas, entity_atlas, item_atlas, mesh_rx);
    event_loop.run_app(&mut app)?;
    Ok(())
}
