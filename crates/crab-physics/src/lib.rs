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
