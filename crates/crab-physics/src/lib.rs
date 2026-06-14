//! # crab-physics
//!
//! Minimal player physics: an axis-aligned bounding box swept against the solid
//! voxels of a [`crab_world::World`], plus gravity. Collision is resolved one
//! axis at a time (the classic Minecraft approach), which is simple and stable.
//!
//! Solidity is "not air" for now (via [`crab_registry`]); precise per-block
//! collision shapes (slabs, stairs, fences, fluids) are a later refinement.

use crab_world::World;
use glam::DVec3;

/// Half the player's width (full width 0.6).
pub const PLAYER_HALF_WIDTH: f64 = 0.3;
/// Player height.
pub const PLAYER_HEIGHT: f64 = 1.8;
/// Downward acceleration (blocks/s²).
pub const GRAVITY: f64 = 30.0;
/// Maximum fall speed (blocks/s).
pub const TERMINAL_VELOCITY: f64 = -78.4;

const EPS: f64 = 1e-7;

/// An axis-aligned bounding box.
#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    /// The player's box given its feet position.
    pub fn player(feet: DVec3) -> Self {
        Self {
            min: DVec3::new(
                feet.x - PLAYER_HALF_WIDTH,
                feet.y,
                feet.z - PLAYER_HALF_WIDTH,
            ),
            max: DVec3::new(
                feet.x + PLAYER_HALF_WIDTH,
                feet.y + PLAYER_HEIGHT,
                feet.z + PLAYER_HALF_WIDTH,
            ),
        }
    }
}

fn is_solid(world: &World, x: i32, y: i32, z: i32) -> bool {
    world
        .block_state(x, y, z)
        .is_some_and(|s| !crab_registry::is_air(s))
}

/// Outcome of a physics step.
#[derive(Clone, Copy, Debug)]
pub struct StepResult {
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub on_ground: bool,
}

/// Advances the player by `dt` seconds: applies gravity, then resolves motion
/// against solid blocks one axis at a time. `feet`/`vel` are `[x, y, z]`.
pub fn step_player(world: &World, feet: [f64; 3], vel: [f64; 3], dt: f64) -> StepResult {
    let mut pos = DVec3::from_array(feet);
    let mut v = DVec3::from_array(vel);

    v.y = (v.y - GRAVITY * dt).max(TERMINAL_VELOCITY);

    // Y
    let desired = v.y * dt;
    let dy = collide_y(world, &Aabb::player(pos), desired);
    let on_ground = desired < 0.0 && dy > desired + EPS;
    if (dy - desired).abs() > EPS {
        v.y = 0.0;
    }
    pos.y += dy;

    // X
    let want = v.x * dt;
    let dx = collide_x(world, &Aabb::player(pos), want);
    if (dx - want).abs() > EPS {
        v.x = 0.0;
    }
    pos.x += dx;

    // Z
    let want = v.z * dt;
    let dz = collide_z(world, &Aabb::player(pos), want);
    if (dz - want).abs() > EPS {
        v.z = 0.0;
    }
    pos.z += dz;

    StepResult {
        position: pos.to_array(),
        velocity: v.to_array(),
        on_ground,
    }
}

/// Clamp vertical motion `dy` so the box doesn't pass through solid blocks.
pub fn collide_y(world: &World, aabb: &Aabb, dy: f64) -> f64 {
    if dy == 0.0 {
        return 0.0;
    }
    let x0 = aabb.min.x.floor() as i32;
    let x1 = (aabb.max.x - EPS).floor() as i32;
    let z0 = aabb.min.z.floor() as i32;
    let z1 = (aabb.max.z - EPS).floor() as i32;
    let mut dy = dy;
    if dy < 0.0 {
        let start = (aabb.min.y - EPS).floor() as i32;
        let end = (aabb.min.y + dy).floor() as i32;
        for by in end..=start {
            for bx in x0..=x1 {
                for bz in z0..=z1 {
                    if is_solid(world, bx, by, bz) && aabb.min.y + dy < (by + 1) as f64 {
                        dy = ((by + 1) as f64 - aabb.min.y).min(0.0);
                    }
                }
            }
        }
    } else {
        let start = (aabb.max.y + EPS).floor() as i32;
        let end = (aabb.max.y + dy).floor() as i32;
        for by in start..=end {
            for bx in x0..=x1 {
                for bz in z0..=z1 {
                    if is_solid(world, bx, by, bz) && aabb.max.y + dy > by as f64 {
                        dy = (by as f64 - aabb.max.y).max(0.0);
                    }
                }
            }
        }
    }
    dy
}

/// Clamp X motion against solid blocks.
pub fn collide_x(world: &World, aabb: &Aabb, dx: f64) -> f64 {
    if dx == 0.0 {
        return 0.0;
    }
    let y0 = aabb.min.y.floor() as i32;
    let y1 = (aabb.max.y - EPS).floor() as i32;
    let z0 = aabb.min.z.floor() as i32;
    let z1 = (aabb.max.z - EPS).floor() as i32;
    let mut dx = dx;
    if dx < 0.0 {
        let start = (aabb.min.x - EPS).floor() as i32;
        let end = (aabb.min.x + dx).floor() as i32;
        for bx in end..=start {
            for by in y0..=y1 {
                for bz in z0..=z1 {
                    if is_solid(world, bx, by, bz) && aabb.min.x + dx < (bx + 1) as f64 {
                        dx = ((bx + 1) as f64 - aabb.min.x).min(0.0);
                    }
                }
            }
        }
    } else {
        let start = (aabb.max.x + EPS).floor() as i32;
        let end = (aabb.max.x + dx).floor() as i32;
        for bx in start..=end {
            for by in y0..=y1 {
                for bz in z0..=z1 {
                    if is_solid(world, bx, by, bz) && aabb.max.x + dx > bx as f64 {
                        dx = (bx as f64 - aabb.max.x).max(0.0);
                    }
                }
            }
        }
    }
    dx
}

/// Clamp Z motion against solid blocks.
pub fn collide_z(world: &World, aabb: &Aabb, dz: f64) -> f64 {
    if dz == 0.0 {
        return 0.0;
    }
    let x0 = aabb.min.x.floor() as i32;
    let x1 = (aabb.max.x - EPS).floor() as i32;
    let y0 = aabb.min.y.floor() as i32;
    let y1 = (aabb.max.y - EPS).floor() as i32;
    let mut dz = dz;
    if dz < 0.0 {
        let start = (aabb.min.z - EPS).floor() as i32;
        let end = (aabb.min.z + dz).floor() as i32;
        for bz in end..=start {
            for bx in x0..=x1 {
                for by in y0..=y1 {
                    if is_solid(world, bx, by, bz) && aabb.min.z + dz < (bz + 1) as f64 {
                        dz = ((bz + 1) as f64 - aabb.min.z).min(0.0);
                    }
                }
            }
        }
    } else {
        let start = (aabb.max.z + EPS).floor() as i32;
        let end = (aabb.max.z + dz).floor() as i32;
        for bz in start..=end {
            for bx in x0..=x1 {
                for by in y0..=y1 {
                    if is_solid(world, bx, by, bz) && aabb.max.z + dz > bz as f64 {
                        dz = (bz as f64 - aabb.max.z).max(0.0);
                    }
                }
            }
        }
    }
    dz
}

/// A voxel raycast hit: the solid block struck and the face it was entered
/// through (as a normal, e.g. `[0,1,0]` = top).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RayHit {
    pub block: [i32; 3],
    pub face: [i32; 3],
}

impl RayHit {
    /// The empty cell against the hit face — where a placed block would go.
    pub fn place_position(&self) -> [i32; 3] {
        [
            self.block[0] + self.face[0],
            self.block[1] + self.face[1],
            self.block[2] + self.face[2],
        ]
    }
}

/// Casts a ray from `origin` along `dir` (need not be normalised) up to
/// `max_dist` blocks, returning the first solid block hit (Amanatides–Woo DDA).
pub fn raycast(world: &World, origin: [f64; 3], dir: [f64; 3], max_dist: f64) -> Option<RayHit> {
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len < 1e-9 {
        return None;
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];
    let mut cell = [
        origin[0].floor() as i32,
        origin[1].floor() as i32,
        origin[2].floor() as i32,
    ];
    let step = [sign(d[0]), sign(d[1]), sign(d[2])];
    let t_delta = [inv_abs(d[0]), inv_abs(d[1]), inv_abs(d[2])];
    let mut t_max = [
        boundary_t(origin[0], d[0], cell[0]),
        boundary_t(origin[1], d[1], cell[1]),
        boundary_t(origin[2], d[2], cell[2]),
    ];
    let mut face = [0i32; 3];
    let max_steps = (max_dist as i32) * 3 + 9;
    for _ in 0..max_steps {
        if is_solid(world, cell[0], cell[1], cell[2]) {
            return Some(RayHit { block: cell, face });
        }
        // advance along the axis with the nearest voxel boundary
        let axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };
        if t_max[axis] > max_dist {
            return None;
        }
        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        face = [0, 0, 0];
        face[axis] = -step[axis];
    }
    None
}

fn sign(v: f64) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

fn inv_abs(v: f64) -> f64 {
    if v.abs() < 1e-9 {
        f64::INFINITY
    } else {
        (1.0 / v).abs()
    }
}

fn boundary_t(origin: f64, dir: f64, cell: i32) -> f64 {
    if dir.abs() < 1e-9 {
        return f64::INFINITY;
    }
    let next = if dir > 0.0 {
        f64::from(cell + 1)
    } else {
        f64::from(cell)
    };
    (next - origin) / dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_world::{BlockStates, Chunk, Section, World};

    const STONE: u32 = 1;

    fn world_with_floor(floor_y: i32) -> World {
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
        for x in 0..16 {
            for z in 0..16 {
                world.set_block_state(x, floor_y, z, STONE);
            }
        }
        world
    }

    #[test]
    fn collide_y_lands_on_floor() {
        let world = world_with_floor(-60); // block occupies -60..-59, top = -59
        let aabb = Aabb::player(DVec3::new(8.0, -50.0, 8.0));
        let dy = collide_y(&world, &aabb, -20.0);
        // feet should stop at the floor top (-59): dy = -59 - (-50) = -9
        assert!((dy - (-9.0)).abs() < 1e-6, "dy was {dy}");
    }

    #[test]
    fn collide_y_free_fall_unobstructed() {
        let world = world_with_floor(-60);
        // far above, small step that doesn't reach the floor
        let aabb = Aabb::player(DVec3::new(8.0, 50.0, 8.0));
        assert!((collide_y(&world, &aabb, -1.0) - (-1.0)).abs() < 1e-9);
    }

    #[test]
    fn gravity_settles_on_floor() {
        let world = world_with_floor(-60);
        let mut pos = [8.0, -50.0, 8.0];
        let mut vel = [0.0, 0.0, 0.0];
        let mut grounded = false;
        for _ in 0..400 {
            let r = step_player(&world, pos, vel, 0.05);
            pos = r.position;
            vel = r.velocity;
            grounded = r.on_ground;
        }
        assert!(grounded, "should be on the ground");
        assert!((pos[1] - (-59.0)).abs() < 1e-6, "rested at y={}", pos[1]);
    }

    #[test]
    fn raycast_hits_floor_from_above() {
        let world = world_with_floor(-60); // block at y=-60, top at -59
        let hit = raycast(&world, [8.5, -50.0, 8.5], [0.0, -1.0, 0.0], 20.0).unwrap();
        assert_eq!(hit.block, [8, -60, 8]);
        assert_eq!(hit.face, [0, 1, 0]); // entered through the top
        assert_eq!(hit.place_position(), [8, -59, 8]);
    }

    #[test]
    fn raycast_misses_into_open_sky() {
        let world = world_with_floor(-60);
        assert!(raycast(&world, [8.5, -50.0, 8.5], [0.0, 1.0, 0.0], 20.0).is_none());
    }

    #[test]
    fn raycast_respects_max_distance() {
        let world = world_with_floor(-60);
        // floor is 10 blocks below; a 3-block ray shouldn't reach it
        assert!(raycast(&world, [8.5, -50.0, 8.5], [0.0, -1.0, 0.0], 3.0).is_none());
    }

    #[test]
    fn horizontal_motion_blocked_by_wall() {
        let mut world = world_with_floor(-60);
        // wall at x=10 spanning the player's height at z=8
        for y in -59..-56 {
            world.set_block_state(10, y, 8, STONE);
        }
        // player just west of the wall, moving +x into it
        let aabb = Aabb::player(DVec3::new(9.0, -59.0, 8.0));
        let dx = collide_x(&world, &aabb, 2.0);
        // max.x starts at 9.3; wall face at x=10; allowed = 10 - 9.3 = 0.7
        assert!((dx - 0.7).abs() < 1e-6, "dx was {dx}");
    }
}
