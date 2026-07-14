//! A simple perspective camera producing a wgpu-ready view-projection matrix.

use glam::{camera::rh, Mat4, Vec3};

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
        let view = rh::view::look_at_mat4(self.eye, self.target, self.up);
        let proj =
            rh::proj::directx::perspective(self.fovy_radians, self.aspect, self.znear, self.zfar);
        proj * view
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_projection_uses_webgpu_depth_range() {
        let camera = Camera {
            eye: Vec3::ZERO,
            target: -Vec3::Z,
            up: Vec3::Y,
            aspect: 1.0,
            fovy_radians: 90.0_f32.to_radians(),
            znear: 1.0,
            zfar: 10.0,
        };

        let view_proj = camera.view_proj();
        let near = view_proj.project_point3(Vec3::new(0.0, 0.0, -camera.znear));
        let far = view_proj.project_point3(Vec3::new(0.0, 0.0, -camera.zfar));

        assert!(near.abs_diff_eq(Vec3::ZERO, 1.0e-6));
        assert!(far.abs_diff_eq(Vec3::Z, 1.0e-6));
    }
}
