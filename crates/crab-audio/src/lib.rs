//! # crab-audio
//!
//! Loads Minecraft sounds from the launcher **asset store** (the shared
//! `assets/` dir of `indexes/<id>.json` + content-addressed `objects/`) and
//! plays them with `rodio`.
//!
//! Sounds are never bundled with Crabcraft — they come from the user's own
//! install, located via the `CRABCRAFT_ASSETS` directory at runtime. Playback
//! degrades gracefully to a no-op when no audio output device is available
//! (e.g. headless CI), while decoding still works for verification.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Errors loading the asset index.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad asset index: {0}")]
    Index(String),
}

/// Maps logical asset paths (e.g. `minecraft/sounds/dig/grass1.ogg`) to the
/// content hash naming the file under `objects/`.
pub struct AssetIndex {
    objects: HashMap<String, String>,
}

impl AssetIndex {
    /// Loads a launcher `assets/indexes/<id>.json` file.
    pub fn load(index_json: &Path) -> Result<Self, AudioError> {
        let text = std::fs::read_to_string(index_json)?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| AudioError::Index(e.to_string()))?;
        let objs = v
            .get("objects")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| AudioError::Index("missing `objects`".into()))?;
        let objects = objs
            .iter()
            .filter_map(|(k, val)| {
                let h = val.get("hash")?.as_str()?;
                Some((k.clone(), h.to_string()))
            })
            .collect();
        Ok(Self { objects })
    }

    /// Content hash for a logical asset path, if present.
    #[must_use]
    pub fn hash(&self, logical: &str) -> Option<&str> {
        self.objects.get(logical).map(String::as_str)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

/// Path of a content-addressed object: `objects/<hash[0:2]>/<hash>`.
#[must_use]
pub fn object_path(assets_dir: &Path, hash: &str) -> PathBuf {
    let prefix = &hash[..2.min(hash.len())];
    assets_dir.join("objects").join(prefix).join(hash)
}

/// Reads a sound's OGG bytes by logical name (e.g. `"dig/grass1"`).
#[must_use]
pub fn read_sound(assets_dir: &Path, index: &AssetIndex, name: &str) -> Option<Vec<u8>> {
    let logical = format!("minecraft/sounds/{name}.ogg");
    let hash = index.hash(&logical)?;
    std::fs::read(object_path(assets_dir, hash)).ok()
}

/// Decodes an OGG to its total sample count (verification; needs no device).
#[must_use]
pub fn ogg_sample_count(ogg: &[u8]) -> Option<usize> {
    let decoder = rodio::Decoder::new(Cursor::new(ogg.to_vec())).ok()?;
    Some(decoder.count())
}

/// The sound-material group for a block (`"grass"`, `"wood"`, `"sand"`,
/// `"gravel"`, or `"stone"`). A heuristic over names (vanilla uses a sound-type
/// table we don't carry yet).
#[must_use]
pub fn material(block_name: &str) -> &'static str {
    let n = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    let has = |kws: &[&str]| kws.iter().any(|k| n.contains(k));
    if has(&[
        "log",
        "planks",
        "wood",
        "fence",
        "sapling",
        "door",
        "_sign",
        "crafting_table",
        "bookshelf",
        "chest",
        "barrel",
        "ladder",
        "bamboo",
    ]) {
        "wood"
    } else if has(&[
        "grass", "leaves", "wool", "moss", "hay", "vine", "fern", "flower", "crop", "wheat",
        "carpet",
    ]) {
        "grass"
    } else if n.contains("sand") && !n.contains("sandstone") {
        "sand"
    } else if has(&[
        "gravel", "dirt", "clay", "soul", "mud", "podzol", "mycelium", "farmland", "path", "snow",
    ]) {
        "gravel"
    } else {
        "stone"
    }
}

/// The block-break / place (`dig`) sound name, e.g. `"dig/grass1"`.
#[must_use]
pub fn break_sound(block_name: &str) -> String {
    format!("dig/{}1", material(block_name))
}

/// The footstep (`step`) sound name for the block walked on, e.g. `"step/grass1"`.
#[must_use]
pub fn step_sound(block_name: &str) -> String {
    format!("step/{}1", material(block_name))
}

/// The player-hurt sound name.
#[must_use]
pub fn hurt_sound() -> &'static str {
    "damage/hit1"
}

/// Fire-and-forget OGG playback. A no-op (but still decode-checkable) when no
/// audio output device is available.
pub struct SoundPlayer {
    // Field order matters: the stream must outlive the handle.
    handle: Option<rodio::OutputStreamHandle>,
    _stream: Option<rodio::OutputStream>,
}

impl SoundPlayer {
    /// Opens the default audio output, or a silent player if none is available.
    #[must_use]
    pub fn new() -> Self {
        match rodio::OutputStream::try_default() {
            Ok((stream, handle)) => Self {
                handle: Some(handle),
                _stream: Some(stream),
            },
            Err(e) => {
                tracing::warn!("no audio output ({e}); sounds disabled");
                Self {
                    handle: None,
                    _stream: None,
                }
            }
        }
    }

    /// Whether a real output device is driving playback.
    #[must_use]
    pub fn available(&self) -> bool {
        self.handle.is_some()
    }

    /// Decodes and plays an OGG; returns whether it decoded successfully.
    /// (Plays only if an output device exists, but always validates decoding.)
    pub fn play_ogg(&self, ogg: Vec<u8>) -> bool {
        match &self.handle {
            Some(handle) => match rodio::Decoder::new(Cursor::new(ogg)) {
                Ok(source) => {
                    use rodio::Source;
                    let _ = handle.play_raw(source.convert_samples());
                    true
                }
                Err(_) => false,
            },
            None => rodio::Decoder::new(Cursor::new(ogg)).is_ok(),
        }
    }
}

impl Default for SoundPlayer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn break_sound_picks_material() {
        assert_eq!(break_sound("minecraft:grass_block"), "dig/grass1");
        assert_eq!(break_sound("minecraft:oak_log"), "dig/wood1");
        assert_eq!(break_sound("minecraft:oak_planks"), "dig/wood1");
        assert_eq!(break_sound("minecraft:sand"), "dig/sand1");
        assert_eq!(break_sound("minecraft:sandstone"), "dig/stone1");
        assert_eq!(break_sound("minecraft:gravel"), "dig/gravel1");
        assert_eq!(break_sound("minecraft:dirt"), "dig/gravel1");
        assert_eq!(break_sound("minecraft:stone"), "dig/stone1");
        assert_eq!(break_sound("minecraft:cobblestone"), "dig/stone1");
    }
}
