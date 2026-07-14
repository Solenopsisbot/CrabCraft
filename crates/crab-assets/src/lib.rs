//! # crab-assets
//!
//! Loads Minecraft block **models** and **textures** straight from a client jar
//! (your own install — assets are never bundled with Crabcraft) and stitches a
//! texture atlas for the renderer.
//!
//! Scope: **full-cube** blocks resolve to per-face atlas textures. Blocks with
//! `elements` (slabs, stairs, plants, lanterns, …) resolve to element geometry
//! ([`ElementData`]). Vanilla `variants` and `multipart` blockstate definitions
//! are matched against the active protocol registry's property values, including
//! weighted model alternatives and blockstate rotations.
//!
//! Model resolution follows vanilla: walk the `parent` chain, merge `textures`
//! maps (child wins), take the deepest `elements`, then resolve each face's
//! `#variable` texture reference through the merged map.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::Deserialize;

mod model;
use model::{resolve, ElementJson, Resolved};

pub mod entity;
pub use entity::{
    boat_geometry, load_entity_atlas, load_entity_texture, load_geometry, parse_geometry,
    player_geometry, Bone, Cube, EntityAtlas, EntityGeometry, EntityModelEntry,
};

pub mod gui;
pub use gui::{load_gui_atlas, Glyph, GuiAtlas};

/// Error type for asset loading.
#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("image: {0}")]
    Image(#[from] image::ImageError),
}

/// Complete CPU-side resource generation built transactionally before any GPU
/// resources are replaced.
pub struct ResourceSet {
    pub blocks: Atlas,
    pub items: ItemAtlas,
    pub gui: GuiAtlas,
    pub entities: Option<EntityAtlas>,
    pub destroy_stages: Option<(Vec<u8>, u32, u32)>,
}

/// Loads every CPU-side resource derived from one validated archive. Callers
/// can perform this on a worker and commit the returned set atomically.
pub fn load_resource_set(
    archive: &Path,
    registries: crab_registry::RegistrySet,
    entity_models: Option<&Path>,
) -> Result<ResourceSet, AssetError> {
    let block_names: Vec<String> = registries
        .blocks()
        .iter()
        .map(|block| block.name.to_owned())
        .collect();
    let item_names: Vec<String> = registries
        .items()
        .iter()
        .map(|item| item.name.to_owned())
        .collect();
    let entities = entity_models.map(|models| {
        let types: Vec<(i32, String)> = registries
            .entities()
            .iter()
            .map(|entity| (entity.id as i32, entity.name.to_owned()))
            .collect();
        load_entity_atlas(archive, models, &types)
    });
    Ok(ResourceSet {
        blocks: load_block_atlas_with_registry(archive, &block_names, registries)?,
        items: load_item_atlas(archive, &item_names)?,
        gui: load_gui_atlas(archive)?,
        entities,
        destroy_stages: load_destroy_stages(archive),
    })
}

const TILE: u32 = 16;
/// Order matches `crab-render`'s face order: +X, -X, +Y, -Y, +Z, -Z.
const FACE_NAMES: [&str; 6] = ["east", "west", "up", "down", "south", "north"];

/// Per-face atlas coordinates + tint multiplier.
#[derive(Clone, Copy, Debug)]
pub struct FaceTex {
    /// `[u0, v0, u1, v1]` in atlas (0..1) space.
    pub uv: [f32; 4],
    /// RGB multiplied with the sampled texel (white = untinted).
    pub tint: [f32; 3],
}

/// The six faces of a block, in `crab-render`'s face order.
#[derive(Clone, Copy, Debug)]
pub struct BlockModel {
    pub faces: [FaceTex; 6],
    /// True if the block resolved to a real textured cube (vs a flat fallback).
    pub textured: bool,
}

/// One face of a model element, prepared for the mesher.
#[derive(Clone, Copy, Debug)]
pub struct ElementFace {
    /// Atlas sub-rect `[u0, v0, u1, v1]` this face samples.
    pub uv: [f32; 4],
    pub tint: [f32; 3],
    /// Neighbour face index (0..6, `crab-render` order) that hides this face
    /// when occupied by a full cube, or `None` to always draw.
    pub cull: Option<u8>,
    /// Clockwise texture rotation in quarter turns.
    pub uv_rotation: u8,
}

/// An element rotation about one axis, in 0..16 model space.
#[derive(Clone, Copy, Debug)]
pub struct ElementRotation {
    pub origin: [f32; 3],
    /// 0 = X, 1 = Y, 2 = Z.
    pub axis: u8,
    pub angle: f32,
    pub rescale: bool,
}

/// One cuboid of a non-full-cube block model, in 0..16 model space. Faces are
/// in `crab-render` order (+X, -X, +Y, -Y, +Z, -Z); `None` = absent.
#[derive(Clone, Debug)]
pub struct ElementData {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub rotation: Option<ElementRotation>,
    pub faces: [Option<ElementFace>; 6],
}

/// A model `display` transform, using vanilla model-space units and degrees.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct DisplayTransform {
    #[serde(default)]
    pub rotation: [f32; 3],
    #[serde(default)]
    pub translation: [f32; 3],
    #[serde(default = "unit_scale")]
    pub scale: [f32; 3],
}

impl Default for DisplayTransform {
    fn default() -> Self {
        Self {
            rotation: [0.0; 3],
            translation: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

const fn unit_scale() -> [f32; 3] {
    [1.0; 3]
}

/// One model application selected by a vanilla blockstate definition.
#[derive(Clone, Debug)]
pub struct BlockStateModelPart {
    /// Weighted model alternatives. Vanilla chooses one pseudo-randomly per
    /// block position; multipart definitions contribute multiple parts.
    pub alternatives: Vec<BlockStateModelAlternative>,
}

#[derive(Clone, Debug)]
pub struct BlockStateModelAlternative {
    pub elements: Vec<ElementData>,
    /// Blockstate `x`/`y` model rotation in degrees.
    pub rotation: [f32; 3],
    pub weight: u32,
    pub uvlock: bool,
}

/// A stitched texture atlas plus per-block face lookups.
pub struct Atlas {
    /// RGBA8 atlas pixels, row-major.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    blocks: HashMap<String, BlockModel>,
    non_cube: HashMap<String, Vec<ElementData>>,
    state_models: HashMap<u32, Vec<BlockStateModelPart>>,
    state_cubes: HashSet<u32>,
    fallback: BlockModel,
    white_uv: [f32; 4],
    /// When true (the debug atlas), every block is treated as a full cube.
    assume_cube: bool,
    unresolved: Vec<String>,
    missing_textures: Vec<String>,
}

impl Atlas {
    /// Model for a (namespaced or bare) block name, falling back to a flat tile.
    pub fn model(&self, block_name: &str) -> &BlockModel {
        let bare = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
        self.blocks.get(bare).unwrap_or(&self.fallback)
    }

    /// Element geometry for a non-full-cube block (slabs, stairs, plants, …),
    /// or `None` if the block is a full cube / flat fallback.
    pub fn block_elements(&self, block_name: &str) -> Option<&[ElementData]> {
        let bare = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
        self.non_cube.get(bare).map(Vec::as_slice)
    }

    /// Alternate element model retained for legacy state-family fallbacks.
    pub fn block_elements_variant(
        &self,
        block_name: &str,
        variant: &str,
    ) -> Option<&[ElementData]> {
        let bare = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
        self.non_cube
            .get(&format!("{bare}#{variant}"))
            .map(Vec::as_slice)
    }

    /// Model parts selected by the active registry values for `state`.
    pub fn block_state_model(&self, state: u32) -> Option<&[BlockStateModelPart]> {
        self.state_models.get(&state).map(Vec::as_slice)
    }

    /// Whether a specific state resolves to one opaque full-cube model.
    pub fn is_state_cube(&self, state: u32, block_name: &str) -> bool {
        if self.assume_cube {
            true
        } else if self.state_models.contains_key(&state) {
            self.state_cubes.contains(&state)
        } else {
            self.is_cube(block_name)
        }
    }

    /// Whether the block renders as a full cube (so it occludes neighbour faces).
    pub fn is_cube(&self, block_name: &str) -> bool {
        if self.assume_cube {
            return true;
        }
        let bare = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
        self.blocks.contains_key(bare)
    }

    /// UV of the solid-white tile (for tinting flat-coloured geometry like
    /// entity boxes).
    pub fn white_uv(&self) -> [f32; 4] {
        self.white_uv
    }

    /// Requested registry blocks for which neither a blockstate-selected model
    /// nor a legacy direct model could be resolved from the active assets.
    #[must_use]
    pub fn unresolved_models(&self) -> &[String] {
        &self.unresolved
    }

    /// Referenced textures that were absent or could not be decoded.
    #[must_use]
    pub fn missing_textures(&self) -> &[String] {
        &self.missing_textures
    }

    /// A trivial 1-tile white atlas where every block maps to a flat white tile.
    /// Lets the renderer/tests run without a jar (everything is untextured).
    pub fn debug_uniform() -> Self {
        let white = FaceTex {
            uv: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0, 1.0, 1.0],
        };
        Self {
            rgba: vec![255u8; (TILE * TILE * 4) as usize],
            width: TILE,
            height: TILE,
            blocks: HashMap::new(),
            non_cube: HashMap::new(),
            state_models: HashMap::new(),
            state_cubes: HashSet::new(),
            fallback: BlockModel {
                faces: [white; 6],
                textured: false,
            },
            white_uv: [0.0, 0.0, 1.0, 1.0],
            assume_cube: true,
            unresolved: Vec::new(),
            missing_textures: Vec::new(),
        }
    }
}

/// A stitched atlas of flat item icons (one 16x16 tile per resolved item),
/// keyed by bare item name (e.g. `"diamond"`).
pub struct ItemAtlas {
    /// RGBA8 atlas pixels, row-major.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    icons: HashMap<String, [f32; 4]>,
    models: HashMap<String, ItemModel>,
    unresolved: Vec<String>,
    missing_textures: Vec<String>,
}

/// Resolved 3D item geometry and its inherited ground display transform.
#[derive(Clone, Debug)]
pub struct ItemModel {
    pub elements: Vec<ElementData>,
    pub ground: DisplayTransform,
}

impl ItemAtlas {
    /// Atlas UV `[u0, v0, u1, v1]` for an item, if an icon was resolved.
    #[must_use]
    pub fn icon(&self, item_name: &str) -> Option<[f32; 4]> {
        let bare = item_name.strip_prefix("minecraft:").unwrap_or(item_name);
        self.icons.get(bare).copied()
    }

    /// Resolved 3D item model, when the item does not use generated flat layers.
    #[must_use]
    pub fn model(&self, item_name: &str) -> Option<&ItemModel> {
        let bare = item_name.strip_prefix("minecraft:").unwrap_or(item_name);
        self.models.get(bare)
    }

    /// Number of items with a resolved icon.
    #[must_use]
    pub fn len(&self) -> usize {
        self.icons.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.icons.is_empty()
    }

    /// Requested non-air items whose default model and icon could not be
    /// resolved from either the legacy or 1.21.4+ item-definition format.
    #[must_use]
    pub fn unresolved_models(&self) -> &[String] {
        &self.unresolved
    }

    /// Referenced textures that were absent or could not be decoded.
    #[must_use]
    pub fn missing_textures(&self) -> &[String] {
        &self.missing_textures
    }

    /// An atlas with no icons (a single transparent tile). Lets the renderer
    /// run without a jar — the hotbar simply shows no item icons.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            rgba: vec![0u8; (TILE * TILE * 4) as usize],
            width: TILE,
            height: TILE,
            icons: HashMap::new(),
            models: HashMap::new(),
            unresolved: Vec::new(),
            missing_textures: Vec::new(),
        }
    }
}

/// JSON form of a block model file.
#[derive(Deserialize)]
pub(crate) struct ModelJson {
    pub parent: Option<String>,
    #[serde(default)]
    pub textures: HashMap<String, String>,
    #[serde(default)]
    pub elements: Option<Vec<ElementJson>>,
    #[serde(default)]
    pub display: HashMap<String, DisplayTransform>,
}

#[derive(Deserialize)]
struct BlockStateJson {
    #[serde(default)]
    variants: HashMap<String, ModelChoice>,
    #[serde(default)]
    multipart: Vec<MultipartCase>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ModelChoice {
    One(ModelApplication),
    Weighted(Vec<ModelApplication>),
}

impl ModelChoice {
    fn applications(&self) -> &[ModelApplication] {
        match self {
            Self::One(model) => std::slice::from_ref(model),
            Self::Weighted(models) => models,
        }
    }
}

#[derive(Deserialize)]
struct ModelApplication {
    model: String,
    #[serde(default)]
    x: f32,
    #[serde(default)]
    y: f32,
    #[serde(default)]
    weight: Option<u32>,
    #[serde(default)]
    uvlock: bool,
}

#[derive(Deserialize)]
struct MultipartCase {
    #[serde(default)]
    when: Option<serde_json::Value>,
    apply: ModelChoice,
}

struct ParsedStateAlternative {
    elements: Vec<ParsedElem>,
    rotation: [f32; 3],
    weight: u32,
    full_cube: bool,
    uvlock: bool,
}

struct ParsedStatePart {
    alternatives: Vec<ParsedStateAlternative>,
}

fn read_blockstate<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    bare: &str,
) -> Option<BlockStateJson> {
    let path = format!("assets/minecraft/blockstates/{bare}.json");
    let mut file = archive.by_name(&path).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

fn property_condition_matches(condition: &serde_json::Value, properties: &[(&str, &str)]) -> bool {
    let Some(object) = condition.as_object() else {
        return false;
    };
    if let Some(or) = object.get("OR").and_then(serde_json::Value::as_array) {
        return or
            .iter()
            .any(|condition| property_condition_matches(condition, properties));
    }
    if let Some(and) = object.get("AND").and_then(serde_json::Value::as_array) {
        return and
            .iter()
            .all(|condition| property_condition_matches(condition, properties));
    }
    object.iter().all(|(name, expected)| {
        let Some(expected) = expected.as_str() else {
            return false;
        };
        properties
            .iter()
            .find(|(property, _)| *property == name)
            .is_some_and(|(_, actual)| expected.split('|').any(|value| value == *actual))
    })
}

fn variant_matches(key: &str, properties: &[(&str, &str)]) -> bool {
    key.is_empty()
        || key.split(',').all(|entry| {
            let Some((name, expected)) = entry.split_once('=') else {
                return false;
            };
            properties
                .iter()
                .any(|(property, actual)| *property == name && *actual == expected)
        })
}

/// Reads + parses `assets/minecraft/models/<name>.json` from the jar.
pub(crate) fn read_model<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<ModelJson> {
    let (namespace, bare) = split_resource_id(name);
    let path = format!("assets/{namespace}/models/{bare}.json");
    let mut file = archive.by_name(&path).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

fn split_resource_id(id: &str) -> (&str, &str) {
    id.split_once(':').unwrap_or(("minecraft", id))
}

/// Resolves the context-free/default plain model referenced by the 1.21.4+
/// client item definition. Dynamic branches are evaluated as an unselected,
/// unused stack: condition uses `on_false`, while select/range dispatch use
/// their explicit fallback. This is the authoritative default appearance used
/// by inventory entries that do not retain the relevant stack components.
fn read_client_item_model<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    item_id: &str,
) -> Option<Option<String>> {
    let (namespace, bare) = split_resource_id(item_id);
    let path = format!("assets/{namespace}/items/{bare}.json");
    let mut file = archive.by_name(&path).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(default_plain_item_model(value.get("model")?, 0))
}

fn default_plain_item_model(value: &serde_json::Value, depth: usize) -> Option<String> {
    if depth > 32 {
        return None;
    }
    let object = value.as_object()?;
    let kind = object.get("type")?.as_str()?;
    match kind.strip_prefix("minecraft:").unwrap_or(kind) {
        "model" => object.get("model")?.as_str().map(str::to_owned),
        "special" => object.get("base")?.as_str().map(str::to_owned),
        "condition" => object
            .get("on_false")
            .or_else(|| object.get("on_true"))
            .and_then(|model| default_plain_item_model(model, depth + 1)),
        "select" | "range_dispatch" => object
            .get("fallback")
            .and_then(|model| default_plain_item_model(model, depth + 1))
            .or_else(|| {
                object
                    .get("cases")
                    .or_else(|| object.get("entries"))
                    .and_then(serde_json::Value::as_array)
                    .and_then(|entries| entries.first())
                    .and_then(|entry| entry.get("model"))
                    .and_then(|model| default_plain_item_model(model, depth + 1))
            }),
        "composite" => object
            .get("models")?
            .as_array()?
            .iter()
            .find_map(|model| default_plain_item_model(model, depth + 1)),
        "empty" | "bundle/selected_item" => None,
        _ => None,
    }
}

fn load_element_variant<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    cache: &mut HashMap<String, Option<Resolved>>,
    block_elems: &mut HashMap<String, Vec<ParsedElem>>,
    tex_paths: &mut BTreeSet<String>,
    key: String,
    model_name: String,
) {
    let Some(resolved) = resolve(archive, &model_name, cache) else {
        return;
    };
    let elems = parse_elements(&resolved);
    for element in &elems {
        for face in element.faces.iter().flatten() {
            tex_paths.insert(face.path.clone());
        }
    }
    block_elems.insert(key, elems);
}

fn load_state_models<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    cache: &mut HashMap<String, Option<Resolved>>,
    tex_paths: &mut BTreeSet<String>,
    bare: &str,
    registries: crab_registry::RegistrySet,
) -> HashMap<u32, Vec<ParsedStatePart>> {
    let Some(definition) = read_blockstate(archive, bare) else {
        return HashMap::new();
    };
    let Some(block) = registries.block_by_name(bare) else {
        return HashMap::new();
    };
    let mut states = HashMap::new();
    for state in block.min_state..=block.max_state {
        let Some(properties) = registries.block_state_properties(state) else {
            continue;
        };
        let mut choices: Vec<&ModelChoice> = Vec::new();
        if let Some((_, choice)) = definition
            .variants
            .iter()
            .filter(|(key, _)| variant_matches(key, &properties))
            .max_by_key(|(key, _)| {
                if key.is_empty() {
                    0
                } else {
                    key.split(',').count()
                }
            })
        {
            choices.push(choice);
        }
        for case in &definition.multipart {
            if case
                .when
                .as_ref()
                .is_none_or(|condition| property_condition_matches(condition, &properties))
            {
                choices.push(&case.apply);
            }
        }

        let mut parts = Vec::new();
        for choice in choices {
            let mut alternatives = Vec::new();
            for application in choice.applications() {
                let Some(resolved) = resolve(archive, &application.model, cache) else {
                    continue;
                };
                let elements = parse_elements(&resolved);
                if elements.is_empty() {
                    continue;
                }
                for element in &elements {
                    for face in element.faces.iter().flatten() {
                        tex_paths.insert(face.path.clone());
                    }
                }
                alternatives.push(ParsedStateAlternative {
                    elements,
                    rotation: [application.x, application.y, 0.0],
                    weight: application.weight.unwrap_or(1).max(1),
                    full_cube: cube_faces(&resolved).is_some(),
                    uvlock: application.uvlock,
                });
            }
            if !alternatives.is_empty() {
                parts.push(ParsedStatePart { alternatives });
            }
        }
        if !parts.is_empty() {
            states.insert(state, parts);
        }
    }
    states
}

/// Loads block models + textures for `block_names` from `jar_path` and builds an
/// atlas. Blocks that aren't full cubes get a flat fallback colour.
pub fn load_block_atlas(jar_path: &Path, block_names: &[String]) -> Result<Atlas, AssetError> {
    load_block_atlas_with_registry(jar_path, block_names, crab_registry::RegistrySet::global())
}

/// Session-scoped form of [`load_block_atlas`].
pub fn load_block_atlas_with_registry(
    jar_path: &Path,
    block_names: &[String],
    registries: crab_registry::RegistrySet,
) -> Result<Atlas, AssetError> {
    let file = File::open(jar_path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))?;

    // Resolve each block to either full-cube faces or element geometry.
    let mut cache: HashMap<String, Option<Resolved>> = HashMap::new();
    let mut block_faces: HashMap<String, [(Option<String>, bool); 6]> = HashMap::new();
    let mut block_elems: HashMap<String, Vec<ParsedElem>> = HashMap::new();
    let mut state_models: HashMap<u32, Vec<ParsedStatePart>> = HashMap::new();
    let mut tex_paths: BTreeSet<String> = BTreeSet::new();
    let mut unresolved = Vec::new();

    for name in block_names {
        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
        // Fluids have no block model JSON in the vanilla client jar. Their
        // renderer is special-cased by vanilla, so provide the equivalent
        // still/top and flowing/side texture mapping explicitly instead of
        // leaving them on the opaque hashed-colour fallback.
        if matches!(bare, "water" | "bubble_column" | "lava") {
            let fluid = if bare == "bubble_column" {
                "water"
            } else {
                bare
            };
            let still = format!("minecraft:block/{fluid}_still");
            let flow = format!("minecraft:block/{fluid}_flow");
            tex_paths.insert(still.clone());
            tex_paths.insert(flow.clone());
            let tinted = fluid == "water";
            block_faces.insert(
                bare.to_string(),
                [
                    (Some(flow.clone()), tinted),
                    (Some(flow.clone()), tinted),
                    (Some(still.clone()), tinted),
                    (Some(still), tinted),
                    (Some(flow.clone()), tinted),
                    (Some(flow), tinted),
                ],
            );
            continue;
        }
        let selected_models =
            load_state_models(&mut archive, &mut cache, &mut tex_paths, bare, registries);
        let resolved_by_state = !selected_models.is_empty();
        state_models.extend(selected_models);
        if bare.ends_with("_door") && !bare.ends_with("_trapdoor") {
            for half in ["bottom", "top"] {
                for hinge in ["left", "right"] {
                    for open in [false, true] {
                        let suffix = if open { "_open" } else { "" };
                        load_element_variant(
                            &mut archive,
                            &mut cache,
                            &mut block_elems,
                            &mut tex_paths,
                            format!("{bare}#{half}_{hinge}{}", if open { "_open" } else { "" }),
                            format!("block/{bare}_{half}_{hinge}{suffix}"),
                        );
                    }
                }
            }
            continue;
        }
        if bare.ends_with("_trapdoor") {
            for variant in ["bottom", "top", "open"] {
                load_element_variant(
                    &mut archive,
                    &mut cache,
                    &mut block_elems,
                    &mut tex_paths,
                    format!("{bare}#{variant}"),
                    format!("block/{bare}_{variant}"),
                );
            }
            continue;
        }
        if bare == "redstone_wire" {
            for variant in ["dot", "north", "south", "east", "west", "up"] {
                let (key, model) = match variant {
                    "dot" => ("dot", "redstone_dust_dot"),
                    "north" => ("north", "redstone_dust_side0"),
                    "south" => ("south", "redstone_dust_side_alt0"),
                    "east" => ("east", "redstone_dust_side_alt1"),
                    "west" => ("west", "redstone_dust_side1"),
                    _ => ("up", "redstone_dust_up"),
                };
                load_element_variant(
                    &mut archive,
                    &mut cache,
                    &mut block_elems,
                    &mut tex_paths,
                    format!("{bare}#{key}"),
                    format!("block/{model}"),
                );
            }
            continue;
        }
        if matches!(bare, "furnace" | "blast_furnace" | "smoker") {
            for (variant, suffix) in [("off", ""), ("on", "_on")] {
                load_element_variant(
                    &mut archive,
                    &mut cache,
                    &mut block_elems,
                    &mut tex_paths,
                    format!("{bare}#{variant}"),
                    format!("block/{bare}{suffix}"),
                );
            }
            continue;
        }
        if matches!(bare, "campfire" | "soul_campfire") {
            for (variant, suffix) in [("on", ""), ("off", "_off")] {
                load_element_variant(
                    &mut archive,
                    &mut cache,
                    &mut block_elems,
                    &mut tex_paths,
                    format!("{bare}#{variant}"),
                    format!("block/{bare}{suffix}"),
                );
            }
            continue;
        }
        if bare.ends_with("_fence") || bare.ends_with("_pane") || bare == "iron_bars" {
            for variant in ["post", "side"] {
                let variant_model = format!("block/{bare}_{variant}");
                let Some(resolved) = resolve(&mut archive, &variant_model, &mut cache) else {
                    continue;
                };
                let elems = parse_elements(&resolved);
                for element in &elems {
                    for face in element.faces.iter().flatten() {
                        tex_paths.insert(face.path.clone());
                    }
                }
                block_elems.insert(format!("{bare}#{variant}"), elems);
            }
            continue;
        }
        if bare.ends_with("_wall") {
            for variant in ["post", "side", "side_tall"] {
                let variant_model = format!("block/{bare}_{variant}");
                let Some(resolved) = resolve(&mut archive, &variant_model, &mut cache) else {
                    continue;
                };
                let elems = parse_elements(&resolved);
                for element in &elems {
                    for face in element.faces.iter().flatten() {
                        tex_paths.insert(face.path.clone());
                    }
                }
                block_elems.insert(format!("{bare}#{variant}"), elems);
            }
            continue;
        }
        if bare.ends_with("rail") {
            for variant in ["raised_ne", "raised_sw"] {
                load_element_variant(
                    &mut archive,
                    &mut cache,
                    &mut block_elems,
                    &mut tex_paths,
                    format!("{bare}#{variant}"),
                    format!("block/{bare}_{variant}"),
                );
            }
            if bare == "rail" {
                load_element_variant(
                    &mut archive,
                    &mut cache,
                    &mut block_elems,
                    &mut tex_paths,
                    format!("{bare}#corner"),
                    "block/rail_corner".to_string(),
                );
            } else {
                for variant in ["on", "on_raised_ne", "on_raised_sw"] {
                    load_element_variant(
                        &mut archive,
                        &mut cache,
                        &mut block_elems,
                        &mut tex_paths,
                        format!("{bare}#{variant}"),
                        format!("block/{bare}_{variant}"),
                    );
                }
            }
        }
        let model_name = format!("block/{bare}");
        let Some(resolved) = resolve(&mut archive, &model_name, &mut cache) else {
            if !resolved_by_state {
                unresolved.push(name.clone());
            }
            continue;
        };
        if let Some(faces) = cube_faces(&resolved) {
            for (path, _tinted) in &faces {
                if let Some(path) = path {
                    tex_paths.insert(path.clone());
                }
            }
            block_faces.insert(bare.to_string(), faces);
        } else if !resolved.elements.is_empty() {
            let elems = parse_elements(&resolved);
            for e in &elems {
                for f in e.faces.iter().flatten() {
                    tex_paths.insert(f.path.clone());
                }
            }
            block_elems.insert(bare.to_string(), elems);
        }
        if bare.ends_with("_stairs") {
            for variant in ["inner", "outer"] {
                let variant_model = format!("block/{bare}_{variant}");
                let Some(resolved) = resolve(&mut archive, &variant_model, &mut cache) else {
                    continue;
                };
                let elems = parse_elements(&resolved);
                for element in &elems {
                    for face in element.faces.iter().flatten() {
                        tex_paths.insert(face.path.clone());
                    }
                }
                block_elems.insert(format!("{bare}#{variant}"), elems);
            }
        }
    }

    // Build the atlas: one tile per referenced texture + a white tile (index 0).
    let paths: Vec<String> = tex_paths.into_iter().collect();
    let tile_count = paths.len() as u32 + 1; // +1 white
    let grid = (tile_count as f64).sqrt().ceil() as u32;
    let dim = grid * TILE;
    let mut rgba = vec![0u8; (dim * dim * 4) as usize];

    // White tile at slot 0.
    blit_tile(&mut rgba, dim, 0, &[255u8; (TILE * TILE * 4) as usize]);
    let white_uv = slot_uv(0, grid);

    let mut tex_uv: HashMap<String, [f32; 4]> = HashMap::new();
    let mut missing_textures = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        let slot = i as u32 + 1;
        let tile = load_texture_tile(&mut archive, path).unwrap_or_else(|| {
            missing_textures.push(path.clone());
            [200u8; (TILE * TILE * 4) as usize]
        });
        blit_tile(&mut rgba, dim, slot, &tile);
        tex_uv.insert(path.clone(), slot_uv(slot, grid));
    }

    // Build per-block models.
    let mut blocks = HashMap::new();
    for (name, faces) in block_faces {
        let mut model_faces = [FaceTex {
            uv: white_uv,
            tint: [1.0, 1.0, 1.0],
        }; 6];
        for (i, face) in faces.iter().enumerate() {
            let (uv, tint) = match face {
                (Some(path), tinted) => {
                    let uv = tex_uv.get(path).copied().unwrap_or(white_uv);
                    let tint = if *tinted {
                        FOLIAGE_TINT
                    } else {
                        [1.0, 1.0, 1.0]
                    };
                    (uv, tint)
                }
                (None, _) => (white_uv, hash_color(&name)),
            };
            model_faces[i] = FaceTex { uv, tint };
        }
        blocks.insert(
            name,
            BlockModel {
                faces: model_faces,
                textured: true,
            },
        );
    }

    // Build non-cube element geometry, mapping each face's 0..16 uv into its
    // atlas tile.
    let mut non_cube: HashMap<String, Vec<ElementData>> = HashMap::new();
    for (name, elems) in block_elems {
        let data: Vec<ElementData> = elems
            .into_iter()
            .map(|e| ElementData {
                from: e.from,
                to: e.to,
                rotation: e.rotation,
                faces: e.faces.map(|of| {
                    of.map(|pf| {
                        let tile = tex_uv.get(&pf.path).copied().unwrap_or(white_uv);
                        ElementFace {
                            uv: sub_rect(tile, pf.uv16),
                            tint: if pf.tinted {
                                FOLIAGE_TINT
                            } else {
                                [1.0, 1.0, 1.0]
                            },
                            cull: pf.cull,
                            uv_rotation: pf.uv_rotation,
                        }
                    })
                }),
            })
            .collect();
        non_cube.insert(name, data);
    }

    let mut finished_state_models = HashMap::new();
    let mut state_cubes = HashSet::new();
    for (state, parts) in state_models {
        if parts.len() == 1
            && parts[0]
                .alternatives
                .iter()
                .all(|alternative| alternative.full_cube)
        {
            state_cubes.insert(state);
        }
        let parts = parts
            .into_iter()
            .map(|part| BlockStateModelPart {
                alternatives: part
                    .alternatives
                    .into_iter()
                    .map(|alternative| BlockStateModelAlternative {
                        elements: alternative
                            .elements
                            .into_iter()
                            .map(|element| ElementData {
                                from: element.from,
                                to: element.to,
                                rotation: element.rotation,
                                faces: element.faces.map(|face| {
                                    face.map(|face| {
                                        let tile =
                                            tex_uv.get(&face.path).copied().unwrap_or(white_uv);
                                        ElementFace {
                                            uv: sub_rect(tile, face.uv16),
                                            tint: if face.tinted {
                                                FOLIAGE_TINT
                                            } else {
                                                [1.0, 1.0, 1.0]
                                            },
                                            cull: face.cull,
                                            uv_rotation: face.uv_rotation,
                                        }
                                    })
                                }),
                            })
                            .collect(),
                        rotation: alternative.rotation,
                        weight: alternative.weight,
                        uvlock: alternative.uvlock,
                    })
                    .collect(),
            })
            .collect();
        finished_state_models.insert(state, parts);
    }

    let fallback = BlockModel {
        faces: [FaceTex {
            uv: white_uv,
            tint: [0.55, 0.55, 0.6],
        }; 6],
        textured: false,
    };

    Ok(Atlas {
        rgba,
        width: dim,
        height: dim,
        blocks,
        non_cube,
        state_models: finished_state_models,
        state_cubes,
        fallback,
        white_uv,
        assume_cube: false,
        unresolved,
        missing_textures,
    })
}

/// Resolves the ordered icon texture layers for an item. Generated models can
/// use multiple layers (for example potion contents and an overlay); block
/// items fall back to one representative face texture.
fn resolve_item_icon_layers<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    model_name: &str,
    cache: &mut HashMap<String, Option<Resolved>>,
) -> Vec<String> {
    let Some(resolved) = resolve(archive, model_name, cache) else {
        return Vec::new();
    };
    let mut layers = Vec::new();
    for index in 0..32 {
        let key = format!("layer{index}");
        if !resolved.textures.contains_key(&key) {
            break;
        }
        if let Some(texture) = model::resolve_texture(&resolved.textures, &format!("#{key}")) {
            layers.push(texture);
        }
    }
    if !layers.is_empty() {
        return layers;
    }
    // Block items inherit a block model; pick a sensible representative face.
    for key in [
        "up", "side", "all", "north", "particle", "end", "cross", "texture",
    ] {
        if resolved.textures.contains_key(key) {
            if let Some(p) = model::resolve_texture(&resolved.textures, &format!("#{key}")) {
                return vec![p];
            }
        }
    }
    resolved
        .textures
        .values()
        .find_map(|v| model::resolve_texture(&resolved.textures, v))
        .into_iter()
        .collect()
}

/// Loads flat **item icons** for `item_names` from `jar_path` and stitches them
/// into a single atlas. Items whose model/texture can't be found are skipped.
pub fn load_item_atlas(jar_path: &Path, item_names: &[String]) -> Result<ItemAtlas, AssetError> {
    let file = File::open(jar_path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))?;
    let mut cache: HashMap<String, Option<Resolved>> = HashMap::new();

    // Resolve each item to its icon texture path.
    let mut item_tex: HashMap<String, Vec<String>> = HashMap::new();
    let mut item_models: HashMap<String, (Vec<ParsedElem>, DisplayTransform)> = HashMap::new();
    let mut tex_paths: BTreeSet<String> = BTreeSet::new();
    let mut unresolved = Vec::new();
    for name in item_names {
        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
        if bare == "air" {
            continue;
        }
        let modern_model = read_client_item_model(&mut archive, name);
        if modern_model == Some(None) {
            // An explicit empty/context-only modern definition has no
            // context-free visual. Do not resurrect a removed legacy path.
            continue;
        }
        let model_name = modern_model
            .flatten()
            .unwrap_or_else(|| format!("minecraft:item/{bare}"));
        let mut resolved_any = false;
        let icon_layers = resolve_item_icon_layers(&mut archive, &model_name, &mut cache);
        if !icon_layers.is_empty() {
            resolved_any = true;
            tex_paths.extend(icon_layers.iter().cloned());
            item_tex.insert(bare.to_string(), icon_layers);
        }
        if let Some(resolved) = resolve(&mut archive, &model_name, &mut cache) {
            let elements = parse_elements(&resolved);
            if !elements.is_empty() {
                resolved_any = true;
                for element in &elements {
                    for face in element.faces.iter().flatten() {
                        tex_paths.insert(face.path.clone());
                    }
                }
                let ground = resolved.display.get("ground").copied().unwrap_or_default();
                item_models.insert(bare.to_string(), (elements, ground));
            }
        }
        if !resolved_any {
            unresolved.push(name.clone());
        }
    }

    // One tile per unique texture, laid out in a square grid.
    let paths: Vec<String> = tex_paths.into_iter().collect();
    // Each item icon gets its own composed tile after the source texture tiles.
    let tile_count = (paths.len() + item_tex.len()).max(1) as u32;
    let grid = (f64::from(tile_count)).sqrt().ceil() as u32;
    let dim = grid * TILE;
    let mut rgba = vec![0u8; (dim * dim * 4) as usize];
    let mut tex_uv: HashMap<String, [f32; 4]> = HashMap::new();
    let mut missing_textures = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        let slot = i as u32;
        let tile = load_texture_tile(&mut archive, path).unwrap_or_else(|| {
            missing_textures.push(path.clone());
            [200u8; (TILE * TILE * 4) as usize]
        });
        blit_tile(&mut rgba, dim, slot, &tile);
        tex_uv.insert(path.clone(), slot_uv(slot, grid));
    }

    let mut icons = HashMap::new();
    for (index, (item, layers)) in item_tex.into_iter().enumerate() {
        let mut composed = [0u8; (TILE * TILE * 4) as usize];
        for path in layers {
            if let Some(layer) = load_texture_tile(&mut archive, &path) {
                alpha_composite(&mut composed, &layer);
            }
        }
        let slot = paths.len() as u32 + index as u32;
        blit_tile(&mut rgba, dim, slot, &composed);
        icons.insert(item, slot_uv(slot, grid));
    }

    let models = item_models
        .into_iter()
        .map(|(name, (elements, ground))| {
            let elements = elements
                .into_iter()
                .map(|element| ElementData {
                    from: element.from,
                    to: element.to,
                    rotation: element.rotation,
                    faces: element.faces.map(|face| {
                        face.map(|face| {
                            let tile = tex_uv.get(&face.path).copied().unwrap_or([0.0; 4]);
                            ElementFace {
                                uv: sub_rect(tile, face.uv16),
                                tint: if face.tinted { FOLIAGE_TINT } else { [1.0; 3] },
                                cull: face.cull,
                                uv_rotation: face.uv_rotation,
                            }
                        })
                    }),
                })
                .collect();
            (name, ItemModel { elements, ground })
        })
        .collect();

    Ok(ItemAtlas {
        rgba,
        width: dim,
        height: dim,
        icons,
        models,
        unresolved,
        missing_textures,
    })
}

fn alpha_composite(destination: &mut [u8], source: &[u8]) {
    for (dst, src) in destination.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
        let source_alpha = u32::from(src[3]);
        let destination_alpha = u32::from(dst[3]);
        let output_alpha = source_alpha + (destination_alpha * (255 - source_alpha) + 127) / 255;
        if output_alpha == 0 {
            continue;
        }
        for channel in 0..3 {
            let source_premultiplied = u32::from(src[channel]) * source_alpha;
            let destination_premultiplied =
                u32::from(dst[channel]) * destination_alpha * (255 - source_alpha) / 255;
            dst[channel] =
                ((source_premultiplied + destination_premultiplied) / output_alpha).min(255) as u8;
        }
        dst[3] = output_alpha.min(255) as u8;
    }
}

/// Generic green for tinted faces (grass top, leaves). Real biome tint later.
const FOLIAGE_TINT: [f32; 3] = [0.45, 0.70, 0.33];

/// Per-face texture path + tinted flag for a full-cube model, or `None` if the
/// model has no full-cube element.
fn cube_faces(resolved: &Resolved) -> Option<[(Option<String>, bool); 6]> {
    let cube = resolved.elements.iter().find(|e| is_full_cube(e))?;
    let mut faces: [(Option<String>, bool); 6] = Default::default();
    for (i, fname) in FACE_NAMES.iter().enumerate() {
        if let Some(face) = cube.faces.get(*fname) {
            let path = model::resolve_texture(&resolved.textures, &face.texture);
            faces[i] = (path, face.tintindex.is_some());
        }
    }
    // Require at least the top + a side to call it a usable cube.
    if faces[2].0.is_some() || faces[0].0.is_some() {
        Some(faces)
    } else {
        None
    }
}

fn is_full_cube(e: &ElementJson) -> bool {
    e.from == [0.0, 0.0, 0.0] && e.to == [16.0, 16.0, 16.0]
}

/// Maps a model face name to a `crab-render` face index (+X,-X,+Y,-Y,+Z,-Z).
fn face_index(name: &str) -> Option<usize> {
    Some(match name {
        "east" => 0,
        "west" => 1,
        "up" => 2,
        "down" => 3,
        "south" => 4,
        "north" => 5,
        _ => return None,
    })
}

/// A parsed (pre-atlas) element face: texture path + 0..16 uv + tint + cull.
struct ParsedFace {
    path: String,
    uv16: [f32; 4],
    tinted: bool,
    cull: Option<u8>,
    uv_rotation: u8,
}

/// A parsed (pre-atlas) element.
struct ParsedElem {
    from: [f32; 3],
    to: [f32; 3],
    rotation: Option<ElementRotation>,
    faces: [Option<ParsedFace>; 6],
}

/// Parses a resolved model's elements into pre-atlas geometry.
fn parse_elements(resolved: &Resolved) -> Vec<ParsedElem> {
    resolved
        .elements
        .iter()
        .map(|e| {
            let mut faces: [Option<ParsedFace>; 6] = [None, None, None, None, None, None];
            for (fname, fj) in &e.faces {
                let Some(idx) = face_index(fname) else {
                    continue;
                };
                if let Some(path) = model::resolve_texture(&resolved.textures, &fj.texture) {
                    faces[idx] = Some(ParsedFace {
                        path,
                        uv16: fj.uv.unwrap_or_else(|| default_face_uv(e, idx)),
                        tinted: fj.tintindex.is_some(),
                        cull: fj.cullface.as_deref().and_then(face_index).map(|i| i as u8),
                        uv_rotation: fj.rotation.rem_euclid(360).div_euclid(90) as u8 % 4,
                    });
                }
            }
            let rotation = e.rotation.as_ref().map(|r| ElementRotation {
                origin: r.origin,
                axis: match r.axis.as_str() {
                    "x" => 0,
                    "z" => 2,
                    _ => 1,
                },
                angle: r.angle,
                rescale: r.rescale,
            });
            ParsedElem {
                from: e.from,
                to: e.to,
                rotation,
                faces,
            }
        })
        .collect()
}

/// Vanilla derives omitted face UVs from the element bounds. The mapping is
/// direction-specific because north/east faces view the cuboid from the
/// opposite axis direction.
fn default_face_uv(element: &ElementJson, face: usize) -> [f32; 4] {
    let [fx, fy, fz] = element.from;
    let [tx, ty, tz] = element.to;
    match face {
        0 => [16.0 - tz, 16.0 - ty, 16.0 - fz, 16.0 - fy], // east
        1 => [fz, 16.0 - ty, tz, 16.0 - fy],               // west
        2 => [fx, fz, tx, tz],                             // up
        3 => [fx, 16.0 - tz, tx, 16.0 - fz],               // down
        4 => [fx, 16.0 - ty, tx, 16.0 - fy],               // south
        5 => [16.0 - tx, 16.0 - ty, 16.0 - fx, 16.0 - fy], // north
        _ => [0.0, 0.0, 16.0, 16.0],
    }
}

/// Maps a face's 0..16 model uv into its atlas tile sub-rect.
fn sub_rect(tile: [f32; 4], uv16: [f32; 4]) -> [f32; 4] {
    let [au0, av0, au1, av1] = tile;
    let lx = |t: f32| au0 + (au1 - au0) * (t / 16.0);
    let ly = |t: f32| av0 + (av1 - av0) * (t / 16.0);
    [lx(uv16[0]), ly(uv16[1]), lx(uv16[2]), ly(uv16[3])]
}

/// Reads a 16x16 RGBA tile (top-left / first animation frame) for a texture ref.
fn load_texture_tile<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    tex_ref: &str,
) -> Option<[u8; (TILE * TILE * 4) as usize]> {
    let (namespace, bare) = split_resource_id(tex_ref);
    let path = format!("assets/{namespace}/textures/{bare}.png");
    let mut file = archive.by_name(&path).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut out = [0u8; (TILE * TILE * 4) as usize];
    for y in 0..TILE {
        for x in 0..TILE {
            // First frame: clamp into the source (handles 16x16 and animated 16xN).
            let sx = x.min(w.saturating_sub(1));
            let sy = y.min(h.saturating_sub(1));
            let p = img.get_pixel(sx, sy).0;
            let i = ((y * TILE + x) * 4) as usize;
            out[i..i + 4].copy_from_slice(&p);
        }
    }
    Some(out)
}

/// Loads the 10 block-breaking overlays (`block/destroy_stage_0..9`) from the
/// jar, stacked into one 16x160 RGBA atlas (stage `n` occupies rows
/// `n*16 .. n*16+16`, so its V range is `[n/10, (n+1)/10]`). Returns `None` if
/// none are present.
pub fn load_destroy_stages(jar_path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let file = File::open(jar_path).ok()?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).ok()?;
    let (w, h) = (TILE, TILE * 10);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let mut any = false;
    for stage in 0..10u32 {
        let Some(tile) = load_texture_tile(&mut archive, &format!("block/destroy_stage_{stage}"))
        else {
            continue;
        };
        any = true;
        let oy = stage * TILE;
        for y in 0..TILE {
            for x in 0..TILE {
                let src = ((y * TILE + x) * 4) as usize;
                let dst = (((oy + y) * w + x) * 4) as usize;
                rgba[dst..dst + 4].copy_from_slice(&tile[src..src + 4]);
            }
        }
    }
    any.then_some((rgba, w, h))
}

fn blit_tile(atlas: &mut [u8], dim: u32, slot: u32, tile: &[u8]) {
    let grid = dim / TILE;
    let (col, row) = (slot % grid, slot / grid);
    let (ox, oy) = (col * TILE, row * TILE);
    for y in 0..TILE {
        for x in 0..TILE {
            let src = ((y * TILE + x) * 4) as usize;
            let dst = (((oy + y) * dim + (ox + x)) * 4) as usize;
            atlas[dst..dst + 4].copy_from_slice(&tile[src..src + 4]);
        }
    }
}

fn slot_uv(slot: u32, grid: u32) -> [f32; 4] {
    let (col, row) = (slot % grid, slot / grid);
    let step = 1.0 / grid as f32;
    // Inset by a fraction of a texel to avoid bleeding with nearest sampling.
    let inset = step * 0.001;
    let u0 = col as f32 * step + inset;
    let v0 = row as f32 * step + inset;
    [u0, v0, u0 + step - 2.0 * inset, v0 + step - 2.0 * inset]
}

fn hash_color(name: &str) -> [f32; 3] {
    let mut h: u32 = 2_166_136_261;
    for b in name.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    [
        0.35 + ((h >> 16) & 0xff) as f32 / 255.0 * 0.5,
        0.35 + ((h >> 8) & 0xff) as f32 / 255.0 * 0.5,
        0.35 + (h & 0xff) as f32 / 255.0 * 0.5,
    ]
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn fluids_use_vanilla_texture_fallback_without_block_models() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "crabcraft-fluid-atlas-{}-{nonce}.jar",
            std::process::id()
        ));
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, color) in [
            ("water_still", [20, 40, 200, 160]),
            ("water_flow", [30, 60, 220, 160]),
        ] {
            writer
                .start_file(
                    format!("assets/minecraft/textures/block/{name}.png"),
                    options,
                )
                .unwrap();
            let image = image::RgbaImage::from_pixel(16, 16, image::Rgba(color));
            let mut png = std::io::Cursor::new(Vec::new());
            image.write_to(&mut png, image::ImageFormat::Png).unwrap();
            writer.write_all(png.get_ref()).unwrap();
        }
        writer.finish().unwrap();

        let atlas = load_block_atlas(&path, &["minecraft:water".to_string()]).unwrap();
        assert!(atlas.model("water").textured);
        assert!(atlas.unresolved_models().is_empty());
        assert!(atlas.missing_textures().is_empty());
        assert!(atlas
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel == [20, 40, 200, 160]));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn blockstate_conditions_support_vanilla_boolean_forms() {
        let properties = [("north", "true"), ("east", "false"), ("half", "top")];
        assert!(variant_matches("half=top,north=true", &properties));
        assert!(!variant_matches("half=bottom", &properties));
        assert!(property_condition_matches(
            &serde_json::json!({"north": "true|false", "half": "top"}),
            &properties,
        ));
        assert!(property_condition_matches(
            &serde_json::json!({"OR": [{"east": "true"}, {"half": "top"}]}),
            &properties,
        ));
        assert!(!property_condition_matches(
            &serde_json::json!({"AND": [{"north": "true"}, {"half": "bottom"}]}),
            &properties,
        ));
    }

    #[test]
    fn omitted_element_uvs_follow_vanilla_face_projection() {
        let element = ElementJson {
            from: [2.0, 3.0, 5.0],
            to: [11.0, 13.0, 15.0],
            rotation: None,
            faces: HashMap::new(),
        };
        assert_eq!(default_face_uv(&element, 0), [1.0, 3.0, 11.0, 13.0]);
        assert_eq!(default_face_uv(&element, 1), [5.0, 3.0, 15.0, 13.0]);
        assert_eq!(default_face_uv(&element, 2), [2.0, 5.0, 11.0, 15.0]);
        assert_eq!(default_face_uv(&element, 3), [2.0, 1.0, 11.0, 11.0]);
        assert_eq!(default_face_uv(&element, 4), [2.0, 3.0, 11.0, 13.0]);
        assert_eq!(default_face_uv(&element, 5), [5.0, 3.0, 14.0, 13.0]);
    }

    #[test]
    fn resolves_modern_item_definition_and_namespaced_assets() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "crabcraft-modern-item-{}-{nonce}.jar",
            std::process::id()
        ));
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("assets/minecraft/items/test_item.json", options)
            .unwrap();
        writer
            .write_all(br#"{"model":{"type":"minecraft:model","model":"test:item/actual"}}"#)
            .unwrap();
        writer
            .start_file("assets/test/models/item/actual.json", options)
            .unwrap();
        writer
            .write_all(
                br##"{"parent":"minecraft:item/generated","textures":{"layer0":"test:item/actual","layer1":"test:item/overlay"}}"##,
            )
            .unwrap();
        writer
            .start_file("assets/minecraft/models/item/generated.json", options)
            .unwrap();
        writer.write_all(br#"{}"#).unwrap();
        writer
            .start_file("assets/test/textures/item/actual.png", options)
            .unwrap();
        let image = image::RgbaImage::from_pixel(16, 16, image::Rgba([12, 34, 56, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        writer.write_all(png.get_ref()).unwrap();
        writer
            .start_file("assets/test/textures/item/overlay.png", options)
            .unwrap();
        let image = image::RgbaImage::from_pixel(16, 16, image::Rgba([200, 100, 50, 128]));
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        writer.write_all(png.get_ref()).unwrap();
        writer.finish().unwrap();

        let atlas = load_item_atlas(&path, &["minecraft:test_item".to_string()]).unwrap();
        assert_eq!(atlas.len(), 1);
        assert!(atlas.icon("test_item").is_some());
        assert!(atlas.unresolved_models().is_empty());
        assert!(atlas.missing_textures().is_empty());
        assert!(atlas
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel == [106, 67, 52, 255]));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn modern_item_definition_uses_context_free_fallback() {
        let definition = serde_json::json!({
            "type": "minecraft:condition",
            "property": "minecraft:using_item",
            "on_true": {"type": "minecraft:model", "model": "test:item/using"},
            "on_false": {
                "type": "minecraft:range_dispatch",
                "property": "minecraft:damage",
                "fallback": {"type": "minecraft:model", "model": "test:item/default"},
                "entries": []
            }
        });
        assert_eq!(
            default_plain_item_model(&definition, 0).as_deref(),
            Some("test:item/default")
        );
    }

    #[test]
    fn loads_registry_selected_blockstate_model() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "crabcraft-blockstate-{}-{nonce}.jar",
            std::process::id()
        ));
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("assets/minecraft/blockstates/oak_stairs.json", options)
            .unwrap();
        writer
            .write_all(
                br##"{
                    "variants": {
                        "": {"model":"minecraft:block/fallback"},
                        "facing=north,half=top,shape=straight,waterlogged=true": {
                            "model":"minecraft:block/test_stair",
                            "x":90,
                            "y":180,
                            "uvlock":true,
                            "weight":3
                        }
                    }
                }"##,
            )
            .unwrap();
        writer
            .start_file("assets/minecraft/models/block/test_stair.json", options)
            .unwrap();
        writer
            .write_all(
                br##"{
                    "textures":{"all":"minecraft:block/test"},
                    "elements":[{
                        "from":[0,0,0],
                        "to":[16,8,16],
                        "faces":{"up":{"texture":"#all","rotation":90}}
                    }]
                }"##,
            )
            .unwrap();
        writer
            .start_file("assets/minecraft/models/block/fallback.json", options)
            .unwrap();
        writer
            .write_all(
                br##"{
                    "textures":{"all":"minecraft:block/fallback"},
                    "elements":[{
                        "from":[0,0,0],
                        "to":[16,16,16],
                        "faces":{"up":{"texture":"#all"}}
                    }]
                }"##,
            )
            .unwrap();
        writer.finish().unwrap();

        let block = crab_registry::block_by_name("oak_stairs").unwrap();
        let atlas = load_block_atlas(&path, &["minecraft:oak_stairs".to_string()]).unwrap();
        let parts = atlas.block_state_model(block.min_state).unwrap();
        assert_eq!(parts.len(), 1);
        let alternative = &parts[0].alternatives[0];
        assert_eq!(alternative.rotation, [90.0, 180.0, 0.0]);
        assert_eq!(alternative.weight, 3);
        assert!(alternative.uvlock);
        assert_eq!(alternative.elements.len(), 1);
        assert_eq!(alternative.elements[0].to, [16.0, 8.0, 16.0]);
        assert_eq!(alternative.elements[0].faces[2].unwrap().uv_rotation, 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn loads_inherited_item_geometry_and_ground_transform() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "crabcraft-item-model-{}-{nonce}.jar",
            std::process::id()
        ));
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("assets/minecraft/models/item/test_block.json", options)
            .unwrap();
        writer
            .write_all(br#"{"parent":"minecraft:block/test_block"}"#)
            .unwrap();
        writer
            .start_file("assets/minecraft/models/block/test_block.json", options)
            .unwrap();
        writer
            .write_all(
                br##"{
                    "textures":{"all":"minecraft:block/test"},
                    "display":{"ground":{
                        "rotation":[10,20,30],
                        "translation":[1,2,3],
                        "scale":[0.25,0.5,0.75]
                    }},
                    "elements":[{
                        "from":[0,0,0],
                        "to":[16,8,16],
                        "faces":{"up":{"texture":"#all","rotation":180}}
                    }]
                }"##,
            )
            .unwrap();
        writer.finish().unwrap();

        let atlas = load_item_atlas(&path, &["minecraft:test_block".to_string()]).unwrap();
        let model = atlas.model("test_block").unwrap();
        assert_eq!(atlas.missing_textures(), &["minecraft:block/test"]);
        assert_eq!(model.ground.rotation, [10.0, 20.0, 30.0]);
        assert_eq!(model.ground.translation, [1.0, 2.0, 3.0]);
        assert_eq!(model.ground.scale, [0.25, 0.5, 0.75]);
        assert_eq!(model.elements.len(), 1);
        assert_eq!(model.elements[0].to, [16.0, 8.0, 16.0]);
        assert_eq!(model.elements[0].faces[2].unwrap().uv_rotation, 2);
        std::fs::remove_file(path).unwrap();
    }
}
