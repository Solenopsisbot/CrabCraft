//! A simple perspective camera producing a wgpu-ready view-projection matrix.

use glam::{Mat4, Vec3};

/// A look-at perspective camera.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub aspect: f32,
    pub fovy_radians: f32,
    pub znear: f32,
    pub zfar: f32,
}

impl Camera {
    /// Combined projection * view matrix.
    ///
    /// Uses right-handed look-at and a `[0, 1]` depth-range projection (the
    /// convention wgpu/Metal/DX expect), so no extra clip-space correction is
    /// needed.
    pub fn view_proj(&self) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye, self.target, self.up);
        let proj = Mat4::perspective_rh(self.fovy_radians, self.aspect, self.znear, self.zfar);
        proj * view
    }
}
