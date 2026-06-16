//! # crab-assets
//!
//! Loads Minecraft block **models** and **textures** straight from a client jar
//! (your own install — assets are never bundled with Crabcraft) and stitches a
//! texture atlas for the renderer.
//!
//! Scope: **full-cube** blocks resolve to per-face atlas textures. Blocks with
//! `elements` (slabs, stairs, plants, lanterns, …) resolve to element geometry
//! ([`ElementData`]) — their default model only, since blockstate variant /
//! multipart selection (fences, walls, rotated stairs) isn't parsed yet, so
//! those still fall back to a flat cube.
//!
//! Model resolution follows vanilla: walk the `parent` chain, merge `textures`
//! maps (child wins), take the deepest `elements`, then resolve each face's
//! `#variable` texture reference through the merged map.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::Deserialize;

mod model;
use model::{resolve, ElementJson, Resolved};

pub mod entity;
pub use entity::{
    load_entity_atlas, load_entity_texture, load_geometry, parse_geometry, player_geometry, Bone,
    Cube, EntityAtlas, EntityGeometry, EntityModelEntry,
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

/// A stitched texture atlas plus per-block face lookups.
pub struct Atlas {
    /// RGBA8 atlas pixels, row-major.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    blocks: HashMap<String, BlockModel>,
    non_cube: HashMap<String, Vec<ElementData>>,
    fallback: BlockModel,
    white_uv: [f32; 4],
    /// When true (the debug atlas), every block is treated as a full cube.
    assume_cube: bool,
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
            fallback: BlockModel {
                faces: [white; 6],
                textured: false,
            },
            white_uv: [0.0, 0.0, 1.0, 1.0],
            assume_cube: true,
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
}

impl ItemAtlas {
    /// Atlas UV `[u0, v0, u1, v1]` for an item, if an icon was resolved.
    #[must_use]
    pub fn icon(&self, item_name: &str) -> Option<[f32; 4]> {
        let bare = item_name.strip_prefix("minecraft:").unwrap_or(item_name);
        self.icons.get(bare).copied()
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

    /// An atlas with no icons (a single transparent tile). Lets the renderer
    /// run without a jar — the hotbar simply shows no item icons.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            rgba: vec![0u8; (TILE * TILE * 4) as usize],
            width: TILE,
            height: TILE,
            icons: HashMap::new(),
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
}

/// Reads + parses `assets/minecraft/models/<name>.json` from the jar.
pub(crate) fn read_model<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<ModelJson> {
    let bare = name.strip_prefix("minecraft:").unwrap_or(name);
    let path = format!("assets/minecraft/models/{bare}.json");
    let mut file = archive.by_name(&path).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

/// Loads block models + textures for `block_names` from `jar_path` and builds an
/// atlas. Blocks that aren't full cubes get a flat fallback colour.
pub fn load_block_atlas(jar_path: &Path, block_names: &[String]) -> Result<Atlas, AssetError> {
    let file = File::open(jar_path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))?;

    // Resolve each block to either full-cube faces or element geometry.
    let mut cache: HashMap<String, Option<Resolved>> = HashMap::new();
    let mut block_faces: HashMap<String, [(Option<String>, bool); 6]> = HashMap::new();
    let mut block_elems: HashMap<String, Vec<ParsedElem>> = HashMap::new();
    let mut tex_paths: BTreeSet<String> = BTreeSet::new();

    for name in block_names {
        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
        let model_name = format!("block/{bare}");
        let Some(resolved) = resolve(&mut archive, &model_name, &mut cache) else {
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
    for (i, path) in paths.iter().enumerate() {
        let slot = i as u32 + 1;
        let tile =
            load_texture_tile(&mut archive, path).unwrap_or([200u8; (TILE * TILE * 4) as usize]);
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
                        }
                    })
                }),
            })
            .collect();
        non_cube.insert(name, data);
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
        fallback,
        white_uv,
        assume_cube: false,
    })
}

/// Resolves the single icon texture path for an item: `layer0` for flat
/// (generated) items, else a representative face texture for block items.
fn resolve_item_icon<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    item_bare: &str,
    cache: &mut HashMap<String, Option<Resolved>>,
) -> Option<String> {
    let resolved = resolve(archive, &format!("item/{item_bare}"), cache)?;
    if resolved.textures.contains_key("layer0") {
        return model::resolve_texture(&resolved.textures, "#layer0");
    }
    // Block items inherit a block model; pick a sensible representative face.
    for key in [
        "up", "side", "all", "north", "particle", "end", "cross", "texture",
    ] {
        if resolved.textures.contains_key(key) {
            if let Some(p) = model::resolve_texture(&resolved.textures, &format!("#{key}")) {
                return Some(p);
            }
        }
    }
    resolved
        .textures
        .values()
        .find_map(|v| model::resolve_texture(&resolved.textures, v))
}

/// Loads flat **item icons** for `item_names` from `jar_path` and stitches them
/// into a single atlas. Items whose model/texture can't be found are skipped.
pub fn load_item_atlas(jar_path: &Path, item_names: &[String]) -> Result<ItemAtlas, AssetError> {
    let file = File::open(jar_path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))?;
    let mut cache: HashMap<String, Option<Resolved>> = HashMap::new();

    // Resolve each item to its icon texture path.
    let mut item_tex: HashMap<String, String> = HashMap::new();
    let mut tex_paths: BTreeSet<String> = BTreeSet::new();
    for name in item_names {
        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
        if bare == "air" {
            continue;
        }
        if let Some(path) = resolve_item_icon(&mut archive, bare, &mut cache) {
            tex_paths.insert(path.clone());
            item_tex.insert(bare.to_string(), path);
        }
    }

    // One tile per unique texture, laid out in a square grid.
    let paths: Vec<String> = tex_paths.into_iter().collect();
    let tile_count = paths.len().max(1) as u32;
    let grid = (f64::from(tile_count)).sqrt().ceil() as u32;
    let dim = grid * TILE;
    let mut rgba = vec![0u8; (dim * dim * 4) as usize];
    let mut tex_uv: HashMap<String, [f32; 4]> = HashMap::new();
    for (i, path) in paths.iter().enumerate() {
        let slot = i as u32;
        let tile =
            load_texture_tile(&mut archive, path).unwrap_or([200u8; (TILE * TILE * 4) as usize]);
        blit_tile(&mut rgba, dim, slot, &tile);
        tex_uv.insert(path.clone(), slot_uv(slot, grid));
    }

    let icons = item_tex
        .into_iter()
        .filter_map(|(item, path)| tex_uv.get(&path).map(|uv| (item, *uv)))
        .collect();

    Ok(ItemAtlas {
        rgba,
        width: dim,
        height: dim,
        icons,
    })
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
                        uv16: fj.uv.unwrap_or([0.0, 0.0, 16.0, 16.0]),
                        tinted: fj.tintindex.is_some(),
                        cull: fj.cullface.as_deref().and_then(face_index).map(|i| i as u8),
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
    let bare = tex_ref.strip_prefix("minecraft:").unwrap_or(tex_ref);
    let path = format!("assets/minecraft/textures/{bare}.png");
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
