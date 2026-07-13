//! GUI atlas: stitches the vanilla `widgets`/`inventory` GUI sprites and the
//! `ascii` bitmap font from the client jar into one texture, with named sprite
//! rects and per-glyph metrics. Loaded from the user's jar at runtime (never
//! bundled).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::AssetError;

const ATLAS_W: u32 = 1024;
const ATLAS_H: u32 = 1280;
// Placement of each source image within the atlas.
const WIDGETS_AT: (u32, u32) = (0, 0);
const INVENTORY_AT: (u32, u32) = (256, 0);
const FONT_AT: (u32, u32) = (0, 256);
const ICONS_AT: (u32, u32) = (256, 256);
const GENERIC_CONTAINER_AT: (u32, u32) = (0, 384);
const FURNACE_AT: (u32, u32) = (256, 384);
const BLAST_FURNACE_AT: (u32, u32) = (0, 640);
const SMOKER_AT: (u32, u32) = (256, 640);
const DISPENSER_AT: (u32, u32) = (512, 0);
const HOPPER_AT: (u32, u32) = (512, 256);
const BREWING_STAND_AT: (u32, u32) = (512, 512);
const CRAFTING_TABLE_AT: (u32, u32) = (768, 0);
const ENCHANTING_TABLE_AT: (u32, u32) = (768, 256);
const ANVIL_AT: (u32, u32) = (768, 512);
const GRINDSTONE_AT: (u32, u32) = (768, 768);
const SMITHING_AT: (u32, u32) = (512, 768);
const CARTOGRAPHY_AT: (u32, u32) = (256, 896);
const EFFECTS_AT: (u32, u32) = (0, 1024);
const LOOM_AT: (u32, u32) = (512, 1024);
const STONECUTTER_AT: (u32, u32) = (768, 1024);
const EFFECT_NAMES: [&str; 33] = [
    "speed",
    "slowness",
    "haste",
    "mining_fatigue",
    "strength",
    "instant_health",
    "instant_damage",
    "jump_boost",
    "nausea",
    "regeneration",
    "resistance",
    "fire_resistance",
    "water_breathing",
    "invisibility",
    "blindness",
    "night_vision",
    "hunger",
    "weakness",
    "poison",
    "wither",
    "health_boost",
    "absorption",
    "saturation",
    "glowing",
    "levitation",
    "luck",
    "unluck",
    "slow_falling",
    "conduit_power",
    "dolphins_grace",
    "bad_omen",
    "hero_of_the_village",
    "darkness",
];

/// One bitmap-font glyph: its atlas UV, drawn pixel width, and pixel advance.
#[derive(Clone, Copy, Debug, Default)]
pub struct Glyph {
    pub uv: [f32; 4],
    pub width: f32,
    pub advance: f32,
}

/// Stitched GUI sprites + bitmap font.
pub struct GuiAtlas {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    sprites: HashMap<&'static str, [f32; 4]>,
    glyphs: Vec<Glyph>,
    /// Whether real textures loaded (vs the empty fallback).
    pub loaded: bool,
}

impl GuiAtlas {
    /// Atlas UV `[u0,v0,u1,v1]` for a named sprite (`"hotbar"`, `"selection"`,
    /// `"inventory"`).
    #[must_use]
    pub fn sprite(&self, name: &str) -> Option<[f32; 4]> {
        self.sprites.get(name).copied()
    }

    /// Glyph metrics for a character (ASCII; `?` for anything out of range).
    #[must_use]
    pub fn glyph(&self, ch: char) -> Glyph {
        let c = ch as usize;
        self.glyphs
            .get(c)
            .copied()
            .unwrap_or_else(|| self.glyphs[b'?' as usize])
    }

    /// Pixel width a string would occupy at 1px scale.
    #[must_use]
    pub fn text_width(&self, text: &str) -> f32 {
        text.chars().map(|c| self.glyph(c).advance).sum()
    }

    /// An empty atlas (no sprites, blank glyphs) for the no-jar case.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            rgba: vec![0; 4],
            width: 1,
            height: 1,
            sprites: HashMap::new(),
            glyphs: vec![Glyph::default(); 256],
            loaded: false,
        }
    }
}

fn read_png<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    path: &str,
) -> Option<(Vec<u8>, u32, u32)> {
    let mut entry = archive.by_name(path).ok()?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some((img.into_raw(), w, h))
}

fn blit(atlas: &mut [u8], src: &[u8], sw: u32, sh: u32, at: (u32, u32)) {
    for y in 0..sh.min(ATLAS_H - at.1) {
        for x in 0..sw.min(ATLAS_W - at.0) {
            let s = ((y * sw + x) * 4) as usize;
            let d = (((at.1 + y) * ATLAS_W + at.0 + x) * 4) as usize;
            atlas[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
}

/// Normalised atlas UV for a pixel rect.
fn uv(x: u32, y: u32, w: u32, h: u32) -> [f32; 4] {
    [
        x as f32 / ATLAS_W as f32,
        y as f32 / ATLAS_H as f32,
        (x + w) as f32 / ATLAS_W as f32,
        (y + h) as f32 / ATLAS_H as f32,
    ]
}

/// Loads + stitches the GUI sprites and font from `jar_path`.
pub fn load_gui_atlas(jar_path: &Path) -> Result<GuiAtlas, AssetError> {
    let file = File::open(jar_path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))?;
    let mut rgba = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
    let mut sprites: HashMap<&'static str, [f32; 4]> = HashMap::new();

    if let Some((src, w, h)) = read_png(&mut archive, "assets/minecraft/textures/gui/widgets.png") {
        blit(&mut rgba, &src, w, h, WIDGETS_AT);
        sprites.insert("hotbar", uv(WIDGETS_AT.0, WIDGETS_AT.1, 182, 22));
        sprites.insert("selection", uv(WIDGETS_AT.0, WIDGETS_AT.1 + 22, 24, 24));
        sprites.insert("button", uv(WIDGETS_AT.0, WIDGETS_AT.1 + 66, 200, 20));
        sprites.insert("button_hover", uv(WIDGETS_AT.0, WIDGETS_AT.1 + 86, 200, 20));
    }
    if let Some((src, w, h)) = read_png(
        &mut archive,
        "assets/minecraft/textures/gui/container/inventory.png",
    ) {
        blit(&mut rgba, &src, w, h, INVENTORY_AT);
        sprites.insert("inventory", uv(INVENTORY_AT.0, INVENTORY_AT.1, 176, 166));
    }
    if let Some((src, w, h)) = read_png(
        &mut archive,
        "assets/minecraft/textures/gui/container/generic_54.png",
    ) {
        blit(&mut rgba, &src, w, h, GENERIC_CONTAINER_AT);
        sprites.insert(
            "generic_54",
            uv(GENERIC_CONTAINER_AT.0, GENERIC_CONTAINER_AT.1, 176, 222),
        );
    }
    for (path, name, at) in [
        (
            "assets/minecraft/textures/gui/container/furnace.png",
            "furnace",
            FURNACE_AT,
        ),
        (
            "assets/minecraft/textures/gui/container/blast_furnace.png",
            "blast_furnace",
            BLAST_FURNACE_AT,
        ),
        (
            "assets/minecraft/textures/gui/container/smoker.png",
            "smoker",
            SMOKER_AT,
        ),
    ] {
        if let Some((src, w, h)) = read_png(&mut archive, path) {
            blit(&mut rgba, &src, w, h, at);
            sprites.insert(name, uv(at.0, at.1, 176, 166));
            let full_name = match name {
                "furnace" => "furnace_full",
                "blast_furnace" => "blast_furnace_full",
                _ => "smoker_full",
            };
            sprites.insert(full_name, uv(at.0, at.1, 256, 256));
        }
    }
    for (path, name, at, width, height) in [
        (
            "assets/minecraft/textures/gui/container/dispenser.png",
            "dispenser",
            DISPENSER_AT,
            176,
            166,
        ),
        (
            "assets/minecraft/textures/gui/container/hopper.png",
            "hopper",
            HOPPER_AT,
            176,
            133,
        ),
        (
            "assets/minecraft/textures/gui/container/brewing_stand.png",
            "brewing_stand",
            BREWING_STAND_AT,
            176,
            166,
        ),
        (
            "assets/minecraft/textures/gui/container/crafting_table.png",
            "crafting_table",
            CRAFTING_TABLE_AT,
            176,
            166,
        ),
        (
            "assets/minecraft/textures/gui/container/enchanting_table.png",
            "enchanting_table",
            ENCHANTING_TABLE_AT,
            176,
            166,
        ),
        (
            "assets/minecraft/textures/gui/container/anvil.png",
            "anvil",
            ANVIL_AT,
            176,
            166,
        ),
        (
            "assets/minecraft/textures/gui/container/grindstone.png",
            "grindstone",
            GRINDSTONE_AT,
            176,
            166,
        ),
        (
            "assets/minecraft/textures/gui/container/smithing.png",
            "smithing",
            SMITHING_AT,
            176,
            166,
        ),
        (
            "assets/minecraft/textures/gui/container/cartography_table.png",
            "cartography_table",
            CARTOGRAPHY_AT,
            176,
            166,
        ),
        (
            "assets/minecraft/textures/gui/container/loom.png",
            "loom",
            LOOM_AT,
            176,
            166,
        ),
        (
            "assets/minecraft/textures/gui/container/stonecutter.png",
            "stonecutter",
            STONECUTTER_AT,
            176,
            166,
        ),
    ] {
        if let Some((src, w, h)) = read_png(&mut archive, path) {
            blit(&mut rgba, &src, w, h, at);
            sprites.insert(name, uv(at.0, at.1, width, height));
        }
    }
    if let Some((src, w, h)) = read_png(&mut archive, "assets/minecraft/textures/gui/icons.png") {
        blit(&mut rgba, &src, w, h, ICONS_AT);
        let (ix, iy) = ICONS_AT;
        // Status-bar icons (classic icons.png layout): 9x9 each; xp bar 182x5.
        sprites.insert("heart_bg", uv(ix + 16, iy, 9, 9));
        sprites.insert("heart_full", uv(ix + 52, iy, 9, 9));
        sprites.insert("heart_half", uv(ix + 61, iy, 9, 9));
        sprites.insert("food_bg", uv(ix + 16, iy + 27, 9, 9));
        sprites.insert("food_full", uv(ix + 52, iy + 27, 9, 9));
        sprites.insert("food_half", uv(ix + 61, iy + 27, 9, 9));
        sprites.insert("xp_bg", uv(ix, iy + 64, 182, 5));
        sprites.insert("xp_full", uv(ix, iy + 69, 182, 5));
    }
    for (index, &name) in EFFECT_NAMES.iter().enumerate() {
        let path = format!("assets/minecraft/textures/mob_effect/{name}.png");
        if let Some((src, w, h)) = read_png(&mut archive, &path) {
            let at = (
                EFFECTS_AT.0 + (index as u32 % 16) * 18,
                EFFECTS_AT.1 + (index as u32 / 16) * 18,
            );
            blit(&mut rgba, &src, w, h, at);
            sprites.insert(name, uv(at.0, at.1, w.min(18), h.min(18)));
        }
    }

    let mut glyphs = vec![Glyph::default(); 256];
    let mut loaded = !sprites.is_empty();
    if let Some((font, fw, fh)) = read_png(&mut archive, "assets/minecraft/textures/font/ascii.png")
    {
        blit(&mut rgba, &font, fw, fh, FONT_AT);
        loaded = true;
        let cell = fw / 16; // 8 for a 128px sheet
        for c in 0..256u32 {
            let (cx, cy) = (c % 16 * cell, c / 16 * cell);
            // Trim to the rightmost opaque column.
            let mut width = 0u32;
            for col in 0..cell {
                for row in 0..cell {
                    let i = (((cy + row) * fw + cx + col) * 4 + 3) as usize;
                    if font.get(i).copied().unwrap_or(0) > 0 {
                        width = col + 1;
                    }
                }
            }
            let advance = if width == 0 { 4.0 } else { (width + 1) as f32 };
            glyphs[c as usize] = Glyph {
                uv: uv(FONT_AT.0 + cx, FONT_AT.1 + cy, width.max(1), cell),
                width: width as f32,
                advance,
            };
        }
    }

    Ok(GuiAtlas {
        rgba,
        width: ATLAS_W,
        height: ATLAS_H,
        sprites,
        glyphs,
        loaded,
    })
}
