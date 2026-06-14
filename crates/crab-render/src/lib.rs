//! # crab-render
//!
//! A `wgpu` renderer for the Crabcraft world. It meshes [`crab_world::World`]
//! block data (face culling + flat per-block colours via [`crab_registry`]) and
//! rasterizes it.
//!
//! The pipeline is window-agnostic: [`renderer::render_to_rgba`] /
//! [`render_to_png`] render headlessly to an image (great for tests and CI),
//! and the same shader/vertex format drives the live windowed mode.
//!
//! Textures/block-models are intentionally not here yet — flat colours are
//! enough to see terrain; the asset pipeline is a later milestone.

pub mod camera;
pub mod mesh;
pub mod renderer;

pub use camera::Camera;
pub use mesh::{box_mesh, entity_mesh, mesh_region, Mesh, Vertex};
pub use renderer::{
    build_block_pipeline, render_to_png, render_to_rgba, upload_atlas, upload_texture,
    CameraUniform, ATLAS_FORMAT, DEPTH_FORMAT,
};
