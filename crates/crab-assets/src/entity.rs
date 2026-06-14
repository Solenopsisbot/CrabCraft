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

/// An entity model: its texture dimensions and flattened cube list.
#[derive(Clone, Debug)]
pub struct EntityGeometry {
    pub texture_width: f32,
    pub texture_height: f32,
    pub cubes: Vec<Cube>,
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

    let bones = geo.get("bones")?.as_array()?;
    let mut cubes = Vec::new();
    for bone in bones {
        let bone_mirror = bone.get("mirror").and_then(Value::as_bool).unwrap_or(false);
        let Some(bone_cubes) = bone.get("cubes").and_then(Value::as_array) else {
            continue;
        };
        for c in bone_cubes {
            let (Some(origin), Some(size)) = (arr3(c.get("origin")), arr3(c.get("size"))) else {
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
    Some(EntityGeometry {
        texture_width: tw,
        texture_height: th,
        cubes,
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
