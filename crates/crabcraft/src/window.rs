//! Live windowed renderer: a winit window + wgpu surface that draws the world
//! from [`Shared`] with a free-fly camera (WASD + QE up/down, arrow keys look).
//!
//! The world is re-meshed on a short timer (not every frame) around the camera,
//! and the cached mesh is drawn each frame.
//!
//! NOTE: this needs a display to actually run; it is compile-verified here but
//! not click-tested in the headless build environment.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crab_render::{build_block_pipeline, mesh_region, CameraUniform, Vertex, DEPTH_FORMAT};
use glam::Vec3;
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::client::Shared;

const REMESH_INTERVAL: Duration = Duration::from_millis(300);
const MESH_RADIUS: i32 = 48;
const MOVE_SPEED: f32 = 24.0; // blocks/sec
const LOOK_SPEED: f32 = 1.6; // rad/sec

/// A free-fly camera (decoupled from the player; moves the viewpoint only).
struct FlyCam {
    pos: Vec3,
    yaw: f32,
    pitch: f32,
}

impl FlyCam {
    fn forward(&self) -> Vec3 {
        Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        )
        .normalize()
    }

    fn render_camera(&self, aspect: f32) -> crab_render::Camera {
        crab_render::Camera {
            eye: self.pos,
            target: self.pos + self.forward(),
            up: Vec3::Y,
            aspect,
            fovy_radians: 70f32.to_radians(),
            znear: 0.1,
            zfar: 1000.0,
        }
    }
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
    depth_view: wgpu::TextureView,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_count: u32,
}

impl Graphics {
    fn new(window: Arc<Window>) -> Self {
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

        let (pipeline, bind_group_layout) = build_block_pipeline(&device, config.format);
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::cast_slice(&[CameraUniform {
                view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

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
            depth_view,
            vertex_buffer: None,
            vertex_count: 0,
        }
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

    fn upload_mesh(&mut self, vertices: &[Vertex]) {
        self.vertex_count = vertices.len() as u32;
        self.vertex_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("world vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            },
        ));
    }

    fn render(&mut self, cam: &FlyCam) {
        let uniform = CameraUniform::new(&cam.render_camera(self.aspect()));
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
            if let Some(vb) = &self.vertex_buffer {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.draw(0..self.vertex_count, 0..1);
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
    gfx: Option<Graphics>,
    cam: FlyCam,
    cam_initialized: bool,
    keys: HashSet<KeyCode>,
    last_frame: Instant,
    last_remesh: Instant,
}

impl App {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            gfx: None,
            cam: FlyCam {
                pos: Vec3::new(0.0, 0.0, 0.0),
                yaw: 0.0,
                pitch: -0.3,
            },
            cam_initialized: false,
            keys: HashSet::new(),
            last_frame: Instant::now(),
            last_remesh: Instant::now() - REMESH_INTERVAL,
        }
    }

    fn update_camera(&mut self, dt: f32) {
        let forward = self.cam.forward();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let mut delta = Vec3::ZERO;
        let pressed = |c: KeyCode| self.keys.contains(&c);
        if pressed(KeyCode::KeyW) {
            delta += forward;
        }
        if pressed(KeyCode::KeyS) {
            delta -= forward;
        }
        if pressed(KeyCode::KeyD) {
            delta += right;
        }
        if pressed(KeyCode::KeyA) {
            delta -= right;
        }
        if pressed(KeyCode::KeyE) || pressed(KeyCode::Space) {
            delta += Vec3::Y;
        }
        if pressed(KeyCode::KeyQ) || pressed(KeyCode::ShiftLeft) {
            delta -= Vec3::Y;
        }
        if delta != Vec3::ZERO {
            self.cam.pos += delta.normalize() * MOVE_SPEED * dt;
        }
        if pressed(KeyCode::ArrowLeft) {
            self.cam.yaw -= LOOK_SPEED * dt;
        }
        if pressed(KeyCode::ArrowRight) {
            self.cam.yaw += LOOK_SPEED * dt;
        }
        if pressed(KeyCode::ArrowUp) {
            self.cam.pitch = (self.cam.pitch + LOOK_SPEED * dt).clamp(-1.5, 1.5);
        }
        if pressed(KeyCode::ArrowDown) {
            self.cam.pitch = (self.cam.pitch - LOOK_SPEED * dt).clamp(-1.5, 1.5);
        }
    }

    fn maybe_remesh(&mut self) {
        let Some(gfx) = self.gfx.as_mut() else {
            return;
        };
        if self.last_remesh.elapsed() < REMESH_INTERVAL {
            return;
        }
        self.last_remesh = Instant::now();

        let world = self.shared.world.lock().unwrap();
        let cx = self.cam.pos.x.floor() as i32;
        let cy = self.cam.pos.y.floor() as i32;
        let cz = self.cam.pos.z.floor() as i32;
        let min_y = world.min_y.max(cy - MESH_RADIUS);
        let max_y = (world.min_y + world.height - 1).min(cy + MESH_RADIUS);
        let mesh = mesh_region(
            &world,
            [cx - MESH_RADIUS, min_y, cz - MESH_RADIUS],
            [cx + MESH_RADIUS, max_y, cz + MESH_RADIUS],
        );
        drop(world);
        gfx.upload_mesh(&mesh.vertices);
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
        self.gfx = Some(Graphics::new(window));
        self.last_frame = Instant::now();
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
            WindowEvent::RedrawRequested => {
                // Initialise the camera at the player's spawn the first time.
                if !self.cam_initialized {
                    let p = *self.shared.player.lock().unwrap();
                    if p.spawned {
                        self.cam.pos = Vec3::new(p.x as f32, p.y as f32 + 1.6, p.z as f32);
                        self.cam_initialized = true;
                    }
                }

                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;
                self.update_camera(dt);
                self.maybe_remesh();

                let cam = FlyCam {
                    pos: self.cam.pos,
                    yaw: self.cam.yaw,
                    pitch: self.cam.pitch,
                };
                if let Some(gfx) = self.gfx.as_mut() {
                    gfx.render(&cam);
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
pub fn run(shared: Arc<Shared>) -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    let mut app = App::new(shared);
    event_loop.run_app(&mut app)?;
    Ok(())
}
