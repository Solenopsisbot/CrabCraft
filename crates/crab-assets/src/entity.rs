//! Entity geometry and texture discovery.
//!
//! Vanilla **Java** entity models are code-defined and are not resource files
//! in a client jar. This module therefore reads entity textures (and optional
//! geometry JSON supplied by a resource pack) directly from the user's jar,
//! then synthesizes a stable textured model from the selected registry hitbox
//! when Java geometry is unavailable. The JSON parser remains available for
//! custom packs and for callers that already have compatible geometry data.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek};
use std::path::Path;

use serde_json::Value;

#[path = "vanilla_models.rs"]
mod vanilla_models;

/// A single box of an entity model (model-space coordinates, pixels).
#[derive(Clone, Debug)]
pub struct Cube {
    /// Minimum corner.
    pub origin: [f32; 3],
    pub size: [f32; 3],
    /// Box-UV origin in the texture (pixels).
    pub uv: [f32; 2],
    pub mirror: bool,
}

/// A model bone: its pivot, rest rotation (degrees), and cubes. Geometry JSON
/// uses the same flat bone representation, while generated jar-only models use
/// the two bones needed for head and walk animation.
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

const MAX_GEOMETRY_BYTES: u64 = 8 * 1024 * 1024;

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

/// The classic (wide-arm) Java player model. Box UVs match the 64x64 skin
/// layout; bone names (`*_arm`/`*_leg`) drive the walk animation.
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

/// Built-in Java-style boat hull. Textures still come from the user's Java
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

/// Maps a registry entity name to its shared model base name and jar texture
/// path (relative to `textures/entity/`) for mobs whose asset names differ from
/// the entity name — shared models, variant skins, or subfolders. Without this
/// table those entities would miss their texture and render as boxes.
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
        // Projectiles and vehicle variants reuse a common model. Some of these
        // assets are named differently from their Java registry type.
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
        // The 1.20 registry uses generic boat names while the jar stores the
        // concrete wood variants. Oak is the vanilla fallback for those IDs.
        "boat" => ("boat", "boat/oak"),
        "chest_boat" => ("chest_boat", "chest_boat/oak"),
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

/// Loads custom entity geometry from `<models_dir>/<name>.geo.json`.
///
/// This is retained for callers with an existing compatible geometry pack.
/// Vanilla Java client jars do not contain these files; use
/// [`load_geometry_from_jar`] for jar-only loading.
pub fn load_geometry(models_dir: &Path, name: &str) -> Option<EntityGeometry> {
    // Accept both the common geometry suffix and plain JSON used by a few
    // projectile models in third-party packs.
    for suffix in [".geo.json", ".json"] {
        if let Ok(text) = fs::read_to_string(models_dir.join(format!("{name}{suffix}"))) {
            if let Some(geometry) = parse_geometry(&text) {
                return Some(geometry);
            }
        }
    }
    None
}

/// Loads optional entity geometry JSON embedded in a client/resource-pack jar.
///
/// Java's vanilla models are compiled into the client and are therefore not
/// discoverable as JSON. Custom packs may nevertheless provide geometry under
/// one of the conventional `models/entity` or `geo` paths, so those files are
/// honored before the generated hitbox model is used.
pub fn load_geometry_from_jar(jar_path: &Path, name: &str) -> Option<EntityGeometry> {
    let file = fs::File::open(jar_path).ok()?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).ok()?;
    load_geometry_from_archive(&mut archive, name)
}

fn load_geometry_from_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<EntityGeometry> {
    let geometry_name = entity_alias(name).map_or(name, |(geometry, _)| geometry);
    let names = [geometry_name, name];
    for candidate in names {
        for path in [
            format!("assets/minecraft/models/entity/{candidate}.geo.json"),
            format!("assets/minecraft/models/entity/{candidate}.json"),
            format!("assets/minecraft/geo/{candidate}.geo.json"),
            format!("assets/minecraft/geo/{candidate}.json"),
        ] {
            let Some(bytes) = read_zip_entry(archive, &path) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            if let Some(geometry) = parse_geometry(text) {
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
    load_entity_texture_from_archive(&mut archive, name)
}

fn load_entity_texture_from_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<(Vec<u8>, u32, u32)> {
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
        let Some(bytes) = read_zip_entry(archive, &candidate) else {
            continue;
        };
        if let Ok(img) = image::load_from_memory(&bytes) {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            return Some((rgba.into_raw(), w, h));
        }
    }
    None
}

fn load_painting_texture_from_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<(Vec<u8>, u32, u32)> {
    let path = format!("assets/minecraft/textures/painting/{name}.png");
    let bytes = read_zip_entry(archive, &path)?;
    let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    Some((image.into_raw(), width, height))
}

fn read_zip_entry<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, path: &str) -> Option<Vec<u8>> {
    let mut entry = archive.by_name(path).ok()?;
    let mut bytes = Vec::new();
    entry
        .by_ref()
        .take(MAX_GEOMETRY_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_GEOMETRY_BYTES {
        return None;
    }
    Some(bytes)
}

/// Builds a deterministic textured approximation from an entity hitbox.
///
/// This is intentionally data-driven rather than a guessed protocol mapping:
/// the dimensions come from the selected generated registry, while the texture
/// dimensions come from the user's jar. It keeps every ordinary entity visible
/// without requiring a separate geometry checkout. The result is used only when neither
/// custom jar geometry nor the built-in boat/player models is available.
#[must_use]
pub fn generated_entity_geometry(
    name: &str,
    width: f32,
    height: f32,
    texture_width: u32,
    texture_height: u32,
) -> Option<EntityGeometry> {
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || texture_width == 0
        || texture_height == 0
    {
        return None;
    }
    let geometry_name = entity_alias(name).map_or(name, |(geometry, _)| geometry);
    match geometry_name {
        "boat" => return Some(boat_geometry(false)),
        "chest_boat" => return Some(boat_geometry(true)),
        "player" => return Some(player_geometry()),
        _ => {}
    }

    let w = width * 16.0;
    let h = height * 16.0;
    // Splitting the box gives the generic model a useful head turn and walk
    // animation while preserving the registry hitbox exactly at its extremes.
    let body_h = (h * 0.68).clamp(1.0, h);
    let head_h = (h - body_h).max(0.0);
    let body = Bone {
        name: "body".to_owned(),
        pivot: [0.0, body_h, 0.0],
        rotation: [0.0; 3],
        cubes: vec![Cube {
            origin: [-w * 0.5, 0.0, -w * 0.5],
            size: [w, body_h, w],
            uv: [0.0, 0.0],
            mirror: false,
        }],
    };
    let mut bones = vec![body];
    if head_h > 0.0 {
        let head_w = (w * 0.86).max(1.0);
        bones.push(Bone {
            name: "head".to_owned(),
            pivot: [0.0, body_h, 0.0],
            rotation: [0.0; 3],
            cubes: vec![Cube {
                origin: [-head_w * 0.5, body_h, -head_w * 0.5],
                size: [head_w, head_h, head_w],
                uv: [0.0, 0.0],
                mirror: false,
            }],
        });
    }
    Some(EntityGeometry {
        texture_width: texture_width as f32,
        texture_height: texture_height as f32,
        bones,
    })
}

fn entity_dimensions(registries: crab_registry::RegistrySet, name: &str) -> Option<(f32, f32)> {
    registries
        .entities()
        .iter()
        .find(|entity| entity.name == name)
        .map(|entity| (entity.width, entity.height))
}

/// Loads every entity texture from a jar and creates a model for each registry
/// type. Geometry JSON embedded in the jar wins; otherwise a generated model
/// based on the active registry hitbox is used. This convenience form uses the
/// process-global registry selection; session-owned callers should prefer
/// [`load_entity_atlas_from_jar_with_registry`].
pub fn load_entity_atlas_from_jar(jar_path: &Path, types: &[(i32, String)]) -> EntityAtlas {
    load_entity_atlas_from_jar_with_registry(jar_path, crab_registry::RegistrySet::global(), types)
}

/// Session-scoped form of [`load_entity_atlas_from_jar`] that keeps generated
/// model dimensions aligned with the active protocol registry.
pub fn load_entity_atlas_from_jar_with_registry(
    jar_path: &Path,
    registries: crab_registry::RegistrySet,
    types: &[(i32, String)],
) -> EntityAtlas {
    load_entity_atlas_impl(jar_path, None, registries, types)
}

/// One entity model placed in the shared entity atlas.
#[derive(Clone, Debug)]
pub struct EntityModelEntry {
    pub geo: EntityGeometry,
    /// Top-left of this entity's texture within the atlas (pixels).
    pub atlas_x: f32,
    pub atlas_y: f32,
}

/// A painting texture placed in the shared entity atlas.
#[derive(Clone, Copy, Debug)]
pub struct PaintingAtlasEntry {
    /// Top-left of the painting texture within the atlas (pixels).
    pub atlas_x: f32,
    pub atlas_y: f32,
    pub width: u32,
    pub height: u32,
}

/// The vanilla 1.20 painting registry order. The protocol transmits the
/// registry id; the client jar stores these names under `textures/painting`.
#[must_use]
pub fn painting_texture_name(variant: u32) -> Option<&'static str> {
    Some(match variant {
        0 => "kebab",
        1 => "aztec",
        2 => "alban",
        3 => "aztec2",
        4 => "bomb",
        5 => "plant",
        6 => "wasteland",
        7 => "pool",
        8 => "courbet",
        9 => "sea",
        10 => "sunset",
        11 => "creebet",
        12 => "wanderer",
        13 => "graham",
        14 => "match",
        15 => "bust",
        16 => "stage",
        17 => "void",
        18 => "skull_and_roses",
        19 => "wither",
        20 => "fighters",
        21 => "pointer",
        22 => "pigscene",
        23 => "burning_skull",
        24 => "skeleton",
        25 => "donkey_kong",
        26 => "earth",
        27 => "wind",
        28 => "water",
        29 => "fire",
        _ => return None,
    })
}

/// A stitched atlas of entity textures plus the geometry + placement per type.
pub struct EntityAtlas {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub models: HashMap<i32, EntityModelEntry>,
    /// Painting variant textures keyed by the protocol registry id.
    pub paintings: HashMap<u32, PaintingAtlasEntry>,
}

/// Loads entity geometry from a caller-provided directory when present, then
/// falls back to jar geometry and the generated hitbox model. This compatibility
/// entry point no longer requires the directory; pass an empty path to use the
/// jar-only behavior exposed by [`load_entity_atlas_from_jar`].
pub fn load_entity_atlas(
    jar_path: &Path,
    models_dir: &Path,
    types: &[(i32, String)],
) -> EntityAtlas {
    let models_dir = (!models_dir.as_os_str().is_empty()).then_some(models_dir);
    load_entity_atlas_impl(
        jar_path,
        models_dir,
        crab_registry::RegistrySet::global(),
        types,
    )
}

fn load_entity_atlas_impl(
    jar_path: &Path,
    models_dir: Option<&Path>,
    registries: crab_registry::RegistrySet,
    types: &[(i32, String)],
) -> EntityAtlas {
    let Ok(file) = fs::File::open(jar_path) else {
        return empty_entity_atlas();
    };
    let Ok(mut archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return empty_entity_atlas();
    };
    let mut loaded: Vec<(i32, EntityGeometry, Vec<u8>, u32, u32)> = Vec::new();
    let mut painting_textures: Vec<(u32, Vec<u8>, u32, u32)> = Vec::new();
    let (mut max_w, mut max_h) = (1u32, 1u32);
    for (id, name) in types {
        let geo_name = entity_alias(name).map_or(name.as_str(), |(g, _)| g);
        let geo = models_dir
            .and_then(|dir| load_geometry(dir, geo_name))
            .or_else(|| load_geometry_from_archive(&mut archive, name));
        let Some((rgba, w, h)) = load_entity_texture_from_archive(&mut archive, name) else {
            continue;
        };
        let geo = geo
            .or_else(|| vanilla_models::geometry_for(name, w, h))
            .or_else(|| {
                let (width, height) = entity_dimensions(registries, name)?;
                generated_entity_geometry(name, width, height, w, h)
            });
        if let Some(geo) = geo {
            max_w = max_w.max(w);
            max_h = max_h.max(h);
            loaded.push((*id, geo, rgba, w, h));
        }
    }
    for variant in 0..30 {
        let Some(name) = painting_texture_name(variant) else {
            continue;
        };
        let Some((rgba, w, h)) = load_painting_texture_from_archive(&mut archive, name) else {
            continue;
        };
        max_w = max_w.max(w);
        max_h = max_h.max(h);
        painting_textures.push((variant, rgba, w, h));
    }
    if loaded.is_empty() && painting_textures.is_empty() {
        return empty_entity_atlas();
    }

    let total = loaded.len() + painting_textures.len();
    let cols = (total as f64).sqrt().ceil() as u32;
    let rows = (total as u32).div_ceil(cols);
    let (aw, ah) = (cols * max_w, rows * max_h);
    let mut rgba = vec![0u8; (aw * ah * 4) as usize];
    let mut models = HashMap::new();
    let mut paintings = HashMap::new();

    let entity_count = loaded.len();
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
    for (i, (variant, tex, w, h)) in painting_textures.into_iter().enumerate() {
        let i = entity_count + i;
        let (col, row) = (i as u32 % cols, i as u32 / cols);
        let (ox, oy) = (col * max_w, row * max_h);
        for y in 0..h {
            for x in 0..w {
                let src = ((y * w + x) * 4) as usize;
                let dst = (((oy + y) * aw + (ox + x)) * 4) as usize;
                rgba[dst..dst + 4].copy_from_slice(&tex[src..src + 4]);
            }
        }
        paintings.insert(
            variant,
            PaintingAtlasEntry {
                atlas_x: ox as f32,
                atlas_y: oy as f32,
                width: w,
                height: h,
            },
        );
    }

    EntityAtlas {
        rgba,
        width: aw,
        height: ah,
        models,
        paintings,
    }
}

fn empty_entity_atlas() -> EntityAtlas {
    EntityAtlas {
        rgba: vec![0; 4],
        width: 1,
        height: 1,
        models: HashMap::new(),
        paintings: HashMap::new(),
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
    use std::io::{Cursor, Write};

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
        assert_eq!(entity_alias("boat"), Some(("boat", "boat/oak")));
        assert_eq!(
            entity_alias("chest_boat"),
            Some(("chest_boat", "chest_boat/oak"))
        );
        assert_eq!(entity_alias("oak_boat"), Some(("boat", "boat/oak")));
        assert_eq!(boat_geometry(false).cube_count(), 7);
        assert_eq!(boat_geometry(true).cube_count(), 8);
        assert_eq!(entity_alias("cow"), Some(("cow", "cow/temperate_cow")));
        // Unaliased mobs fall through to the name-based defaults.
        assert_eq!(entity_alias("zombie"), None);
    }

    #[test]
    fn generated_geometry_uses_registry_dimensions() {
        let geometry = generated_entity_geometry("allay", 0.35, 0.6, 32, 32).unwrap();
        assert_eq!(geometry.texture_width, 32.0);
        assert_eq!(geometry.texture_height, 32.0);
        assert_eq!(geometry.cube_count(), 2);
        let body = &geometry.bones[0].cubes[0];
        assert_eq!(body.origin, [-2.8, 0.0, -2.8]);
        assert!((body.size[1] - 6.528).abs() < 0.001);
    }

    #[test]
    fn geometry_json_can_be_read_from_a_jar() {
        let json = r#"{
            "format_version": "1.12.0",
            "minecraft:geometry": [{
                "description": {"texture_width": 16, "texture_height": 16},
                "bones": [{"name": "body", "pivot": [0, 0, 0],
                    "cubes": [{"origin": [-1, 0, -1], "size": [2, 2, 2], "uv": [0, 0]}]}]
            }]
        }"#;
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            writer
                .start_file(
                    "assets/minecraft/models/entity/test.geo.json",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(json.as_bytes()).unwrap();
            writer.finish().unwrap();
        }
        let path = std::env::temp_dir().join(format!(
            "crab-assets-entity-{}-{}.jar",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, bytes.into_inner()).unwrap();
        let geometry = load_geometry_from_jar(&path, "test").unwrap();
        assert_eq!(geometry.cube_count(), 1);
        assert_eq!(geometry.texture_width, 16.0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn jar_texture_gets_generated_model_without_external_geometry() {
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([220, 80, 40, 255]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            writer
                .start_file(
                    "assets/minecraft/textures/entity/allay.png",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(png.get_ref()).unwrap();
            writer.finish().unwrap();
        }
        let path = std::env::temp_dir().join(format!(
            "crab-assets-entity-texture-{}-{}.jar",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, bytes.into_inner()).unwrap();
        let types = vec![(0, "allay".to_owned())];
        let atlas = load_entity_atlas_from_jar(&path, &types);
        let model = atlas.models.get(&0).unwrap();
        assert_eq!(atlas.width, 4);
        assert_eq!(atlas.height, 4);
        // The jar supplies only the texture; the rest-pose geometry comes from
        // the bundled vanilla model table (allay has two wings plus body parts).
        assert_eq!(model.geo.cube_count(), 7);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn jar_painting_texture_is_indexed_in_entity_atlas() {
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            16,
            16,
            image::Rgba([40, 120, 220, 255]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            writer
                .start_file(
                    "assets/minecraft/textures/painting/kebab.png",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(png.get_ref()).unwrap();
            writer.finish().unwrap();
        }
        let path = std::env::temp_dir().join(format!(
            "crab-assets-painting-{}-{}.jar",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, bytes.into_inner()).unwrap();
        let atlas = load_entity_atlas_from_jar(&path, &[]);
        let painting = atlas.paintings.get(&0).unwrap();
        assert_eq!((painting.width, painting.height), (16, 16));
        assert!(painting.atlas_x < atlas.width as f32);
        assert!(painting.atlas_y < atlas.height as f32);
        assert_eq!(painting_texture_name(0), Some("kebab"));
        assert_eq!(painting_texture_name(30), None);
        fs::remove_file(path).unwrap();
    }
}
