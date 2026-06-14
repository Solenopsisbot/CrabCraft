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

use crab_assets::Atlas;
use crab_render::{
    build_block_pipeline, mesh_region, upload_atlas, CameraUniform, Vertex, DEPTH_FORMAT,
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

/// Builds box vertices for every tracked entity, tinted by entity colour.
fn entity_vertices(shared: &Shared, white_uv: [f32; 4]) -> Vec<Vertex> {
    let entities = shared.entities.lock().unwrap();
    let mut verts = Vec::new();
    for e in entities.values() {
        let hw = f64::from(e.half_width);
        let min = [(e.x - hw) as f32, e.y as f32, (e.z - hw) as f32];
        let max = [
            (e.x + hw) as f32,
            (e.y + f64::from(e.height)) as f32,
            (e.z + hw) as f32,
        ];
        verts.extend(crab_render::box_mesh(min, max, white_uv, e.color));
    }
    verts
}

/// A tiny white "+" drawn in screen-centre NDC, depth-test disabled.
const CROSSHAIR_WGSL: &str = "
@vertex fn vs(@location(0) p: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(p, 0.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.9, 0.9, 0.9, 1.0); }
";

/// Two thin bars (a "+") in NDC, kept pixel-square via `aspect`.
fn crosshair_vertices(aspect: f32) -> Vec<[f32; 2]> {
    let arm = 0.018;
    let thick = 0.0022;
    let (hx, hy) = (arm / aspect, thick);
    let (vx, vy) = (thick / aspect, arm);
    let quad = |x: f32, y: f32| [[-x, -y], [x, -y], [x, y], [-x, -y], [x, y], [-x, y]];
    let mut v = Vec::new();
    v.extend_from_slice(&quad(hx, hy));
    v.extend_from_slice(&quad(vx, vy));
    v
}

fn build_crosshair_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("crosshair shader"),
        source: wgpu::ShaderSource::Wgsl(CROSSHAIR_WGSL.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("crosshair layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    const ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("crosshair pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs",
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRS,
            }],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
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
    crosshair_pipeline: wgpu::RenderPipeline,
    crosshair_buffer: wgpu::Buffer,
    crosshair_count: u32,
    /// Per-frame entity box mesh (drawn with the block pipeline).
    entity_buffer: Option<(wgpu::Buffer, u32)>,
}

impl Graphics {
    fn new(window: Arc<Window>, atlas: &Atlas) -> Self {
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

        let crosshair_pipeline = build_crosshair_pipeline(&device, config.format);
        let crosshair_verts = crosshair_vertices(width as f32 / height as f32);
        let crosshair_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("crosshair"),
            contents: bytemuck::cast_slice(&crosshair_verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let crosshair_count = crosshair_verts.len() as u32;

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
            crosshair_pipeline,
            crosshair_buffer,
            crosshair_count,
            entity_buffer: None,
        }
    }

    fn upload_entities(&mut self, vertices: &[Vertex]) {
        if vertices.is_empty() {
            self.entity_buffer = None;
            return;
        }
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("entities"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        self.entity_buffer = Some((buffer, vertices.len() as u32));
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
            // Entities (same block pipeline + camera/atlas bind groups).
            if let Some((buffer, count)) = &self.entity_buffer {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            // Crosshair overlay (no camera/atlas bind groups).
            pass.set_pipeline(&self.crosshair_pipeline);
            pass.set_vertex_buffer(0, self.crosshair_buffer.slice(..));
            pass.draw(0..self.crosshair_count, 0..1);
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
        mesh_rx: Receiver<((i32, i32), Vec<Vertex>)>,
    ) -> Self {
        Self {
            shared,
            atlas,
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
        self.gfx = Some(Graphics::new(window, &self.atlas));
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
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                let mut controls = self.shared.controls.lock().unwrap();
                match button {
                    MouseButton::Left => controls.attack = true,
                    MouseButton::Right => controls.use_item = true,
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

                let entity_verts = entity_vertices(&self.shared, self.atlas.white_uv());
                if let Some(gfx) = self.gfx.as_mut() {
                    let aspect = gfx.aspect();
                    let eye = Vec3::new(player.x as f32, player.y as f32, player.z as f32);
                    let camera = first_person_camera(eye, self.yaw, self.pitch, aspect);
                    gfx.upload_entities(&entity_verts);
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
pub fn run(shared: Arc<Shared>, atlas: Atlas) -> anyhow::Result<()> {
    let atlas = Arc::new(atlas);
    // Spawn the background mesher.
    let (mesh_tx, mesh_rx) = std::sync::mpsc::channel();
    {
        let shared = Arc::clone(&shared);
        let atlas = Arc::clone(&atlas);
        std::thread::spawn(move || mesher_loop(shared, atlas, mesh_tx));
    }

    let event_loop = EventLoop::new()?;
    let mut app = App::new(shared, atlas, mesh_rx);
    event_loop.run_app(&mut app)?;
    Ok(())
}
