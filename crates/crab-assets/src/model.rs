//! Block-model JSON resolution: parent inheritance, texture-map merging, and
//! `#variable` texture-reference resolution.

use std::collections::HashMap;
use std::io::{Read, Seek};

use serde::Deserialize;

use crate::{read_model, ModelJson};

/// A cuboid element with per-face textures.
#[derive(Deserialize, Clone, Debug)]
pub struct ElementJson {
    pub from: [f32; 3],
    pub to: [f32; 3],
    #[serde(default)]
    pub faces: HashMap<String, FaceJson>,
}

/// One face of an element. (Per-face `uv` is parsed-and-ignored for now; we map
/// each face to the full tile.)
#[derive(Deserialize, Clone, Debug)]
pub struct FaceJson {
    pub texture: String,
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
