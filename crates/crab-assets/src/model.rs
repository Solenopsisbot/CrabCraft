//! Block-model JSON resolution: parent inheritance, texture-map merging, and
//! `#variable` texture-reference resolution.

use std::collections::HashMap;
use std::io::{Read, Seek};

use serde::Deserialize;

use crate::{read_model, ModelJson};

/// A cuboid element with per-face textures and an optional rotation.
#[derive(Deserialize, Clone, Debug)]
pub struct ElementJson {
    pub from: [f32; 3],
    pub to: [f32; 3],
    #[serde(default)]
    pub rotation: Option<RotationJson>,
    #[serde(default)]
    pub faces: HashMap<String, FaceJson>,
}

/// An element rotation about one axis (used by plants, rails, levers, …).
#[derive(Deserialize, Clone, Debug)]
pub struct RotationJson {
    pub origin: [f32; 3],
    pub axis: String,
    pub angle: f32,
    #[serde(default)]
    pub rescale: bool,
}

/// One face of an element. `uv` is `[x1, y1, x2, y2]` in 0..16 texture pixels
/// (defaults to the whole tile); `cullface` names a neighbour direction that,
/// when occupied by a full cube, hides this face.
#[derive(Deserialize, Clone, Debug)]
pub struct FaceJson {
    pub texture: String,
    #[serde(default)]
    pub uv: Option<[f32; 4]>,
    #[serde(default)]
    pub cullface: Option<String>,
    #[serde(default)]
    pub tintindex: Option<i32>,
}

/// A model with its parent chain fully merged.
#[derive(Clone, Default, Debug)]
pub struct Resolved {
    pub textures: HashMap<String, String>,
    pub elements: Vec<ElementJson>,
}

/// Resolves a model by name (e.g. `"block/stone"`), merging parents. Results
/// are memoised in `cache` (parents like `block/cube` are shared widely).
pub fn resolve<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    cache: &mut HashMap<String, Option<Resolved>>,
) -> Option<Resolved> {
    resolve_depth(archive, name, cache, 0)
}

fn resolve_depth<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    cache: &mut HashMap<String, Option<Resolved>>,
    depth: usize,
) -> Option<Resolved> {
    if depth > 16 {
        return None;
    }
    if let Some(cached) = cache.get(name) {
        return cached.clone();
    }

    let result = (|| {
        let json: ModelJson = read_model(archive, name)?;
        let mut resolved = match &json.parent {
            Some(parent) => resolve_depth(archive, parent, cache, depth + 1).unwrap_or_default(),
            None => Resolved::default(),
        };
        for (key, value) in json.textures {
            resolved.textures.insert(key, value);
        }
        if let Some(elements) = json.elements {
            resolved.elements = elements;
        }
        Some(resolved)
    })();

    cache.insert(name.to_string(), result.clone());
    result
}

/// Resolves a face texture reference (`"#side"` or `"block/stone"`) through the
/// merged texture map to a concrete texture path.
pub fn resolve_texture(textures: &HashMap<String, String>, reference: &str) -> Option<String> {
    let mut key = reference.to_string();
    for _ in 0..16 {
        match key.strip_prefix('#') {
            Some(var) => key = textures.get(var)?.clone(),
            None => return Some(key),
        }
    }
    None
}
