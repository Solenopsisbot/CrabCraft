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

/// Loads an entity model from `<models_dir>/<name>.geo.json`.
pub fn load_geometry(models_dir: &Path, name: &str) -> Option<EntityGeometry> {
    let text = fs::read_to_string(models_dir.join(format!("{name}.geo.json"))).ok()?;
    parse_geometry(&text)
}

/// Loads an RGBA entity texture from the client jar, trying the common
/// `entity/<name>/<name>.png` then `entity/<name>.png` layouts.
pub fn load_entity_texture(jar_path: &Path, name: &str) -> Option<(Vec<u8>, u32, u32)> {
    let file = fs::File::open(jar_path).ok()?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).ok()?;
    for candidate in [
        format!("assets/minecraft/textures/entity/{name}/{name}.png"),
        format!("assets/minecraft/textures/entity/{name}.png"),
    ] {
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
        if let (Some(geo), Some((rgba, w, h))) = (
            load_geometry(models_dir, name),
            load_entity_texture(jar_path, name),
        ) {
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
