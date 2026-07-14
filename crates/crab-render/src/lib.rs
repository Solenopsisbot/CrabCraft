//! # crab-render
//!
//! A `wgpu` renderer for the Crabcraft world. It meshes revisioned
//! [`crab_world::WorldSnapshot`] regions with session-scoped registry and asset
//! data, then rasterizes them.
//!
//! The pipeline is window-agnostic: [`renderer::render_to_rgba`] /
//! [`render_to_png`] render headlessly to an image (great for tests and CI),
//! and the same shader/vertex format drives the live windowed mode.
//!
//! Model and texture loading stays in `crab-assets`; this crate consumes the
//! resulting immutable atlas/model generations.

pub mod camera;
pub mod hud;
pub mod mesh;
pub mod renderer;

pub use camera::Camera;
pub use hud::{
    build_hud_pipelines, container_geometry, container_rect, container_slot_rect,
    enchantment_option_rect, furnace_geometry, furnace_slot_rect, hotbar_slot_rect, hud_geometry,
    inventory_geometry, inventory_rect, inventory_slot_rect, menu_button_rect, menu_geometry,
    push_text, render_hud_to_png, simple_container_geometry, simple_container_rect,
    simple_container_slot_rect, status_effect_geometry, HudFrame, HudPipelines,
};
pub use mesh::{
    block_item_mesh, block_state_item_mesh, box_mesh, entity_armour_mesh, entity_mesh,
    entity_mesh_with_pose, item_model_mesh, mesh_region, mesh_region_with_registry, Mesh, Vertex,
};
pub use renderer::{
    build_block_pipeline, render_to_png, render_to_rgba, upload_atlas, upload_texture,
    CameraUniform, ATLAS_FORMAT, DEPTH_FORMAT,
};
