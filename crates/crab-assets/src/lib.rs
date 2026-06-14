//! # crab-assets
//!
//! Loads Minecraft block **models** and **textures** straight from a client jar
//! (your own install — assets are never bundled with Crabcraft) and stitches a
//! texture atlas for the renderer.
//!
//! Scope: the common **full-cube** blocks (stone, dirt, grass, logs, planks,
//! ores, wool, …) are resolved to per-face atlas textures. Non-cube models
//! (stairs, slabs, fences, plants, glass panes, fluids) fall back to a flat
//! per-block colour. This covers the vast majority of what terrain looks like;
//! full arbitrary-model support is a later refinement.
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

/// A stitched texture atlas plus per-block face lookups.
pub struct Atlas {
    /// RGBA8 atlas pixels, row-major.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    blocks: HashMap<String, BlockModel>,
    fallback: BlockModel,
}

impl Atlas {
    /// Model for a (namespaced or bare) block name, falling back to a flat tile.
    pub fn model(&self, block_name: &str) -> &BlockModel {
        let bare = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
        self.blocks.get(bare).unwrap_or(&self.fallback)
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
            fallback: BlockModel {
                faces: [white; 6],
                textured: false,
            },
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

    // Resolve each block's six faces to (texture path, tinted?).
    let mut cache: HashMap<String, Option<Resolved>> = HashMap::new();
    let mut block_faces: HashMap<String, [(Option<String>, bool); 6]> = HashMap::new();
    let mut tex_paths: BTreeSet<String> = BTreeSet::new();

    for name in block_names {
        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
        let model_name = format!("block/{bare}");
        if let Some(faces) = resolve_block_faces(&mut archive, &model_name, &mut cache) {
            for (path, _tinted) in &faces {
                if let Some(path) = path {
                    tex_paths.insert(path.clone());
                }
            }
            block_faces.insert(bare.to_string(), faces);
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
        fallback,
    })
}

/// Generic green for tinted faces (grass top, leaves). Real biome tint later.
const FOLIAGE_TINT: [f32; 3] = [0.45, 0.70, 0.33];

fn resolve_block_faces<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    model_name: &str,
    cache: &mut HashMap<String, Option<Resolved>>,
) -> Option<[(Option<String>, bool); 6]> {
    let resolved = resolve(archive, model_name, cache)?;
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
