//! Turning a [`World`] region into a textured mesh.
//!
//! Face culling: a block face is emitted only when its neighbour is air (or
//! absent). Each face is textured from the [`crab_assets::Atlas`] (per-block,
//! per-face atlas UVs + a tint multiplier); blocks the atlas doesn't have a cube
//! model for fall back to a flat tinted tile.

use crab_assets::Atlas;
use crab_world::World;

/// A vertex: position, face normal (lighting), atlas UV, and tint multiplier.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tint: [f32; 3],
}

impl Vertex {
    /// wgpu vertex layout matching this struct.
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
            0 => Float32x3, // position
            1 => Float32x3, // normal
            2 => Float32x2, // uv
            3 => Float32x3, // tint
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// A CPU-side mesh (non-indexed triangle list).
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
}

impl Mesh {
    pub fn vertex_count(&self) -> u32 {
        self.vertices.len() as u32
    }

    pub fn triangle_count(&self) -> usize {
        self.vertices.len() / 3
    }
}

struct Face {
    /// Neighbour direction this face points toward.
    dir: [i32; 3],
    normal: [f32; 3],
    /// Four corners (unit cube space), wound as a quad.
    corners: [[f32; 3]; 4],
    /// Per-corner UV in tile space (0..1, v=0 at the texture's top), chosen so
    /// side textures stand upright.
    uvs: [[f32; 2]; 4],
}

#[rustfmt::skip]
const FACES: [Face; 6] = [
    // +X (east)
    Face { dir: [1, 0, 0], normal: [1.0, 0.0, 0.0],
        corners: [[1.0,0.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[1.0,0.0,1.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
    // -X (west)
    Face { dir: [-1, 0, 0], normal: [-1.0, 0.0, 0.0],
        corners: [[0.0,0.0,1.0],[0.0,1.0,1.0],[0.0,1.0,0.0],[0.0,0.0,0.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
    // +Y (top)
    Face { dir: [0, 1, 0], normal: [0.0, 1.0, 0.0],
        corners: [[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[0.0,1.0,1.0]],
        uvs: [[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]] },
    // -Y (bottom)
    Face { dir: [0, -1, 0], normal: [0.0, -1.0, 0.0],
        corners: [[0.0,0.0,1.0],[1.0,0.0,1.0],[1.0,0.0,0.0],[0.0,0.0,0.0]],
        uvs: [[0.0,1.0],[1.0,1.0],[1.0,0.0],[0.0,0.0]] },
    // +Z (south)
    Face { dir: [0, 0, 1], normal: [0.0, 0.0, 1.0],
        corners: [[1.0,0.0,1.0],[1.0,1.0,1.0],[0.0,1.0,1.0],[0.0,0.0,1.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
    // -Z (north)
    Face { dir: [0, 0, -1], normal: [0.0, 0.0, -1.0],
        corners: [[0.0,0.0,0.0],[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,0.0,0.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
];

/// Whether `state` is a full opaque cube, so it hides neighbouring faces.
fn occludes(atlas: &Atlas, state: u32) -> bool {
    !crab_registry::is_air(state)
        && crab_registry::block_name(state).is_some_and(|n| atlas.is_cube(n))
}

/// Builds a textured mesh for the inclusive world-coordinate box `[min, max]`.
pub fn mesh_region(world: &World, atlas: &Atlas, min: [i32; 3], max: [i32; 3]) -> Mesh {
    let mut vertices = Vec::new();
    for x in min[0]..=max[0] {
        for y in min[1]..=max[1] {
            for z in min[2]..=max[2] {
                let Some(state) = world.block_state(x, y, z) else {
                    continue;
                };
                if crab_registry::is_air(state) {
                    continue;
                }
                let name = crab_registry::block_name(state).unwrap_or("");
                let base = [x as f32, y as f32, z as f32];

                // Non-cube blocks (slabs, stairs, fences, plants, …) emit their
                // model's element geometry.
                if let Some(elements) = atlas.block_elements(name) {
                    emit_elements(&mut vertices, world, atlas, base, [x, y, z], elements);
                    continue;
                }

                // Full cube (or flat fallback): one quad per non-occluded face.
                let model = atlas.model(name);
                for (fi, face) in FACES.iter().enumerate() {
                    let neighbor =
                        world.block_state(x + face.dir[0], y + face.dir[1], z + face.dir[2]);
                    if neighbor.is_some_and(|s| occludes(atlas, s)) {
                        continue;
                    }
                    let tex = model.faces[fi];
                    let [u0, v0, u1, v1] = tex.uv;
                    // two triangles: (0,1,2) and (0,2,3)
                    for &ci in &[0usize, 1, 2, 0, 2, 3] {
                        let c = face.corners[ci];
                        let [cu, cv] = face.uvs[ci];
                        vertices.push(Vertex {
                            position: [base[0] + c[0], base[1] + c[1], base[2] + c[2]],
                            normal: face.normal,
                            uv: [u0 + cu * (u1 - u0), v0 + cv * (v1 - v0)],
                            tint: tex.tint,
                        });
                    }
                }
            }
        }
    }
    Mesh { vertices }
}

/// Rotates a normalized (0..1) point about an [`crab_assets::ElementRotation`].
fn rotate_point(p: [f32; 3], rot: &crab_assets::ElementRotation) -> [f32; 3] {
    let o = [
        rot.origin[0] / 16.0,
        rot.origin[1] / 16.0,
        rot.origin[2] / 16.0,
    ];
    let a = rot.angle.to_radians();
    let (s, c) = (a.sin(), a.cos());
    let mut d = [p[0] - o[0], p[1] - o[1], p[2] - o[2]];
    let rescale = if rot.rescale { 1.0 / c.abs() } else { 1.0 };
    match rot.axis {
        0 => {
            let (yy, zz) = (d[1], d[2]);
            d[1] = (yy * c - zz * s) * rescale;
            d[2] = (yy * s + zz * c) * rescale;
        }
        2 => {
            let (xx, yy) = (d[0], d[1]);
            d[0] = (xx * c - yy * s) * rescale;
            d[1] = (xx * s + yy * c) * rescale;
        }
        _ => {
            let (xx, zz) = (d[0], d[2]);
            d[0] = (xx * c + zz * s) * rescale;
            d[2] = (-xx * s + zz * c) * rescale;
        }
    }
    [o[0] + d[0], o[1] + d[1], o[2] + d[2]]
}

/// Emits one block's element geometry (used for non-full-cube models).
fn emit_elements(
    verts: &mut Vec<Vertex>,
    world: &World,
    atlas: &Atlas,
    base: [f32; 3],
    cell: [i32; 3],
    elements: &[crab_assets::ElementData],
) {
    for el in elements {
        let from_n = [el.from[0] / 16.0, el.from[1] / 16.0, el.from[2] / 16.0];
        let to_n = [el.to[0] / 16.0, el.to[1] / 16.0, el.to[2] / 16.0];
        for (fi, face) in FACES.iter().enumerate() {
            let Some(ef) = el.faces[fi] else {
                continue;
            };
            // Cull this face only when a cullface neighbour is a full cube.
            if let Some(cd) = ef.cull {
                let d = FACES[cd as usize].dir;
                let n = world.block_state(cell[0] + d[0], cell[1] + d[1], cell[2] + d[2]);
                if n.is_some_and(|s| occludes(atlas, s)) {
                    continue;
                }
            }
            let [su0, sv0, su1, sv1] = ef.uv;
            for &ci in &[0usize, 1, 2, 0, 2, 3] {
                let uc = face.corners[ci];
                let mut p = [
                    if uc[0] == 0.0 { from_n[0] } else { to_n[0] },
                    if uc[1] == 0.0 { from_n[1] } else { to_n[1] },
                    if uc[2] == 0.0 { from_n[2] } else { to_n[2] },
                ];
                if let Some(rot) = &el.rotation {
                    p = rotate_point(p, rot);
                }
                let [cu, cv] = face.uvs[ci];
                verts.push(Vertex {
                    position: [base[0] + p[0], base[1] + p[1], base[2] + p[2]],
                    normal: face.normal,
                    uv: [su0 + cu * (su1 - su0), sv0 + cv * (sv1 - sv0)],
                    tint: ef.tint,
                });
            }
        }
    }
}

/// Builds a 36-vertex box spanning `[min, max]`, textured with `uv` (an atlas
/// rect, e.g. a white tile) tinted by `tint`. Used for entity boxes.
pub fn box_mesh(min: [f32; 3], max: [f32; 3], uv: [f32; 4], tint: [f32; 3]) -> Vec<Vertex> {
    let [u0, v0, u1, v1] = uv;
    let mut verts = Vec::with_capacity(36);
    for face in &FACES {
        for &ci in &[0usize, 1, 2, 0, 2, 3] {
            let c = face.corners[ci];
            let [cu, cv] = face.uvs[ci];
            verts.push(Vertex {
                position: [
                    min[0] + c[0] * (max[0] - min[0]),
                    min[1] + c[1] * (max[1] - min[1]),
                    min[2] + c[2] * (max[2] - min[2]),
                ],
                normal: face.normal,
                uv: [u0 + cu * (u1 - u0), v0 + cv * (v1 - v0)],
                tint,
            });
        }
    }
    verts
}

fn push_quad(verts: &mut Vec<Vertex>, c: [[f32; 3]; 4], normal: [f32; 3], uv: [[f32; 2]; 4]) {
    for &i in &[0usize, 1, 2, 0, 2, 3] {
        verts.push(Vertex {
            position: c[i],
            normal,
            uv: uv[i],
            tint: [1.0, 1.0, 1.0],
        });
    }
}

/// Rotates `p` about `pivot` by Euler `deg` (X, then Y, then Z).
fn rotate_euler(p: [f32; 3], pivot: [f32; 3], deg: [f32; 3]) -> [f32; 3] {
    let mut d = [p[0] - pivot[0], p[1] - pivot[1], p[2] - pivot[2]];
    if deg[0] != 0.0 {
        let a = deg[0].to_radians();
        let (s, c) = (a.sin(), a.cos());
        let (y, z) = (d[1], d[2]);
        d[1] = y * c - z * s;
        d[2] = y * s + z * c;
    }
    if deg[1] != 0.0 {
        let a = deg[1].to_radians();
        let (s, c) = (a.sin(), a.cos());
        let (x, z) = (d[0], d[2]);
        d[0] = x * c + z * s;
        d[2] = -x * s + z * c;
    }
    if deg[2] != 0.0 {
        let a = deg[2].to_radians();
        let (s, c) = (a.sin(), a.cos());
        let (x, y) = (d[0], d[1]);
        d[0] = x * c - y * s;
        d[1] = x * s + y * c;
    }
    [pivot[0] + d[0], pivot[1] + d[1], pivot[2] + d[2]]
}

/// Procedural walk swing (degrees about X) for a limb bone, or 0 for others.
/// `swing` is the walk-cycle phase in radians; `amount` scales with speed.
fn limb_swing_deg(name: &str, swing: f32, amount: f32) -> f32 {
    let n = name.to_ascii_lowercase();
    let is_leg = n.contains("leg");
    let is_arm = n.contains("arm");
    if !is_leg && !is_arm {
        return 0.0;
    }
    // Side/phase: explicit left/right, else trailing-digit (quadruped diagonals).
    let mut sign = if n.contains("left") {
        -1.0
    } else if n.contains("right") {
        1.0
    } else if let Some(d) = n.bytes().rev().find(u8::is_ascii_digit) {
        if matches!(d, b'0' | b'3') {
            1.0
        } else {
            -1.0
        }
    } else {
        1.0
    };
    if is_arm {
        sign = -sign; // arms swing opposite the legs
    }
    (swing.sin() * amount * sign).to_degrees() * 1.2
}

/// Builds a 3D mesh for a Bedrock entity geometry, feet at `offset`. Each bone's
/// rest rotation (e.g. the cow body's 90° tilt) is applied about its pivot, plus
/// a procedural limb swing (`limb_swing` radians; 0 = rest pose).
///
/// The entity's texture occupies the rectangle at `uv_origin` (pixels) within a
/// texture of size `uv_size` (pixels) — pass `([0,0], [tex_w, tex_h])` for a
/// dedicated texture, or the atlas placement for a shared entity atlas.
#[allow(clippy::too_many_arguments)]
pub fn entity_mesh(
    geo: &crab_assets::EntityGeometry,
    offset: [f32; 3],
    uv_origin: [f32; 2],
    uv_size: [f32; 2],
    limb_swing: f32,
    limb_amount: f32,
    scale: f32,
) -> Vec<Vertex> {
    let (sw, sh) = (uv_size[0].max(1.0), uv_size[1].max(1.0));
    let (ox, oy) = (uv_origin[0], uv_origin[1]);
    let uvr = |x0: f32, y0: f32, x1: f32, y1: f32| {
        [
            [(ox + x0) / sw, (oy + y0) / sh],
            [(ox + x1) / sw, (oy + y0) / sh],
            [(ox + x1) / sw, (oy + y1) / sh],
            [(ox + x0) / sw, (oy + y1) / sh],
        ]
    };
    let mut verts = Vec::new();
    for bone in &geo.bones {
        let swing = limb_swing_deg(&bone.name, limb_swing, limb_amount);
        // Minecraft's bone rotation is the opposite sense to our math rotation,
        // so negate the rest rotation (e.g. the cow body's 90° tilt connects to
        // the head/legs). The swing keeps our sign (it's symmetric anyway).
        let euler = [
            -bone.rotation[0] + swing,
            -bone.rotation[1],
            -bone.rotation[2],
        ];
        // Bedrock model space -> world: rotate about the bone pivot, then /16,
        // X negated (Bedrock/Java mirror), + feet offset.
        let place = |px: f32, py: f32, pz: f32| {
            let r = rotate_euler([px, py, pz], bone.pivot, euler);
            [
                -r[0] / 16.0 * scale + offset[0],
                r[1] / 16.0 * scale + offset[1],
                r[2] / 16.0 * scale + offset[2],
            ]
        };
        let nrm = |n: [f32; 3]| rotate_euler(n, [0.0, 0.0, 0.0], euler);

        for cube in &bone.cubes {
            let o = cube.origin;
            let s = cube.size;
            let (x0, y0, z0) = (o[0], o[1], o[2]);
            let (x1, y1, z1) = (o[0] + s[0], o[1] + s[1], o[2] + s[2]);
            let (u, v) = (cube.uv[0], cube.uv[1]);
            let (sx, sy, sz) = (s[0], s[1], s[2]);

            // +Z (front/south)
            push_quad(
                &mut verts,
                [
                    place(x0, y1, z1),
                    place(x1, y1, z1),
                    place(x1, y0, z1),
                    place(x0, y0, z1),
                ],
                nrm([0.0, 0.0, 1.0]),
                uvr(u + sz, v + sz, u + sz + sx, v + sz + sy),
            );
            // -Z (back/north)
            push_quad(
                &mut verts,
                [
                    place(x1, y1, z0),
                    place(x0, y1, z0),
                    place(x0, y0, z0),
                    place(x1, y0, z0),
                ],
                nrm([0.0, 0.0, -1.0]),
                uvr(
                    u + 2.0 * sz + sx,
                    v + sz,
                    u + 2.0 * sz + 2.0 * sx,
                    v + sz + sy,
                ),
            );
            // +X (east) — world +X is model -X (negated), so it uses model x0 plane
            push_quad(
                &mut verts,
                [
                    place(x0, y1, z0),
                    place(x0, y1, z1),
                    place(x0, y0, z1),
                    place(x0, y0, z0),
                ],
                nrm([1.0, 0.0, 0.0]),
                uvr(u, v + sz, u + sz, v + sz + sy),
            );
            // -X (west)
            push_quad(
                &mut verts,
                [
                    place(x1, y1, z1),
                    place(x1, y1, z0),
                    place(x1, y0, z0),
                    place(x1, y0, z1),
                ],
                nrm([-1.0, 0.0, 0.0]),
                uvr(u + sz + sx, v + sz, u + 2.0 * sz + sx, v + sz + sy),
            );
            // +Y (top)
            push_quad(
                &mut verts,
                [
                    place(x1, y1, z0),
                    place(x0, y1, z0),
                    place(x0, y1, z1),
                    place(x1, y1, z1),
                ],
                nrm([0.0, 1.0, 0.0]),
                uvr(u + sz, v, u + sz + sx, v + sz),
            );
            // -Y (bottom)
            push_quad(
                &mut verts,
                [
                    place(x1, y0, z1),
                    place(x0, y0, z1),
                    place(x0, y0, z0),
                    place(x1, y0, z0),
                ],
                nrm([0.0, -1.0, 0.0]),
                uvr(u + sz + sx, v, u + sz + 2.0 * sx, v + sz),
            );
        }
    }
    verts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_world::{BlockStates, Chunk, Section, World};

    const STONE: u32 = 1;

    fn world_with_chunk0() -> World {
        let mut world = World::overworld();
        let sections = (0..24)
            .map(|_| Section {
                block_count: 0,
                blocks: BlockStates::Uniform(0),
            })
            .collect();
        world.load_chunk(Chunk {
            x: 0,
            z: 0,
            sections,
        });
        world
    }

    #[test]
    fn lone_block_emits_six_faces() {
        let mut world = world_with_chunk0();
        world.set_block_state(5, -60, 5, STONE);
        let atlas = Atlas::debug_uniform();
        let mesh = mesh_region(&world, &atlas, [5, -60, 5], [5, -60, 5]);
        assert_eq!(mesh.vertices.len(), 36); // 6 faces * 2 tris * 3 verts
    }

    #[test]
    fn limb_swing_alternates_and_skips_body() {
        use std::f32::consts::FRAC_PI_2;
        // Non-limb bones never swing.
        assert_eq!(limb_swing_deg("body", FRAC_PI_2, 1.0), 0.0);
        assert_eq!(limb_swing_deg("head", FRAC_PI_2, 1.0), 0.0);
        // Opposite legs swing in opposite directions.
        let l0 = limb_swing_deg("leg0", FRAC_PI_2, 1.0);
        let l1 = limb_swing_deg("leg1", FRAC_PI_2, 1.0);
        assert!(l0 > 0.0 && l1 < 0.0 && (l0 + l1).abs() < 1e-3);
        // Arm swings opposite the leg on the same side.
        let leg = limb_swing_deg("right_leg", FRAC_PI_2, 1.0);
        let arm = limb_swing_deg("right_arm", FRAC_PI_2, 1.0);
        assert!(leg * arm < 0.0);
        // No movement -> no swing.
        assert_eq!(limb_swing_deg("leg0", FRAC_PI_2, 0.0), 0.0);
    }

    #[test]
    fn unloaded_region_is_empty() {
        let world = World::overworld();
        let atlas = Atlas::debug_uniform();
        let mesh = mesh_region(&world, &atlas, [0, 0, 0], [4, 4, 4]);
        assert!(mesh.vertices.is_empty());
    }

    #[test]
    fn fully_enclosed_block_is_culled() {
        let mut world = world_with_chunk0();
        world.set_block_state(5, -60, 5, STONE);
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            world.set_block_state(5 + dx, -60 + dy, 5 + dz, STONE);
        }
        let atlas = Atlas::debug_uniform();
        let mesh = mesh_region(&world, &atlas, [5, -60, 5], [5, -60, 5]);
        assert!(mesh.vertices.is_empty());
    }
}
