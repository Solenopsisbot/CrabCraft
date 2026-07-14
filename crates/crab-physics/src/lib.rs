//! # crab-physics
//!
//! Minimal player physics: an axis-aligned bounding box swept against the solid
//! voxels of a [`crab_world::World`], plus gravity. Collision is resolved one
//! axis at a time (the classic Minecraft approach), which is simple and stable.
//!
//! Empty collision shapes (fluids, plants, rails, torches, etc.) are ignored.
//! Non-empty shapes are full-cube approximations for now; precise slabs,
//! stairs, fences and other partial shapes are a later refinement.

use crab_registry::RegistrySet;
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
        Self::player_with_height(feet, PLAYER_HEIGHT)
    }

    /// Player-sized box with a caller-selected pose height (1.5 while sneaking).
    pub fn player_with_height(feet: DVec3, height: f64) -> Self {
        Self {
            min: DVec3::new(
                feet.x - PLAYER_HALF_WIDTH,
                feet.y,
                feet.z - PLAYER_HALF_WIDTH,
            ),
            max: DVec3::new(
                feet.x + PLAYER_HALF_WIDTH,
                feet.y + height,
                feet.z + PLAYER_HALF_WIDTH,
            ),
        }
    }
}

fn overlaps(a0: f64, a1: f64, b0: f64, b1: f64) -> bool {
    a1 > b0 + EPS && a0 < b1 - EPS
}

fn is_targetable(registries: RegistrySet, world: &World, x: i32, y: i32, z: i32) -> bool {
    world.block_state(x, y, z).is_some_and(|state| {
        !registries.is_air(state)
            && !matches!(
                registries.block_name(state),
                Some("minecraft:water" | "minecraft:lava")
            )
    })
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
    step_player_with_height(world, feet, vel, dt, PLAYER_HEIGHT)
}

/// Advances a player using the collision height of its current pose.
pub fn step_player_with_height(
    world: &World,
    feet: [f64; 3],
    vel: [f64; 3],
    dt: f64,
    height: f64,
) -> StepResult {
    step_player_with_forces(world, feet, vel, dt, height, GRAVITY, TERMINAL_VELOCITY)
}

/// Advances a posed player with caller-selected gravity and terminal velocity.
/// Fluid movement uses this while preserving the same collision/step solver.
pub fn step_player_with_forces(
    world: &World,
    feet: [f64; 3],
    vel: [f64; 3],
    dt: f64,
    height: f64,
    gravity: f64,
    terminal_velocity: f64,
) -> StepResult {
    step_player_with_forces_in(
        RegistrySet::global(),
        world,
        feet,
        vel,
        dt,
        height,
        gravity,
        terminal_velocity,
    )
}

/// Session-scoped form of [`step_player_with_forces`].
#[allow(clippy::too_many_arguments)]
pub fn step_player_with_forces_in(
    registries: RegistrySet,
    world: &World,
    feet: [f64; 3],
    vel: [f64; 3],
    dt: f64,
    height: f64,
    gravity: f64,
    terminal_velocity: f64,
) -> StepResult {
    let mut pos = DVec3::from_array(feet);
    let mut v = DVec3::from_array(vel);

    v.y = (v.y - gravity * dt).max(terminal_velocity);

    // Y
    let desired = v.y * dt;
    let dy = collide_y_in(
        registries,
        world,
        &Aabb::player_with_height(pos, height),
        desired,
    );
    let on_ground = desired < 0.0 && dy > desired + EPS;
    if (dy - desired).abs() > EPS {
        v.y = 0.0;
    }
    pos.y += dy;

    let (want_x, want_z) = (v.x * dt, v.z * dt);
    let direct_start = pos;
    let dx = collide_x_in(
        registries,
        world,
        &Aabb::player_with_height(pos, height),
        want_x,
    );
    pos.x += dx;
    let dz = collide_z_in(
        registries,
        world,
        &Aabb::player_with_height(pos, height),
        want_z,
    );
    pos.z += dz;

    // Vanilla-style auto-step: if grounded horizontal motion was clipped, try
    // the same move from up to 0.6 blocks higher, then settle back down.
    let direct_dist2 = dx * dx + dz * dz;
    let clipped = (dx - want_x).abs() > EPS || (dz - want_z).abs() > EPS;
    if on_ground && clipped {
        let mut stepped = direct_start;
        let up = collide_y_in(
            registries,
            world,
            &Aabb::player_with_height(stepped, height),
            0.6,
        );
        stepped.y += up;
        if up > EPS {
            let sx = collide_x_in(
                registries,
                world,
                &Aabb::player_with_height(stepped, height),
                want_x,
            );
            stepped.x += sx;
            let sz = collide_z_in(
                registries,
                world,
                &Aabb::player_with_height(stepped, height),
                want_z,
            );
            stepped.z += sz;
            let down = collide_y_in(
                registries,
                world,
                &Aabb::player_with_height(stepped, height),
                -up,
            );
            stepped.y += down;
            if sx * sx + sz * sz > direct_dist2 + EPS {
                pos = stepped;
            }
        }
    }
    if (pos.x - direct_start.x - want_x).abs() > EPS {
        v.x = 0.0;
    }
    if (pos.z - direct_start.z - want_z).abs() > EPS {
        v.z = 0.0;
    }

    StepResult {
        position: pos.to_array(),
        velocity: v.to_array(),
        on_ground,
    }
}

/// Whether a player pose at `feet` can occupy the world without intersecting a
/// collidable block. Used to prevent standing up into a low ceiling.
#[must_use]
pub fn can_occupy_player(world: &World, feet: [f64; 3], height: f64) -> bool {
    can_occupy_player_in(RegistrySet::global(), world, feet, height)
}

/// Session-scoped form of [`can_occupy_player`].
#[must_use]
pub fn can_occupy_player_in(
    registries: RegistrySet,
    world: &World,
    feet: [f64; 3],
    height: f64,
) -> bool {
    let aabb = Aabb::player_with_height(DVec3::from_array(feet), height);
    let x0 = aabb.min.x.floor() as i32;
    let x1 = (aabb.max.x - EPS).floor() as i32;
    let y0 = aabb.min.y.floor() as i32;
    let y1 = (aabb.max.y - EPS).floor() as i32;
    let z0 = aabb.min.z.floor() as i32;
    let z1 = (aabb.max.z - EPS).floor() as i32;
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                let Some(state) = world.block_state(x, y, z) else {
                    continue;
                };
                for part in registries.collision_shape(state).boxes() {
                    if overlaps(
                        aabb.min.x,
                        aabb.max.x,
                        f64::from(x) + part.min[0],
                        f64::from(x) + part.max[0],
                    ) && overlaps(
                        aabb.min.y,
                        aabb.max.y,
                        f64::from(y) + part.min[1],
                        f64::from(y) + part.max[1],
                    ) && overlaps(
                        aabb.min.z,
                        aabb.max.z,
                        f64::from(z) + part.min[2],
                        f64::from(z) + part.max[2],
                    ) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Clamp vertical motion `dy` so the box doesn't pass through solid blocks.
pub fn collide_y(world: &World, aabb: &Aabb, dy: f64) -> f64 {
    collide_y_in(RegistrySet::global(), world, aabb, dy)
}

/// Session-scoped vertical collision query.
pub fn collide_y_in(registries: RegistrySet, world: &World, aabb: &Aabb, dy: f64) -> f64 {
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
                    let Some(state) = world.block_state(bx, by, bz) else {
                        continue;
                    };
                    for part in registries.collision_shape(state).boxes() {
                        let top = f64::from(by) + part.max[1];
                        if overlaps(
                            aabb.min.x,
                            aabb.max.x,
                            f64::from(bx) + part.min[0],
                            f64::from(bx) + part.max[0],
                        ) && overlaps(
                            aabb.min.z,
                            aabb.max.z,
                            f64::from(bz) + part.min[2],
                            f64::from(bz) + part.max[2],
                        ) && aabb.min.y >= top - EPS
                            && aabb.min.y + dy < top
                        {
                            dy = (top - aabb.min.y).min(0.0);
                        }
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
                    let Some(state) = world.block_state(bx, by, bz) else {
                        continue;
                    };
                    for part in registries.collision_shape(state).boxes() {
                        let bottom = f64::from(by) + part.min[1];
                        if overlaps(
                            aabb.min.x,
                            aabb.max.x,
                            f64::from(bx) + part.min[0],
                            f64::from(bx) + part.max[0],
                        ) && overlaps(
                            aabb.min.z,
                            aabb.max.z,
                            f64::from(bz) + part.min[2],
                            f64::from(bz) + part.max[2],
                        ) && aabb.max.y <= bottom + EPS
                            && aabb.max.y + dy > bottom
                        {
                            dy = (bottom - aabb.max.y).max(0.0);
                        }
                    }
                }
            }
        }
    }
    dy
}

/// Clamp X motion against solid blocks.
pub fn collide_x(world: &World, aabb: &Aabb, dx: f64) -> f64 {
    collide_x_in(RegistrySet::global(), world, aabb, dx)
}

/// Session-scoped horizontal X collision query.
pub fn collide_x_in(registries: RegistrySet, world: &World, aabb: &Aabb, dx: f64) -> f64 {
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
                    let Some(state) = world.block_state(bx, by, bz) else {
                        continue;
                    };
                    for part in registries.collision_shape(state).boxes() {
                        let edge = f64::from(bx) + part.max[0];
                        if overlaps(
                            aabb.min.y,
                            aabb.max.y,
                            f64::from(by) + part.min[1],
                            f64::from(by) + part.max[1],
                        ) && overlaps(
                            aabb.min.z,
                            aabb.max.z,
                            f64::from(bz) + part.min[2],
                            f64::from(bz) + part.max[2],
                        ) && aabb.min.x >= edge - EPS
                            && aabb.min.x + dx < edge
                        {
                            dx = (edge - aabb.min.x).min(0.0);
                        }
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
                    let Some(state) = world.block_state(bx, by, bz) else {
                        continue;
                    };
                    for part in registries.collision_shape(state).boxes() {
                        let edge = f64::from(bx) + part.min[0];
                        if overlaps(
                            aabb.min.y,
                            aabb.max.y,
                            f64::from(by) + part.min[1],
                            f64::from(by) + part.max[1],
                        ) && overlaps(
                            aabb.min.z,
                            aabb.max.z,
                            f64::from(bz) + part.min[2],
                            f64::from(bz) + part.max[2],
                        ) && aabb.max.x <= edge + EPS
                            && aabb.max.x + dx > edge
                        {
                            dx = (edge - aabb.max.x).max(0.0);
                        }
                    }
                }
            }
        }
    }
    dx
}

/// Clamp Z motion against solid blocks.
pub fn collide_z(world: &World, aabb: &Aabb, dz: f64) -> f64 {
    collide_z_in(RegistrySet::global(), world, aabb, dz)
}

/// Session-scoped horizontal Z collision query.
pub fn collide_z_in(registries: RegistrySet, world: &World, aabb: &Aabb, dz: f64) -> f64 {
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
                    let Some(state) = world.block_state(bx, by, bz) else {
                        continue;
                    };
                    for part in registries.collision_shape(state).boxes() {
                        let edge = f64::from(bz) + part.max[2];
                        if overlaps(
                            aabb.min.x,
                            aabb.max.x,
                            f64::from(bx) + part.min[0],
                            f64::from(bx) + part.max[0],
                        ) && overlaps(
                            aabb.min.y,
                            aabb.max.y,
                            f64::from(by) + part.min[1],
                            f64::from(by) + part.max[1],
                        ) && aabb.min.z >= edge - EPS
                            && aabb.min.z + dz < edge
                        {
                            dz = (edge - aabb.min.z).min(0.0);
                        }
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
                    let Some(state) = world.block_state(bx, by, bz) else {
                        continue;
                    };
                    for part in registries.collision_shape(state).boxes() {
                        let edge = f64::from(bz) + part.min[2];
                        if overlaps(
                            aabb.min.x,
                            aabb.max.x,
                            f64::from(bx) + part.min[0],
                            f64::from(bx) + part.max[0],
                        ) && overlaps(
                            aabb.min.y,
                            aabb.max.y,
                            f64::from(by) + part.min[1],
                            f64::from(by) + part.max[1],
                        ) && aabb.max.z <= edge + EPS
                            && aabb.max.z + dz > edge
                        {
                            dz = (edge - aabb.max.z).max(0.0);
                        }
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
    raycast_in(RegistrySet::global(), world, origin, dir, max_dist)
}

/// Session-scoped block ray cast.
pub fn raycast_in(
    registries: RegistrySet,
    world: &World,
    origin: [f64; 3],
    dir: [f64; 3],
    max_dist: f64,
) -> Option<RayHit> {
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
        if is_targetable(registries, world, cell[0], cell[1], cell[2]) {
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

/// Ray vs axis-aligned box (slab method). Returns the entry distance along
/// `dir` (in the same units as a normalised `dir`, i.e. blocks) if the ray
/// starting at `origin` enters `[min, max]` within a forward direction, or
/// `None` if it misses. A ray starting inside the box returns `0.0`.
pub fn ray_aabb(origin: [f64; 3], dir: [f64; 3], min: [f64; 3], max: [f64; 3]) -> Option<f64> {
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len < 1e-9 {
        return None;
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];
    let mut tmin = 0.0_f64;
    let mut tmax = f64::INFINITY;
    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            // Parallel to this slab: must already be within its extent.
            if origin[i] < min[i] || origin[i] > max[i] {
                return None;
            }
        } else {
            let inv = 1.0 / d[i];
            let mut t1 = (min[i] - origin[i]) * inv;
            let mut t2 = (max[i] - origin[i]) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            tmin = tmin.max(t1);
            tmax = tmax.min(t2);
            if tmin > tmax {
                return None;
            }
        }
    }
    Some(tmin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_world::{Biomes, BlockStates, Chunk, Section, World};

    const STONE: u32 = 1;

    fn world_with_floor(floor_y: i32) -> World {
        let mut world = World::overworld();
        let sections = (0..24)
            .map(|_| Section {
                block_count: 0,
                blocks: BlockStates::Uniform(0),
                biomes: Biomes::Uniform(0),
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
    fn configurable_fluid_gravity_is_gentler_and_terminal_limited() {
        let world = world_with_floor(-60);
        let fluid = step_player_with_forces(
            &world,
            [8.0, -50.0, 8.0],
            [0.0, 0.0, 0.0],
            0.05,
            PLAYER_HEIGHT,
            4.0,
            -3.0,
        );
        assert!((fluid.velocity[1] - -0.2).abs() < 1e-9);
        let terminal = step_player_with_forces(
            &world,
            [8.0, -50.0, 8.0],
            [0.0, -10.0, 0.0],
            0.05,
            PLAYER_HEIGHT,
            4.0,
            -3.0,
        );
        assert_eq!(terminal.velocity[1], -3.0);
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

    #[test]
    fn grounded_player_steps_onto_bottom_slab_but_not_full_block() {
        let mut world = world_with_floor(-60);
        let slab = crab_registry::block_by_name("oak_slab")
            .unwrap()
            .default_state;
        world.set_block_state(9, -59, 8, slab);
        let stepped = step_player_with_height(
            &world,
            [8.0, -59.0, 8.0],
            [4.317, 0.0, 0.0],
            0.25,
            PLAYER_HEIGHT,
        );
        assert!(stepped.position[0] > 9.0, "x={}", stepped.position[0]);
        assert!((stepped.position[1] - -58.5).abs() < 1e-6);

        world.set_block_state(9, -59, 8, STONE);
        let blocked = step_player_with_height(
            &world,
            [8.0, -59.0, 8.0],
            [4.317, 0.0, 0.0],
            0.25,
            PLAYER_HEIGHT,
        );
        assert!((blocked.position[0] - 8.7).abs() < 1e-6);
        assert!((blocked.position[1] - -59.0).abs() < 1e-6);
    }

    #[test]
    fn player_walks_up_stair_in_two_half_block_steps() {
        let mut world = world_with_floor(-60);
        let stairs = crab_registry::block_by_name("oak_stairs")
            .unwrap()
            .default_state; // bottom, straight, facing north
        world.set_block_state(8, -59, 9, stairs);
        let first = step_player_with_height(
            &world,
            [8.5, -59.0, 10.4],
            [0.0, 0.0, -4.317],
            0.25,
            PLAYER_HEIGHT,
        );
        assert!((first.position[1] - -58.5).abs() < 1e-6);
        let second = step_player_with_height(
            &world,
            first.position,
            [0.0, 0.0, -4.317],
            0.25,
            PLAYER_HEIGHT,
        );
        assert!((second.position[1] - -58.0).abs() < 1e-6);
        assert!(second.position[2] < 9.5);
    }

    #[test]
    fn empty_shape_blocks_do_not_stop_movement() {
        let mut world = world_with_floor(-60);
        let flower = crab_registry::block_by_name("dandelion")
            .unwrap()
            .default_state;
        world.set_block_state(9, -59, 8, flower);
        let aabb = Aabb::player(DVec3::new(8.0, -59.0, 8.0));

        assert_eq!(collide_x(&world, &aabb, 2.0), 2.0);
    }

    #[test]
    fn raycast_targets_plants_but_passes_through_fluids() {
        let mut world = world_with_floor(-60);
        let flower = crab_registry::block_by_name("dandelion")
            .unwrap()
            .default_state;
        world.set_block_state(8, -59, 8, flower);
        let hit = raycast(&world, [8.5, -57.0, 8.5], [0.0, -1.0, 0.0], 5.0).unwrap();
        assert_eq!(hit.block, [8, -59, 8]);

        let water = crab_registry::block_by_name("water").unwrap().default_state;
        world.set_block_state(8, -59, 8, water);
        let hit = raycast(&world, [8.5, -57.0, 8.5], [0.0, -1.0, 0.0], 5.0).unwrap();
        assert_eq!(hit.block, [8, -60, 8]);
    }

    #[test]
    fn ray_aabb_hits_and_misses() {
        // Box from (2,0,-0.5) to (3,2,0.5); ray down +x from origin.
        let min = [2.0, 0.0, -0.5];
        let max = [3.0, 2.0, 0.5];
        let t = ray_aabb([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], min, max).unwrap();
        assert!((t - 2.0).abs() < 1e-6, "entry distance was {t}");
        // Aimed above the box → miss.
        assert!(ray_aabb([0.0, 3.0, 0.0], [1.0, 0.0, 0.0], min, max).is_none());
        // Origin inside the box → entry distance 0.
        assert_eq!(
            ray_aabb([2.5, 1.0, 0.0], [1.0, 0.0, 0.0], min, max),
            Some(0.0)
        );
    }
}
