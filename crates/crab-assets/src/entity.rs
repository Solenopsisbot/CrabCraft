//! Bedrock-format entity geometry (`.geo.json`) parsing.
//!
//! Vanilla **Java** entity models are hardcoded in the game's source, but
//! Mojang's `bedrock-samples` ships the same models as JSON geometry, and their
//! box-UV layouts match the Java entity textures for the classic mobs. We parse
//! the cubes (rest pose: bone rotations at rest are ignored) so the renderer can
//! build a 3D model textured with the entity texture from the user's jar.
//!
//! Handles the `1.8.0` form (top-level `geometry.<name>` key) and the `1.12.0`
//! form (`minecraft:geometry` array).
//!
//! These geometry files are Mojang assets (EULA) and are **not** bundled; they
//! are loaded from a user-provided directory at runtime.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

/// A single box of an entity model (Bedrock coords, pixels).
#[derive(Clone, Debug)]
pub struct Cube {
    /// Minimum corner.
    pub origin: [f32; 3],
    pub size: [f32; 3],
    /// Box-UV origin in the texture (pixels).
    pub uv: [f32; 2],
    pub mirror: bool,
}

/// A model bone: its pivot, rest rotation (degrees), and cubes. Bedrock bones
/// nominally form a hierarchy, but the classic mob models we target apply each
/// bone's rest rotation to its own cubes only (e.g. the cow body's 90° tilt),
/// which matches the hardcoded Java models.
#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub pivot: [f32; 3],
    /// Rest rotation in degrees (`rotation` in 1.12.0, `bind_pose_rotation` in
    /// 1.8.0).
    pub rotation: [f32; 3],
    pub cubes: Vec<Cube>,
}

/// An entity model: its texture dimensions and bones.
#[derive(Clone, Debug)]
pub struct EntityGeometry {
    pub texture_width: f32,
    pub texture_height: f32,
    pub bones: Vec<Bone>,
}

impl EntityGeometry {
    /// Total cube count across all bones.
    #[must_use]
    pub fn cube_count(&self) -> usize {
        self.bones.iter().map(|b| b.cubes.len()).sum()
    }
}

/// Parses a `.geo.json` document into a flat cube list (rest pose).
pub fn parse_geometry(json: &str) -> Option<EntityGeometry> {
    let value: Value = serde_json::from_str(json).ok()?;

    let (geo, tw, th) = if let Some(arr) = value.get("minecraft:geometry").and_then(Value::as_array)
    {
        // 1.12.0 form
        let g = arr.first()?;
        let desc = g.get("description");
        let tw = desc
            .and_then(|d| d.get("texture_width"))
            .and_then(Value::as_f64)
            .unwrap_or(64.0) as f32;
        let th = desc
            .and_then(|d| d.get("texture_height"))
            .and_then(Value::as_f64)
            .unwrap_or(64.0) as f32;
        (g.clone(), tw, th)
    } else {
        // 1.8.0 form: a top-level "geometry.<name>" key
        let obj = value.as_object()?;
        let g = obj
            .iter()
            .find(|(k, _)| k.as_str() != "format_version")
            .map(|(_, v)| v)?;
        let tw = g
            .get("texturewidth")
            .and_then(Value::as_f64)
            .unwrap_or(64.0) as f32;
        let th = g
            .get("textureheight")
            .and_then(Value::as_f64)
            .unwrap_or(32.0) as f32;
        (g.clone(), tw, th)
    };

    let bone_values = geo.get("bones")?.as_array()?;
    let mut bones = Vec::new();
    for bone in bone_values {
        let bone_mirror = bone.get("mirror").and_then(Value::as_bool).unwrap_or(false);
        let name = bone
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let pivot = arr3(bone.get("pivot")).unwrap_or([0.0, 0.0, 0.0]);
        // 1.12.0 uses `rotation`; 1.8.0 uses `bind_pose_rotation`.
        let rotation = arr3(bone.get("rotation"))
            .or_else(|| arr3(bone.get("bind_pose_rotation")))
            .unwrap_or([0.0, 0.0, 0.0]);
        let mut cubes = Vec::new();
        if let Some(bone_cubes) = bone.get("cubes").and_then(Value::as_array) {
            for c in bone_cubes {
                let (Some(origin), Some(size)) = (arr3(c.get("origin")), arr3(c.get("size")))
                else {
                    continue;
                };
                let uv = arr2(c.get("uv")).unwrap_or([0.0, 0.0]);
                let mirror = c
                    .get("mirror")
                    .and_then(Value::as_bool)
                    .unwrap_or(bone_mirror);
                cubes.push(Cube {
                    origin,
                    size,
                    uv,
                    mirror,
                });
            }
        }
        bones.push(Bone {
            name,
            pivot,
            rotation,
            cubes,
        });
    }
    Some(EntityGeometry {
        texture_width: tw,
        texture_height: th,
        bones,
    })
}

/// The classic (wide-arm) Java player model, hardcoded since `bedrock-samples`
/// ships no `player.geo.json`. Box UVs match the 64x64 skin layout; bone names
/// (`*_arm`/`*_leg`) drive the walk animation.
#[must_use]
pub fn player_geometry() -> EntityGeometry {
    let bone = |name: &str, pivot: [f32; 3], origin: [f32; 3], size: [f32; 3], uv: [f32; 2]| Bone {
        name: name.to_string(),
        pivot,
        rotation: [0.0, 0.0, 0.0],
        cubes: vec![Cube {
            origin,
            size,
            uv,
            mirror: false,
        }],
    };
    EntityGeometry {
        texture_width: 64.0,
        texture_height: 64.0,
        bones: vec![
            bone(
                "head",
                [0.0, 24.0, 0.0],
                [-4.0, 24.0, -4.0],
                [8.0, 8.0, 8.0],
                [0.0, 0.0],
            ),
            bone(
                "body",
                [0.0, 24.0, 0.0],
                [-4.0, 12.0, -2.0],
                [8.0, 12.0, 4.0],
                [16.0, 16.0],
            ),
            bone(
                "right_arm",
                [-5.0, 22.0, 0.0],
                [-8.0, 12.0, -2.0],
                [4.0, 12.0, 4.0],
                [40.0, 16.0],
            ),
            bone(
                "left_arm",
                [5.0, 22.0, 0.0],
                [4.0, 12.0, -2.0],
                [4.0, 12.0, 4.0],
                [32.0, 48.0],
            ),
            bone(
                "right_leg",
                [-2.0, 12.0, 0.0],
                [-4.0, 0.0, -2.0],
                [4.0, 12.0, 4.0],
                [0.0, 16.0],
            ),
            bone(
                "left_leg",
                [2.0, 12.0, 0.0],
                [0.0, 0.0, -2.0],
                [4.0, 12.0, 4.0],
                [16.0, 48.0],
            ),
        ],
    }
}

/// Built-in Java-style boat hull used because Mojang's Bedrock sample pack
/// does not publish boat geometry. Textures still come from the user's Java
/// client jar. Chest boats add a separate cargo box to the shared hull.
#[must_use]
pub fn boat_geometry(chest: bool) -> EntityGeometry {
    let cube = |name: &str, origin, size, uv| Bone {
        name: name.to_string(),
        pivot: [0.0; 3],
        rotation: [0.0; 3],
        cubes: vec![Cube {
            origin,
            size,
            uv,
            mirror: false,
        }],
    };
    let mut bones = vec![
        cube("bottom", [-14.0, 0.0, -8.0], [28.0, 2.0, 16.0], [0.0, 0.0]),
        cube(
            "left_side",
            [-14.0, 2.0, -8.0],
            [2.0, 6.0, 16.0],
            [0.0, 18.0],
        ),
        cube(
            "right_side",
            [12.0, 2.0, -8.0],
            [2.0, 6.0, 16.0],
            [36.0, 18.0],
        ),
        cube("front", [-12.0, 2.0, -8.0], [24.0, 6.0, 2.0], [0.0, 40.0]),
        cube("back", [-12.0, 2.0, 6.0], [24.0, 6.0, 2.0], [52.0, 40.0]),
        cube(
            "left_paddle",
            [-18.0, 4.0, -1.0],
            [18.0, 1.0, 2.0],
            [0.0, 52.0],
        ),
        cube(
            "right_paddle",
            [0.0, 4.0, -1.0],
            [18.0, 1.0, 2.0],
            [40.0, 52.0],
        ),
    ];
    if chest {
        bones.push(cube(
            "chest",
            [-7.0, 3.0, -5.0],
            [14.0, 10.0, 10.0],
            [80.0, 0.0],
        ));
    }
    EntityGeometry {
        texture_width: 128.0,
        texture_height: 64.0,
        bones,
    }
}

/// Maps a registry entity name to its bedrock geometry file base name and jar
/// texture path (relative to `textures/entity/`), for the mobs whose asset
/// names differ from the entity name — shared models (e.g. `cave_spider` uses
/// the spider model), variant skins (`axolotl/axolotl_lucy`), or subfolders
/// (`magma_cube` -> `slime/magmacube`). Without this they'd render as boxes.
#[must_use]
pub fn entity_alias(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        // Shared models with their own texture.
        "cave_spider" => ("spider", "spider/cave_spider"),
        "magma_cube" => ("magma_cube", "slime/magmacube"),
        "mooshroom" => ("mooshroom", "cow/red_mooshroom"),
        "elder_guardian" => ("guardian", "guardian_elder"),
        "piglin_brute" => ("piglin", "piglin/piglin_brute"),
        "zombified_piglin" => ("piglin", "piglin/zombified_piglin"),
        "zoglin" => ("hoglin", "hoglin/zoglin"),
        "wither" => ("wither_boss", "wither/wither"),
        "giant" => ("zombie", "zombie/zombie"),
        "illusioner" => ("evoker", "illager/illusioner"),
        "wandering_trader" => ("villager_v2", "wandering_trader"),
        "drowned" => ("drowned", "zombie/drowned"),
        "husk" => ("husk", "zombie/husk"),
        "stray" => ("stray", "skeleton/stray"),
        "wither_skeleton" => ("wither_skeleton", "skeleton/wither_skeleton"),
        "evoker" => ("evoker", "illager/evoker"),
        "pillager" => ("pillager", "illager/pillager"),
        "ravager" => ("ravager", "illager/ravager"),
        "vindicator" => ("vindicator", "illager/vindicator"),
        "glow_squid" => ("glow_squid", "squid/glow_squid"),
        "ocelot" => ("ocelot", "cat/ocelot"),
        "leash_knot" => ("leash_knot", "lead_knot"),
        "shulker_bullet" => ("shulker_bullet", "shulker/spark"),
        "breeze_wind_charge" => ("wind_charge", "projectiles/wind_charge"),
        "wind_charge" => ("wind_charge", "projectiles/wind_charge"),
        "evoker_fangs" => ("evocation_fang", "illager/evoker_fangs"),
        "wither_skull" => ("wither_skull", "wither/wither_invulnerable"),
        "bogged" => ("bogged", "skeleton/bogged"),
        // 1.21.5 split these classic textures into climate variants. The
        // loader still tries the old direct paths after these aliases for
        // earlier supported jars.
        "chicken" => ("chicken", "chicken/temperate_chicken"),
        "cow" => ("cow", "cow/temperate_cow"),
        "pig" => ("pig", "pig/temperate_pig"),
        "llama_spit" => ("llama_spit", "llama/spit"),
        // Fish textures share the `fish` directory in Java resource packs.
        "cod" => ("cod", "fish/cod"),
        "pufferfish" => ("pufferfish", "fish/pufferfish"),
        "salmon" => ("salmon", "fish/salmon"),
        "tropical_fish" => ("tropical_fish", "fish/tropical_a"),
        // Projectiles and vehicle variants reuse a common geometry. Some of
        // these models are named differently in the Bedrock sample pack.
        "arrow" => ("arrow", "projectiles/arrow"),
        "spectral_arrow" => ("arrow", "projectiles/spectral_arrow"),
        "dragon_fireball" => ("fireball", "enderdragon/dragon_fireball"),
        "end_crystal" => ("ender_crystal", "end_crystal/end_crystal"),
        "fishing_bobber" => ("fishing_hook", "fishing_hook"),
        "chest_minecart"
        | "command_block_minecart"
        | "furnace_minecart"
        | "hopper_minecart"
        | "spawner_minecart"
        | "tnt_minecart" => ("minecart", "minecart"),
        // Horses (all share the horse model).
        "horse" => ("horse_v2", "horse/horse_brown"),
        "donkey" => ("horse_v2", "horse/donkey"),
        "mule" => ("horse_v2", "horse/mule"),
        "skeleton_horse" => ("horse_v2", "horse/horse_skeleton"),
        "zombie_horse" => ("horse_v2", "horse/horse_brown"),
        // Own model, but a variant/relocated texture.
        "axolotl" => ("axolotl", "axolotl/axolotl_lucy"),
        "cat" => ("cat", "cat/red"),
        "ender_dragon" => ("ender_dragon", "enderdragon/dragon"),
        "frog" => ("frog", "frog/temperate_frog"),
        "llama" => ("llama", "llama/creamy"),
        "trader_llama" => ("llama", "llama/creamy"),
        "parrot" => ("parrot", "parrot/parrot_red_blue"),
        "polar_bear" => ("polar_bear", "bear/polarbear"),
        "rabbit" => ("rabbit", "rabbit/brown"),
        "turtle" => ("turtle", "turtle/big_sea_turtle"),
        "vex" => ("vex", "illager/vex"),
        "armor_stand" => ("armor_stand", "armorstand/wood"),
        "acacia_boat" => ("boat", "boat/acacia"),
        "birch_boat" => ("boat", "boat/birch"),
        "cherry_boat" => ("boat", "boat/cherry"),
        "dark_oak_boat" => ("boat", "boat/dark_oak"),
        "jungle_boat" => ("boat", "boat/jungle"),
        "mangrove_boat" => ("boat", "boat/mangrove"),
        "oak_boat" => ("boat", "boat/oak"),
        "pale_oak_boat" => ("boat", "boat/pale_oak"),
        "spruce_boat" => ("boat", "boat/spruce"),
        "bamboo_raft" => ("boat", "boat/bamboo"),
        "acacia_chest_boat" => ("chest_boat", "chest_boat/acacia"),
        "birch_chest_boat" => ("chest_boat", "chest_boat/birch"),
        "cherry_chest_boat" => ("chest_boat", "chest_boat/cherry"),
        "dark_oak_chest_boat" => ("chest_boat", "chest_boat/dark_oak"),
        "jungle_chest_boat" => ("chest_boat", "chest_boat/jungle"),
        "mangrove_chest_boat" => ("chest_boat", "chest_boat/mangrove"),
        "oak_chest_boat" => ("chest_boat", "chest_boat/oak"),
        "pale_oak_chest_boat" => ("chest_boat", "chest_boat/pale_oak"),
        "spruce_chest_boat" => ("chest_boat", "chest_boat/spruce"),
        "bamboo_chest_raft" => ("chest_boat", "chest_boat/bamboo"),
        _ => return None,
    })
}

/// Loads an entity model from `<models_dir>/<name>.geo.json`.
pub fn load_geometry(models_dir: &Path, name: &str) -> Option<EntityGeometry> {
    // Most Bedrock samples use `.geo.json`; a handful of otherwise compatible
    // projectile models (notably evocation fangs and llama spit) use `.json`.
    for suffix in [".geo.json", ".json"] {
        if let Ok(text) = fs::read_to_string(models_dir.join(format!("{name}{suffix}"))) {
            if let Some(geometry) = parse_geometry(&text) {
                return Some(geometry);
            }
        }
    }
    None
}

/// Loads an RGBA entity texture from the client jar, trying the common
/// `entity/<name>/<name>.png` then `entity/<name>.png` layouts.
pub fn load_entity_texture(jar_path: &Path, name: &str) -> Option<(Vec<u8>, u32, u32)> {
    let file = fs::File::open(jar_path).ok()?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).ok()?;
    let mut candidates = Vec::new();
    // Aliased texture path first (variant skins / subfolders), then the
    // standard `entity/<name>/<name>.png` and `entity/<name>.png` layouts.
    if let Some((_, tex)) = entity_alias(name) {
        candidates.push(format!("assets/minecraft/textures/entity/{tex}.png"));
    }
    candidates.push(format!(
        "assets/minecraft/textures/entity/{name}/{name}.png"
    ));
    candidates.push(format!("assets/minecraft/textures/entity/{name}.png"));
    if name == "player" {
        // Default skin (no per-player skins in offline mode).
        candidates.push("assets/minecraft/textures/entity/player/wide/steve.png".to_string());
    }
    for candidate in candidates {
        if let Ok(mut entry) = archive.by_name(&candidate) {
            let mut bytes = Vec::new();
            if entry.read_to_end(&mut bytes).is_ok() {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    return Some((rgba.into_raw(), w, h));
                }
            }
        }
    }
    None
}

/// One entity model placed in the shared entity atlas.
#[derive(Clone, Debug)]
pub struct EntityModelEntry {
    pub geo: EntityGeometry,
    /// Top-left of this entity's texture within the atlas (pixels).
    pub atlas_x: f32,
    pub atlas_y: f32,
}

/// A stitched atlas of entity textures plus the geometry + placement per type.
pub struct EntityAtlas {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub models: HashMap<i32, EntityModelEntry>,
}

/// Loads geometry (from `models_dir`) + textures (from the jar) for the given
/// `(type_id, name)` list, stitching the textures into one atlas. Types whose
/// geometry or texture is missing are simply skipped (rendered as boxes).
pub fn load_entity_atlas(
    jar_path: &Path,
    models_dir: &Path,
    types: &[(i32, String)],
) -> EntityAtlas {
    let mut loaded: Vec<(i32, EntityGeometry, Vec<u8>, u32, u32)> = Vec::new();
    let (mut max_w, mut max_h) = (1u32, 1u32);
    for (id, name) in types {
        // The player has no bedrock geo file; use the hardcoded humanoid.
        // Shared-model mobs (cave_spider, horses, …) load an aliased geo file.
        let geo_name = entity_alias(name).map_or(name.as_str(), |(g, _)| g);
        let geo = match geo_name {
            "boat" => Some(boat_geometry(false)),
            "chest_boat" => Some(boat_geometry(true)),
            _ => load_geometry(models_dir, geo_name)
                .or_else(|| (name == "player").then(player_geometry)),
        };
        if let (Some(geo), Some((rgba, w, h))) = (geo, load_entity_texture(jar_path, name)) {
            max_w = max_w.max(w);
            max_h = max_h.max(h);
            loaded.push((*id, geo, rgba, w, h));
        }
    }
    if loaded.is_empty() {
        return EntityAtlas {
            rgba: vec![0; 4],
            width: 1,
            height: 1,
            models: HashMap::new(),
        };
    }

    let cols = (loaded.len() as f64).sqrt().ceil() as u32;
    let rows = (loaded.len() as u32).div_ceil(cols);
    let (aw, ah) = (cols * max_w, rows * max_h);
    let mut rgba = vec![0u8; (aw * ah * 4) as usize];
    let mut models = HashMap::new();

    for (i, (id, geo, tex, w, h)) in loaded.into_iter().enumerate() {
        let (col, row) = (i as u32 % cols, i as u32 / cols);
        let (ox, oy) = (col * max_w, row * max_h);
        for y in 0..h {
            for x in 0..w {
                let src = ((y * w + x) * 4) as usize;
                let dst = (((oy + y) * aw + (ox + x)) * 4) as usize;
                rgba[dst..dst + 4].copy_from_slice(&tex[src..src + 4]);
            }
        }
        models.insert(
            id,
            EntityModelEntry {
                geo,
                atlas_x: ox as f32,
                atlas_y: oy as f32,
            },
        );
    }

    EntityAtlas {
        rgba,
        width: aw,
        height: ah,
        models,
    }
}

fn arr3(v: Option<&Value>) -> Option<[f32; 3]> {
    let a = v?.as_array()?;
    Some([
        a.first()?.as_f64()? as f32,
        a.get(1)?.as_f64()? as f32,
        a.get(2)?.as_f64()? as f32,
    ])
}

fn arr2(v: Option<&Value>) -> Option<[f32; 2]> {
    let a = v?.as_array()?;
    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_map_shared_models_and_variant_skins() {
        // Shared model + own texture.
        assert_eq!(
            entity_alias("cave_spider"),
            Some(("spider", "spider/cave_spider"))
        );
        assert_eq!(
            entity_alias("magma_cube"),
            Some(("magma_cube", "slime/magmacube"))
        );
        assert_eq!(
            entity_alias("horse"),
            Some(("horse_v2", "horse/horse_brown"))
        );
        // Variant skin under its own model.
        assert_eq!(
            entity_alias("parrot"),
            Some(("parrot", "parrot/parrot_red_blue"))
        );
        assert_eq!(entity_alias("drowned"), Some(("drowned", "zombie/drowned")));
        assert_eq!(
            entity_alias("tropical_fish"),
            Some(("tropical_fish", "fish/tropical_a"))
        );
        assert_eq!(
            entity_alias("spectral_arrow"),
            Some(("arrow", "projectiles/spectral_arrow"))
        );
        assert_eq!(entity_alias("tnt_minecart"), Some(("minecart", "minecart")));
        assert_eq!(entity_alias("oak_boat"), Some(("boat", "boat/oak")));
        assert_eq!(boat_geometry(false).cube_count(), 7);
        assert_eq!(boat_geometry(true).cube_count(), 8);
        assert_eq!(entity_alias("cow"), Some(("cow", "cow/temperate_cow")));
        // Unaliased mobs fall through to the name-based defaults.
        assert_eq!(entity_alias("zombie"), None);
    }
}
