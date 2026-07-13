//! Live windowed renderer: a winit window + wgpu surface that draws the world
//! from [`Shared`] as a first-person player, with cached per-chunk meshes that
//! rebuild only when a chunk changes (drained from `Shared::dirty_chunks`).
//!
//! NOTE: this needs a display to actually run; it is compile-verified here but
//! not click-tested in the headless build environment.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crab_assets::{Atlas, EntityAtlas, GuiAtlas, ItemAtlas};
use crab_protocol::versions::v1_20_1::play::PlaceRecipe;
use crab_render::{
    block_item_mesh, box_mesh, build_block_pipeline, build_hud_pipelines, container_geometry,
    entity_mesh, entity_mesh_with_pose, furnace_geometry, hud_geometry, inventory_geometry,
    mesh_region, simple_container_geometry, status_effect_geometry, upload_atlas, upload_texture,
    CameraUniform, HudPipelines, Vertex, DEPTH_FORMAT,
};
use glam::Vec3;
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

use crate::client::{ContainerState, CraftingRecipe, EnvironmentState, Shared};

/// Max chunk columns re-meshed per frame (bounds per-frame CPU during loads).
const REMESH_BUDGET: usize = 4;
const LOOK_SPEED: f32 = 110.0; // degrees/sec (arrow-key look)
const EYE_HEIGHT: f32 = 1.62;
type HudRect = (f32, f32, f32, f32);
type RecipeBookGeometry = (Vec<[f32; 5]>, Vec<[f32; 4]>, Vec<[f32; 4]>);

#[derive(Clone, Copy)]
struct RecipeContext {
    panel: HudRect,
    window_id: i8,
    grid: usize,
}
/// Pause-menu button labels (index 0 resumes; the rest match `menu_click`).
const MENU_BUTTONS: [&str; 4] = ["Back to Game", "Options...", "Controls...", "Quit"];
const DONE_BUTTON: [&str; 1] = ["Done"];
const FOV_CHOICES: [f32; 4] = [60.0, 70.0, 90.0, 110.0];
const SENSITIVITY_CHOICES: [f32; 5] = [0.06, 0.09, 0.12, 0.18, 0.24];
const CONTROL_HELP: [&str; 9] = [
    "W A S D    Move",
    "Space    Jump / double-tap flight",
    "Control    Sprint       Shift    Sneak",
    "Mouse    Look / attack / use",
    "1-9 or wheel    Select hotbar",
    "F    Swap main hand and offhand",
    "E    Inventory       T or /    Chat",
    "Tab    Player list       F3    Debug",
    "F5    Cycle camera perspective",
];

fn next_choice(current: f32, choices: &[f32]) -> f32 {
    let index = choices
        .iter()
        .position(|choice| (*choice - current).abs() < f32::EPSILON)
        .unwrap_or(0);
    choices[(index + 1) % choices.len()]
}

/// Approximates vanilla's day/night and storm sky brightness from server state.
fn sky_color(environment: EnvironmentState) -> [f64; 3] {
    let ticks = environment.time_of_day.unsigned_abs() % 24_000;
    let angle = ((ticks as f64 - 6_000.0) / 24_000.0) * std::f64::consts::TAU;
    let daylight = (angle.cos() * 0.5 + 0.5).clamp(0.0, 1.0);
    let storm = (1.0
        - f64::from(environment.rain_level) * 0.35
        - f64::from(environment.thunder_level) * 0.35)
        .clamp(0.25, 1.0);
    let night = [0.015, 0.02, 0.06];
    let day = [0.45, 0.62, 0.92];
    std::array::from_fn(|i| (night[i] + (day[i] - night[i]) * daylight) * storm)
}

fn celestial_mesh(eye: Vec3, environment: EnvironmentState, white_uv: [f32; 4]) -> Vec<Vertex> {
    let ticks = environment.time_of_day.unsigned_abs() % 24_000;
    let angle = ((ticks as f32 - 6_000.0) / 24_000.0) * std::f32::consts::TAU;
    let direction = Vec3::new(0.15, angle.cos(), angle.sin()).normalize();
    let mut vertices = Vec::new();
    for (direction, size, color) in [
        (direction, 24.0, [1.0, 0.88, 0.35]),
        (-direction, 18.0, [0.58, 0.68, 0.9]),
    ] {
        let center = eye + direction * 600.0;
        let half = size / 2.0;
        vertices.extend(box_mesh(
            [center.x - half, center.y - half, center.z - 1.0],
            [center.x + half, center.y + half, center.z + 1.0],
            white_uv,
            color,
        ));
    }
    if direction.y < 0.0 {
        for index in 0..72u32 {
            let hash = index.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
            let azimuth = (hash & 0xffff) as f32 / 65_535.0 * std::f32::consts::TAU;
            let elevation = 0.15 + ((hash >> 16) & 0xff) as f32 / 255.0 * 1.25;
            let star_direction = Vec3::new(
                azimuth.cos() * elevation.cos(),
                elevation.sin(),
                azimuth.sin() * elevation.cos(),
            );
            let center = eye + star_direction * 780.0;
            let size = 0.45 + ((hash >> 24) & 0x03) as f32 * 0.16;
            vertices.extend(box_mesh(
                [center.x - size, center.y - size, center.z - size],
                [center.x + size, center.y + size, center.z + size],
                white_uv,
                [4.0, 4.0, 4.5],
            ));
        }
    }
    let cloud_shift = environment.world_age as f32 * 0.03;
    let cloud_color = {
        let storm = 1.0
            - environment.rain_level.clamp(0.0, 1.0) * 0.45
            - environment.thunder_level.clamp(0.0, 1.0) * 0.3;
        [storm, storm, (storm + 0.05).min(1.0)]
    };
    let anchor_x = (eye.x / 32.0).floor() * 32.0 + cloud_shift.rem_euclid(32.0);
    let anchor_z = (eye.z / 32.0).floor() * 32.0;
    for gx in -4..=4 {
        for gz in -4..=4 {
            let hash = (gx * 73 + gz * 151) as f32;
            if (hash.sin() * 10.0).fract().abs() < 0.32 {
                continue;
            }
            let x = anchor_x + gx as f32 * 32.0;
            let z = anchor_z + gz as f32 * 32.0;
            let y = 128.0 + (hash * 0.17).sin() * 2.0;
            vertices.extend(box_mesh(
                [x - 14.0, y, z - 7.0],
                [x + 14.0, y + 2.0, z + 7.0],
                white_uv,
                cloud_color,
            ));
        }
    }
    vertices
}

/// First-person camera: eye at the player's head, looking along yaw/pitch
/// (Minecraft convention, degrees). Position comes from the player; this just
/// holds the look angles.
fn first_person_camera(
    player_pos: Vec3,
    yaw_deg: f32,
    pitch_deg: f32,
    aspect: f32,
    eye_height: f32,
    fov_degrees: f32,
    perspective: Perspective,
) -> crab_render::Camera {
    let player_eye = player_pos + Vec3::new(0.0, eye_height, 0.0);
    let (yaw, pitch) = (yaw_deg.to_radians(), pitch_deg.to_radians());
    let dir = Vec3::new(
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    );
    let (eye, target) = match perspective {
        Perspective::FirstPerson => (player_eye, player_eye + dir),
        Perspective::ThirdPersonBack => (player_eye - dir * 4.0, player_eye),
        Perspective::ThirdPersonFront => (player_eye + dir * 4.0, player_eye),
    };
    crab_render::Camera {
        eye,
        target,
        up: Vec3::Y,
        aspect,
        fovy_radians: fov_degrees.to_radians(),
        znear: 0.1,
        zfar: 1000.0,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Perspective {
    #[default]
    FirstPerson,
    ThirdPersonBack,
    ThirdPersonFront,
}

impl Perspective {
    fn next(self) -> Self {
        match self {
            Self::FirstPerson => Self::ThirdPersonBack,
            Self::ThirdPersonBack => Self::ThirdPersonFront,
            Self::ThirdPersonFront => Self::FirstPerson,
        }
    }
}

/// A camera-facing quad (billboard) of an item icon at `pos`, for dropped items.
fn push_item_billboard(out: &mut Vec<Vertex>, pos: [f32; 3], uv: [f32; 4], yaw_deg: f32) {
    let s = 0.2;
    let yr = yaw_deg.to_radians();
    let r = [yr.cos() * s, 0.0, yr.sin() * s];
    let up = [0.0, s, 0.0];
    let c = [pos[0], pos[1] + 0.25, pos[2]];
    let n = [yr.sin(), 0.4, -yr.cos()];
    let [u0, v0, u1, v1] = uv;
    let corner = |sx: f32, sy: f32, u, v| {
        (
            [
                c[0] + sx * r[0] + sy * up[0],
                c[1] + sy * up[1],
                c[2] + sx * r[2] + sy * up[2],
            ],
            [u, v],
        )
    };
    let tl = corner(-1.0, 1.0, u0, v0);
    let tr = corner(1.0, 1.0, u1, v0);
    let br = corner(1.0, -1.0, u1, v1);
    let bl = corner(-1.0, -1.0, u0, v1);
    for (p, t) in [tl, tr, br, tl, br, bl] {
        out.push(Vertex {
            position: p,
            normal: n,
            uv: t,
            tint: [1.0, 1.0, 1.0],
            opacity: 1.0,
        });
    }
}

fn box_color(type_id: i32) -> [f32; 3] {
    let h = (type_id as u32).wrapping_mul(2_654_435_761);
    [
        0.4 + ((h >> 16) & 0xff) as f32 / 255.0 * 0.5,
        0.4 + ((h >> 8) & 0xff) as f32 / 255.0 * 0.5,
        0.4 + (h & 0xff) as f32 / 255.0 * 0.5,
    ]
}

fn armour_color(item_name: &str) -> [f32; 3] {
    if item_name.starts_with("leather_") {
        [0.48, 0.28, 0.16]
    } else if item_name.starts_with("golden_") {
        [0.95, 0.72, 0.12]
    } else if item_name.starts_with("diamond_") {
        [0.18, 0.82, 0.82]
    } else if item_name.starts_with("netherite_") {
        [0.24, 0.2, 0.25]
    } else if item_name.starts_with("chainmail_") {
        [0.48, 0.52, 0.55]
    } else if item_name == "turtle_helmet" {
        [0.2, 0.62, 0.28]
    } else {
        [0.72, 0.74, 0.76]
    }
}

fn pretty_item_name(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn bundle_items(
    metadata: Option<&crab_protocol::nbt::Nbt>,
) -> Vec<crab_protocol::versions::v1_20_1::play::SlotItem> {
    use crab_protocol::nbt::Nbt;

    let Some(Nbt::List(entries)) = metadata.and_then(|value| value.get("bundle_contents")) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let Nbt::Int(item_id) = entry.get("id")? else {
                return None;
            };
            let Nbt::Byte(count) = entry.get("count")? else {
                return None;
            };
            Some(crab_protocol::versions::v1_20_1::play::SlotItem {
                item_id: *item_id,
                count: *count,
            })
        })
        .collect()
}

/// Projects nearby sign block-entity text into the HUD font pass. The block
/// model remains world geometry while the glyphs stay crisp at useful reading
/// distances, matching the server-provided four-line front/back content.
fn sign_text_geometry(
    shared: &Shared,
    gui: &GuiAtlas,
    camera: &crab_render::Camera,
    aspect: f32,
) -> Vec<[f32; 4]> {
    let matrix = camera.view_proj();
    let mut out = Vec::new();
    for (&(x, y, z), sign) in shared.signs.lock().unwrap().iter() {
        let center = Vec3::new(x as f32 + 0.5, y as f32 + 0.56, z as f32 + 0.5);
        let distance = camera.eye.distance(center);
        if distance > 32.0 {
            continue;
        }
        let clip = matrix * center.extend(1.0);
        if clip.w <= 0.0 {
            continue;
        }
        let ndc = clip.truncate() / clip.w;
        if !(0.0..=1.0).contains(&ndc.z) || ndc.x.abs() > 1.15 || ndc.y.abs() > 1.15 {
            continue;
        }
        let lines = if sign.front.iter().any(|line| !line.is_empty()) {
            &sign.front
        } else {
            &sign.back
        };
        let height = (0.22 / distance.max(1.0)).clamp(0.014, 0.052);
        for (index, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let display: String = line.chars().take(90).collect();
            let width = gui.text_width(&display) * (height / 8.0) / aspect.max(0.01);
            let line_y = ndc.y + height * (1.8 - index as f32);
            crab_render::push_text(
                &mut out,
                gui,
                &display,
                ndc.x - width / 2.0,
                line_y,
                height,
                aspect,
            );
        }
    }
    out
}

fn book_button_rect(aspect: f32, index: usize) -> (f32, f32, f32, f32) {
    let half_width = 0.38 / aspect.max(0.01);
    match index {
        0 => (
            -half_width,
            -0.7,
            -half_width + 0.18 / aspect.max(0.01),
            -0.58,
        ),
        1 => (
            half_width - 0.18 / aspect.max(0.01),
            -0.7,
            half_width,
            -0.58,
        ),
        2 | 4 => (
            -0.39 / aspect.max(0.01),
            -0.88,
            -0.02 / aspect.max(0.01),
            -0.75,
        ),
        _ => (
            0.02 / aspect.max(0.01),
            -0.88,
            0.39 / aspect.max(0.01),
            -0.75,
        ),
    }
}

fn wrapped_book_lines(page: &str, columns: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in page.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_inclusive(char::is_whitespace) {
            let word = word.trim_end_matches(char::is_whitespace);
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > columns {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            if word.chars().count() > columns {
                for ch in word.chars() {
                    if line.chars().count() == columns {
                        lines.push(std::mem::take(&mut line));
                    }
                    line.push(ch);
                }
            } else {
                line.push_str(word);
            }
        }
        lines.push(line);
    }
    lines.truncate(max_lines);
    lines
}

fn book_geometry(
    gui: &GuiAtlas,
    book: &BookScreen,
    cursor: (f32, f32),
    aspect: f32,
) -> (Vec<[f32; 5]>, Vec<[f32; 4]>) {
    let a = aspect.max(0.01);
    let half_width = 0.46 / a;
    let mut color = Vec::new();
    let mut text = Vec::new();
    push_color2d(
        &mut color,
        -half_width,
        -0.72,
        half_width,
        0.78,
        [0.86, 0.78, 0.58],
    );
    push_color2d(
        &mut color,
        -half_width + 0.025 / a,
        -0.68,
        half_width - 0.025 / a,
        0.73,
        [0.96, 0.91, 0.75],
    );
    let heading = book.title.as_deref().unwrap_or(if book.signing {
        "Sign Book"
    } else if book.writable {
        "Writable Book"
    } else {
        "Written Book"
    });
    let heading_h = 0.052;
    let heading_w = gui.text_width(heading) * (heading_h / 8.0) / a;
    crab_render::push_text(
        &mut text,
        gui,
        heading,
        -heading_w / 2.0,
        0.67,
        heading_h,
        a,
    );
    if let Some(author) = book.author.as_deref() {
        let label = format!("by {author}");
        let h = 0.035;
        let width = gui.text_width(&label) * (h / 8.0) / a;
        crab_render::push_text(&mut text, gui, &label, -width / 2.0, 0.60, h, a);
    }
    if book.signing {
        crab_render::push_text(
            &mut text,
            gui,
            "Enter book title:",
            -half_width + 0.07 / a,
            0.46,
            0.052,
            a,
        );
        crab_render::push_text(
            &mut text,
            gui,
            &format!("{}|", book.sign_title),
            -half_width + 0.07 / a,
            0.32,
            0.06,
            a,
        );
        crab_render::push_text(
            &mut text,
            gui,
            "Signing makes the book read-only.",
            -half_width + 0.07 / a,
            0.12,
            0.04,
            a,
        );
    } else {
        let page = book.pages.get(book.page).map_or("", String::as_str);
        for (index, line) in wrapped_book_lines(page, 32, 14).iter().enumerate() {
            crab_render::push_text(
                &mut text,
                gui,
                line,
                -half_width + 0.07 / a,
                0.50 - index as f32 * 0.075,
                0.052,
                a,
            );
        }
    }
    let page_label = if book.signing {
        String::new()
    } else {
        format!("Page {} of {}", book.page + 1, book.pages.len())
    };
    let label_h = 0.036;
    let label_w = gui.text_width(&page_label) * (label_h / 8.0) / a;
    crab_render::push_text(
        &mut text,
        gui,
        &page_label,
        -label_w / 2.0,
        -0.58,
        label_h,
        a,
    );
    let buttons: &[(usize, &str)] = if book.signing {
        &[(4, "Finalize"), (5, "Cancel")]
    } else if book.writable {
        &[(0, "<"), (1, ">"), (2, "Sign"), (3, "Done")]
    } else {
        &[(0, "<"), (1, ">"), (3, "Done")]
    };
    for &(index, label) in buttons {
        let rect = book_button_rect(a, index);
        let hovered =
            cursor.0 >= rect.0 && cursor.0 <= rect.2 && cursor.1 >= rect.1 && cursor.1 <= rect.3;
        push_color2d(
            &mut color,
            rect.0,
            rect.1,
            rect.2,
            rect.3,
            if hovered {
                [0.65, 0.55, 0.34]
            } else {
                [0.42, 0.34, 0.22]
            },
        );
        let h = (rect.3 - rect.1) * 0.55;
        let width = gui.text_width(label) * (h / 8.0) / a;
        crab_render::push_text(
            &mut text,
            gui,
            label,
            (rect.0 + rect.2 - width) / 2.0,
            rect.1 + (rect.3 - rect.1 + h) / 2.0,
            h,
            a,
        );
    }
    (color, text)
}

fn recipe_book_toggle_rect(panel: HudRect, aspect: f32) -> HudRect {
    let width = 0.11 / aspect.max(0.01);
    (
        panel.0 - width - 0.012,
        panel.3 - 0.17,
        panel.0 - 0.012,
        panel.3 - 0.06,
    )
}

fn recipe_book_panel_rect(panel: HudRect, aspect: f32) -> HudRect {
    let width = 0.52 / aspect.max(0.01);
    (panel.0 - width - 0.025, panel.1, panel.0 - 0.025, panel.3)
}

fn recipe_book_cell_rect(rect: HudRect, visible: usize) -> HudRect {
    let column = visible % 5;
    let row = visible / 5;
    let margin_x = (rect.2 - rect.0) * 0.06;
    let top = rect.3 - (rect.3 - rect.1) * 0.16;
    let size = (rect.2 - rect.0 - margin_x * 2.0) / 5.0;
    let x0 = rect.0 + margin_x + column as f32 * size;
    let y1 = top - row as f32 * size * 1.18;
    (x0, y1 - size, x0 + size, y1)
}

fn recipe_book_geometry(
    gui: &GuiAtlas,
    item_atlas: &ItemAtlas,
    recipes: &[CraftingRecipe],
    page: usize,
    cursor: (f32, f32),
    panel: HudRect,
    aspect: f32,
) -> RecipeBookGeometry {
    let rect = recipe_book_panel_rect(panel, aspect);
    let mut color = Vec::new();
    let mut items = Vec::new();
    let mut text = Vec::new();
    push_color2d(
        &mut color,
        rect.0,
        rect.1,
        rect.2,
        rect.3,
        [0.28, 0.25, 0.20],
    );
    push_color2d(
        &mut color,
        rect.0 + 0.012,
        rect.1 + 0.012,
        rect.2 - 0.012,
        rect.3 - 0.012,
        [0.72, 0.68, 0.56],
    );
    crab_render::push_text(
        &mut text,
        gui,
        "Recipe Book",
        rect.0 + 0.035 / aspect.max(0.01),
        rect.3 - 0.055,
        0.045,
        aspect,
    );
    let start = page * 20;
    for (visible, recipe) in recipes.iter().skip(start).take(20).enumerate() {
        let cell = recipe_book_cell_rect(rect, visible);
        let hovered =
            cursor.0 >= cell.0 && cursor.0 <= cell.2 && cursor.1 >= cell.1 && cursor.1 <= cell.3;
        push_color2d(
            &mut color,
            cell.0,
            cell.1,
            cell.2,
            cell.3,
            if hovered {
                [0.85, 0.78, 0.45]
            } else {
                [0.48, 0.43, 0.32]
            },
        );
        if let Some(uv) = recipe
            .result
            .and_then(|item| u32::try_from(item.item_id).ok())
            .and_then(crab_registry::item_name)
            .and_then(|name| item_atlas.icon(name))
        {
            let inset = (cell.2 - cell.0) * 0.14;
            push_tex2d(
                &mut items,
                cell.0 + inset,
                cell.1 + inset,
                cell.2 - inset,
                cell.3 - inset,
                uv,
            );
        }
    }
    let page_count = recipes.len().max(1).div_ceil(20);
    let label = format!("{} / {}", page.min(page_count - 1) + 1, page_count);
    crab_render::push_text(
        &mut text,
        gui,
        &label,
        rect.0 + 0.15 / aspect.max(0.01),
        rect.1 + 0.065,
        0.038,
        aspect,
    );
    (color, items, text)
}

/// Builds the 9 hotbar item-icon UVs from the player's inventory (slots 36..44).
fn hotbar_icons(shared: &Shared, item_atlas: &ItemAtlas) -> Vec<Option<[f32; 4]>> {
    let inv = shared.inventory.lock().unwrap();
    (0..9)
        .map(|i| {
            inv.get(36 + i).and_then(|s| *s).and_then(|it| {
                let id = u32::try_from(it.item_id).ok()?;
                item_atlas.icon(crab_registry::item_name(id)?)
            })
        })
        .collect()
}

fn push_color_quad(v: &mut Vec<[f32; 5]>, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 3]) {
    for [px, py] in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]] {
        v.push([px, py, c[0], c[1], c[2]]);
    }
}

/// Pushes a textured 2D quad (item-atlas UV) into a HUD textured stream.
fn push_tex2d(v: &mut Vec<[f32; 4]>, x0: f32, y0: f32, x1: f32, y1: f32, uv: [f32; 4]) {
    let [u0, v0, u1, v1] = uv;
    for q in [
        [x0, y1, u0, v0],
        [x1, y1, u1, v0],
        [x1, y0, u1, v1],
        [x0, y1, u0, v0],
        [x1, y0, u1, v1],
        [x0, y0, u0, v1],
    ] {
        v.push(q);
    }
}

fn rotated_point(center: [f32; 2], point: [f32; 2], radians: f32) -> [f32; 2] {
    let (sin, cos) = radians.sin_cos();
    let x = point[0] - center[0];
    let y = point[1] - center[1];
    [center[0] + x * cos - y * sin, center[1] + x * sin + y * cos]
}

/// Pushes an item-atlas quad rotated around its center. This keeps first-person
/// items in HUD space, so terrain never incorrectly draws over the player's hand.
fn push_rotated_tex2d(
    vertices: &mut Vec<[f32; 4]>,
    center: [f32; 2],
    half_size: [f32; 2],
    radians: f32,
    uv: [f32; 4],
) {
    let [u0, v0, u1, v1] = uv;
    let corners = [
        rotated_point(
            center,
            [center[0] - half_size[0], center[1] + half_size[1]],
            radians,
        ),
        rotated_point(
            center,
            [center[0] + half_size[0], center[1] + half_size[1]],
            radians,
        ),
        rotated_point(
            center,
            [center[0] + half_size[0], center[1] - half_size[1]],
            radians,
        ),
        rotated_point(
            center,
            [center[0] - half_size[0], center[1] - half_size[1]],
            radians,
        ),
    ];
    for (corner, uv) in [
        (corners[0], [u0, v0]),
        (corners[1], [u1, v0]),
        (corners[2], [u1, v1]),
        (corners[0], [u0, v0]),
        (corners[2], [u1, v1]),
        (corners[3], [u0, v1]),
    ] {
        vertices.push([corner[0], corner[1], uv[0], uv[1]]);
    }
}

fn inventory_item_icon(shared: &Shared, item_atlas: &ItemAtlas, slot: usize) -> Option<[f32; 4]> {
    let item = shared
        .inventory
        .lock()
        .unwrap()
        .get(slot)
        .copied()
        .flatten()?;
    let name = crab_registry::item_name(u32::try_from(item.item_id).ok()?)?;
    item_atlas.icon(name)
}

fn first_person_items_geometry(
    item_vertices: &mut Vec<[f32; 4]>,
    main_hand: Option<[f32; 4]>,
    offhand: Option<[f32; 4]>,
    swing: f32,
    aspect: f32,
) {
    let arc = (swing.clamp(0.0, 1.0) * std::f32::consts::PI).sin();
    let half_size = [0.22 / aspect.max(0.01), 0.22];
    if let Some(uv) = offhand {
        push_rotated_tex2d(
            item_vertices,
            [-0.72 + arc * 0.04, -0.58 - arc * 0.05],
            half_size,
            0.32,
            uv,
        );
    }
    if let Some(uv) = main_hand {
        push_rotated_tex2d(
            item_vertices,
            [0.72 - arc * 0.22, -0.56 - arc * 0.28],
            half_size,
            -0.32 - arc * 0.5,
            uv,
        );
    }
}

fn push_color2d(vertices: &mut Vec<[f32; 5]>, x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 3]) {
    for [x, y] in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]] {
        vertices.push([x, y, color[0], color[1], color[2]]);
    }
}

fn menu_pixel_rect(
    rect: (f32, f32, f32, f32),
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> (f32, f32, f32, f32) {
    let sx = (rect.2 - rect.0) / 176.0;
    let sy = (rect.3 - rect.1) / 166.0;
    let x0 = rect.0 + x * sx;
    let y1 = rect.3 - y * sy;
    (x0, y1 - height * sy, x0 + width * sx, y1)
}

fn recipe_button_rect(
    texture: &str,
    _page: usize,
    visible: usize,
    aspect: f32,
) -> (f32, f32, f32, f32) {
    let rect = crab_render::simple_container_rect(texture, aspect);
    let (columns, x, y, width, height) = if texture == "loom" {
        (4, 61.0, 13.0, 14.0, 14.0)
    } else {
        (4, 52.0, 14.0, 16.0, 18.0)
    };
    let local = visible % if texture == "loom" { 16 } else { 12 };
    menu_pixel_rect(
        rect,
        x + (local % columns) as f32 * width,
        y + (local / columns) as f32 * height,
        width - 1.0,
        height - 1.0,
    )
}

#[allow(clippy::too_many_arguments)]
fn recipe_selection_geometry(
    color: &mut Vec<[f32; 5]>,
    items: &mut Vec<[f32; 4]>,
    text: &mut Vec<[f32; 4]>,
    gui: &GuiAtlas,
    texture: &str,
    page: usize,
    selected: i16,
    total: usize,
    icons: &[Option<[f32; 4]>],
    cursor: (f32, f32),
    aspect: f32,
) {
    let per_page = if texture == "loom" { 16 } else { 12 };
    for visible in 0..per_page {
        let index = page * per_page + visible;
        if index >= total {
            continue;
        }
        let rect = recipe_button_rect(texture, page, visible, aspect);
        let hovered =
            cursor.0 >= rect.0 && cursor.0 <= rect.2 && cursor.1 >= rect.1 && cursor.1 <= rect.3;
        let active = usize::try_from(selected).ok() == Some(index);
        let shade = if active {
            [0.78, 0.72, 0.35]
        } else if hovered {
            [0.62, 0.62, 0.68]
        } else {
            [0.32, 0.32, 0.36]
        };
        push_color2d(color, rect.0, rect.1, rect.2, rect.3, shade);
        if let Some(uv) = icons.get(index).copied().flatten() {
            let inset_x = (rect.2 - rect.0) * 0.09;
            let inset_y = (rect.3 - rect.1) * 0.09;
            push_tex2d(
                items,
                rect.0 + inset_x,
                rect.1 + inset_y,
                rect.2 - inset_x,
                rect.3 - inset_y,
                uv,
            );
            continue;
        }
        let label = (index + 1).to_string();
        let h = (rect.3 - rect.1) * 0.52;
        let width = gui.text_width(&label) * (h / 8.0) / aspect.max(0.01);
        crab_render::push_text(
            text,
            gui,
            &label,
            (rect.0 + rect.2 - width) / 2.0,
            (rect.1 + rect.3 + h) / 2.0,
            h,
            aspect,
        );
    }
}

fn applicable_stonecutter_recipes(
    shared: &Shared,
    container: &ContainerState,
) -> Vec<crate::client::StonecutterRecipe> {
    let Some(input) = container.slots.first().copied().flatten() else {
        return Vec::new();
    };
    shared
        .stonecutter_recipes
        .lock()
        .unwrap()
        .iter()
        .filter(|recipe| recipe.ingredients.contains(&input.item_id))
        .cloned()
        .collect()
}

fn map_pixel_color(encoded: u8) -> Option<[f32; 3]> {
    if encoded == 0 {
        return None;
    }
    const BASE: [u32; 36] = [
        0x000000, 0x7fb238, 0xf7e9a3, 0xc7c7c7, 0xff0000, 0xa0a0ff, 0xa7a7a7, 0x007c00, 0xffffff,
        0xa4a8b8, 0x976d4d, 0x707070, 0x4040ff, 0x8f7748, 0xfffcf5, 0xd87f33, 0xb24cd8, 0x6699d8,
        0xe5e533, 0x7fcc19, 0xf27fa5, 0x4c4c4c, 0x999999, 0x4c7f99, 0x7f3fb2, 0x334cb2, 0x664c33,
        0x667f33, 0x993333, 0x191919, 0xfaee4d, 0x5cdbd5, 0x4a80ff, 0x00d93a, 0x815631, 0x700200,
    ];
    const SHADE: [f32; 4] = [180.0 / 255.0, 220.0 / 255.0, 1.0, 135.0 / 255.0];
    let base_index = usize::from(encoded / 4);
    let rgb = BASE.get(base_index).copied().unwrap_or_else(|| {
        let hue = (base_index as u32).wrapping_mul(0x045d_9f3b);
        0x404040 | (hue & 0xbfbfbf)
    });
    let shade = SHADE[usize::from(encoded % 4)];
    Some([
        f32::from(((rgb >> 16) & 0xff) as u8) / 255.0 * shade,
        f32::from(((rgb >> 8) & 0xff) as u8) / 255.0 * shade,
        f32::from((rgb & 0xff) as u8) / 255.0 * shade,
    ])
}

fn held_map_geometry(
    color: &mut Vec<[f32; 5]>,
    text: &mut Vec<[f32; 4]>,
    gui: &GuiAtlas,
    map: &crate::client::MapState,
    aspect: f32,
) {
    let size = 1.18;
    let half_width = size / aspect.max(0.01);
    let (x0, x1, y0, y1) = (-half_width, half_width, -0.82, 0.36);
    push_color2d(
        color,
        x0 - 0.055,
        y0 - 0.055,
        x1 + 0.055,
        y1 + 0.055,
        [0.28, 0.18, 0.09],
    );
    push_color2d(color, x0, y0, x1, y1, [0.82, 0.76, 0.59]);
    let pixel_w = (x1 - x0) / 128.0;
    let pixel_h = (y1 - y0) / 128.0;
    for row in 0..128 {
        let mut column = 0;
        while column < 128 {
            let encoded = map.colors[row * 128 + column];
            let mut end = column + 1;
            while end < 128 && map.colors[row * 128 + end] == encoded {
                end += 1;
            }
            if let Some(pixel) = map_pixel_color(encoded) {
                let px0 = x0 + column as f32 * pixel_w;
                let px1 = x0 + end as f32 * pixel_w;
                let py1 = y1 - row as f32 * pixel_h;
                push_color2d(color, px0, py1 - pixel_h, px1, py1, pixel);
            }
            column = end;
        }
    }
    for marker in &map.markers {
        let mx = x0 + (f32::from(marker.x) + 128.0) / 256.0 * (x1 - x0);
        let my = y1 - (f32::from(marker.z) + 128.0) / 256.0 * (y1 - y0);
        let radius = 0.012;
        let marker_color = if marker.kind == 0 {
            [0.9, 0.2, 0.15]
        } else {
            [0.2, 0.25, 0.8]
        };
        push_color2d(
            color,
            mx - radius,
            my - radius,
            mx + radius,
            my + radius,
            marker_color,
        );
        if let Some(label) = marker.label.as_ref() {
            let h = 0.028;
            crab_render::push_text(text, gui, label, mx + radius, my + h, h, aspect);
        }
    }
}

/// Builds chat geometry: recent log lines (and the input line when open) as
/// dark bars + bitmap text near the bottom-left. Returns `(color, text)`.
fn chat_geometry(
    shared: &Shared,
    gui: &GuiAtlas,
    chat_open: bool,
    buffer: &str,
    aspect: f32,
) -> (Vec<[f32; 5]>, Vec<[f32; 4]>) {
    let mut color = Vec::new();
    let mut text = Vec::new();
    let line_h = 0.038;
    let x = -0.96;
    let y_in = -0.52; // input-line top (above the hotbar)
    let hscale = (line_h / 8.0) / aspect.max(0.01);
    let bar = |color: &mut Vec<[f32; 5]>, s: &str, y: f32| {
        let w = (gui.text_width(s) * hscale).max(0.05);
        push_color_quad(
            color,
            x - 0.006,
            y - line_h,
            x + w + 0.006,
            y,
            [0.05, 0.05, 0.05],
        );
    };

    let log = shared.chat_log.lock().unwrap();
    let shown = if chat_open { 10 } else { 7 };
    for (i, line) in log.iter().rev().take(shown).enumerate() {
        let y = y_in - (i as f32 + 1.0) * (line_h * 1.25);
        bar(&mut color, line, y);
        crab_render::push_text(&mut text, gui, line, x, y, line_h, aspect);
    }
    if chat_open {
        let s = format!("> {buffer}");
        bar(&mut color, &s, y_in);
        crab_render::push_text(&mut text, gui, &s, x, y_in, line_h, aspect);
    }
    (color, text)
}

/// Builds the stack-size number text (font quads) for the hotbar, and for the
/// inventory grid when it's open. Counts of 1 are not shown.
fn count_text(shared: &Shared, gui: &GuiAtlas, aspect: f32, inv_open: bool) -> Vec<[f32; 4]> {
    let inv = shared.inventory.lock().unwrap();
    let mut out = Vec::new();
    let mut push_count = |count: i8, rect: (f32, f32, f32, f32)| {
        if count <= 1 {
            return;
        }
        let s = count.to_string();
        let (_x0, y0, x1, y1) = rect;
        let h = (y1 - y0) * 0.5;
        let w = gui.text_width(&s) * (h / 8.0) / aspect.max(0.01);
        crab_render::push_text(&mut out, gui, &s, x1 - w, y0 + h, h, aspect);
    };
    for i in 0..9 {
        if let Some(it) = inv.get(36 + i).and_then(|s| *s) {
            push_count(it.count, crab_render::hotbar_slot_rect(aspect, i));
        }
    }
    if inv_open {
        let rect = crab_render::inventory_rect(aspect);
        for slot in 0..46 {
            if let Some(it) = inv.get(slot).and_then(|s| *s) {
                push_count(it.count, crab_render::inventory_slot_rect(rect, slot));
            }
        }
    }
    out
}

/// Builds the item-icon UVs for all 46 player-inventory window slots.
fn inventory_icons(shared: &Shared, item_atlas: &ItemAtlas) -> Vec<Option<[f32; 4]>> {
    let inv = shared.inventory.lock().unwrap();
    (0..46)
        .map(|i| {
            inv.get(i).and_then(|s| *s).and_then(|it| {
                let id = u32::try_from(it.item_id).ok()?;
                item_atlas.icon(crab_registry::item_name(id)?)
            })
        })
        .collect()
}

struct InventoryPlayerPreview {
    mesh: Vec<Vertex>,
    camera: crab_render::Camera,
    bounds: (f32, f32, f32, f32),
}

fn inventory_player_preview(
    entity_atlas: &EntityAtlas,
    cursor: (f32, f32),
    aspect: f32,
) -> Option<InventoryPlayerPreview> {
    let player_id = crab_registry::entities()
        .iter()
        .find(|entity| entity.name == "player")?
        .id as i32;
    let model = entity_atlas.models.get(&player_id)?;
    let panel = crab_render::inventory_rect(aspect);
    let panel_w = panel.2 - panel.0;
    let panel_h = panel.3 - panel.1;
    // Vanilla's player viewport occupies pixels 26..75 × 8..78 in the
    // 176×166 inventory texture. Keeping those exact proportions prevents the
    // preview from covering armour/crafting slots at unusual aspect ratios.
    let bounds = (
        panel.0 + panel_w * 26.0 / 176.0,
        panel.3 - panel_h * 78.0 / 166.0,
        panel.0 + panel_w * 75.0 / 176.0,
        panel.3 - panel_h * 8.0 / 166.0,
    );
    let center = ((bounds.0 + bounds.2) * 0.5, (bounds.1 + bounds.3) * 0.5);
    let pointer_x = ((cursor.0 - center.0) / (bounds.2 - bounds.0).max(0.01)).clamp(-1.5, 1.5);
    let pointer_y = ((cursor.1 - center.1) / (bounds.3 - bounds.1).max(0.01)).clamp(-1.5, 1.5);
    let yaw = pointer_x * 28.0;
    let mesh = entity_mesh(
        &model.geo,
        [0.0, 0.0, 0.0],
        [model.atlas_x, model.atlas_y],
        [entity_atlas.width as f32, entity_atlas.height as f32],
        0.0,
        0.0,
        1.0,
        yaw,
        yaw + pointer_x * 18.0,
        0.0,
    );
    let viewport_aspect = ((bounds.2 - bounds.0) * aspect / (bounds.3 - bounds.1)).max(0.01);
    let camera = crab_render::Camera {
        eye: Vec3::new(0.0, 0.9 + pointer_y * 0.08, 4.2),
        target: Vec3::new(0.0, 0.9, 0.0),
        up: Vec3::Y,
        aspect: viewport_aspect,
        fovy_radians: 34.0_f32.to_radians(),
        znear: 0.1,
        zfar: 20.0,
    };
    Some(InventoryPlayerPreview {
        mesh,
        camera,
        bounds,
    })
}

fn slot_icons(
    slots: &[Option<crab_protocol::versions::v1_20_1::play::SlotItem>],
    item_atlas: &ItemAtlas,
) -> Vec<Option<[f32; 4]>> {
    slots
        .iter()
        .map(|slot| {
            slot.and_then(|it| {
                let id = u32::try_from(it.item_id).ok()?;
                item_atlas.icon(crab_registry::item_name(id)?)
            })
        })
        .collect()
}

fn container_text(
    container: &ContainerState,
    gui: &GuiAtlas,
    aspect: f32,
    anvil_name: &str,
) -> Vec<[f32; 4]> {
    let rows = container.generic_rows();
    let (panel, pixel_h) = if let Some(rows) = rows {
        (
            crab_render::container_rect(rows, aspect),
            114.0 + rows as f32 * 18.0,
        )
    } else if container.furnace_texture().is_some() {
        (crab_render::inventory_rect(aspect), 166.0)
    } else if let Some(texture) = container.simple_container_texture() {
        (
            crab_render::simple_container_rect(texture, aspect),
            if texture == "hopper" { 133.0 } else { 166.0 },
        )
    } else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let h = (panel.3 - panel.1) / pixel_h * 9.0;
    let x = panel.0 + (panel.2 - panel.0) * 8.0 / 176.0;
    let y = panel.3 - (panel.3 - panel.1) * 6.0 / pixel_h;
    crab_render::push_text(&mut out, gui, &container.title, x, y, h, aspect);
    if container.menu_type == 12 {
        for option in 0..3 {
            let cost = container.properties[option];
            if cost <= 0 {
                continue;
            }
            let text = cost.to_string();
            let th = (panel.3 - panel.1) * 8.0 / 166.0;
            let right = panel.0 + (panel.2 - panel.0) * 166.0 / 176.0;
            let top = panel.3 - (panel.3 - panel.1) * (20.0 + option as f32 * 19.0) / 166.0;
            let tw = gui.text_width(&text) * (th / 8.0) / aspect.max(0.01);
            crab_render::push_text(&mut out, gui, &text, right - tw, top, th, aspect);
        }
    }
    if container.menu_type == 7 && !anvil_name.is_empty() {
        let th = (panel.3 - panel.1) * 9.0 / 166.0;
        let tx = panel.0 + (panel.2 - panel.0) * 62.0 / 176.0;
        let ty = panel.3 - (panel.3 - panel.1) * 24.0 / 166.0;
        crab_render::push_text(&mut out, gui, anvil_name, tx, ty, th, aspect);
    }
    for (slot, item) in container.slots.iter().enumerate() {
        let Some(it) = item.filter(|it| it.count > 1) else {
            continue;
        };
        let rect = if let Some(rows) = rows {
            crab_render::container_slot_rect(panel, rows, slot)
        } else if let Some(texture) = container.simple_container_texture() {
            crab_render::simple_container_slot_rect(panel, texture, slot)
        } else {
            crab_render::furnace_slot_rect(panel, slot)
        };
        let s = it.count.to_string();
        let th = (rect.3 - rect.1) * 0.5;
        let tw = gui.text_width(&s) * (th / 8.0) / aspect.max(0.01);
        crab_render::push_text(&mut out, gui, &s, rect.2 - tw, rect.1 + th, th, aspect);
    }
    out
}

/// GPU + window resources, created once the event loop is `resumed`.
struct Graphics {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    inventory_camera_buffer: wgpu::Buffer,
    inventory_camera_bind_group: wgpu::BindGroup,
    atlas_bind_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    depth_view: wgpu::TextureView,
    /// Cached vertex buffer per chunk column.
    chunk_meshes: HashMap<(i32, i32), (wgpu::Buffer, u32)>,
    hud: HudPipelines,
    hud_color_buffer: Option<(wgpu::Buffer, u32)>,
    /// HUD GUI-sprite verts (hotbar/inventory backgrounds, gui atlas).
    hud_gui_buffer: Option<(wgpu::Buffer, u32)>,
    /// HUD item-icon verts (item atlas).
    hud_item_buffer: Option<(wgpu::Buffer, u32)>,
    /// HUD text verts (font glyphs, gui atlas) drawn last.
    hud_text_buffer: Option<(wgpu::Buffer, u32)>,
    /// Item-icon atlas bound for the HUD's textured pass.
    item_atlas_bind_group: wgpu::BindGroup,
    /// GUI sprite + font atlas bound for the HUD's gui/text passes.
    gui_atlas_bind_group: wgpu::BindGroup,
    /// Entity texture atlas (for 3D entity models).
    entity_atlas_bind_group: wgpu::BindGroup,
    /// Item atlas bound in the *world* pass for dropped-item billboards.
    item_world_bind_group: wgpu::BindGroup,
    /// Per-frame box mesh for entities lacking a model (block atlas texture).
    box_entity_buffer: Option<(wgpu::Buffer, u32)>,
    /// Per-frame 3D model mesh for entities with a model (entity atlas texture).
    model_entity_buffer: Option<(wgpu::Buffer, u32)>,
    /// Per-frame dropped-item billboards (item atlas texture).
    item_entity_buffer: Option<(wgpu::Buffer, u32)>,
    /// Destroy-stage (block-breaking crack) overlay atlas, if loaded.
    crack_bind_group: Option<wgpu::BindGroup>,
    /// Per-frame crack overlay cube for the block being mined.
    crack_buffer: Option<(wgpu::Buffer, u32)>,
    /// Vanilla-style 3D local-player preview shown over the inventory panel.
    inventory_player_buffer: Option<(wgpu::Buffer, u32)>,
    inventory_player_bounds: Option<(f32, f32, f32, f32)>,
}

impl Graphics {
    fn new(
        window: Arc<Window>,
        atlas: &Atlas,
        entity_atlas: &EntityAtlas,
        item_atlas: &ItemAtlas,
        gui_atlas: &GuiAtlas,
        crack: Option<&(Vec<u8>, u32, u32)>,
    ) -> Self {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("no adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("crabcraft device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let (pipeline, camera_bgl, texture_bgl) = build_block_pipeline(&device, config.format);
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::cast_slice(&[CameraUniform {
                view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
                lighting: [1.0, 0.0, 0.0, 0.0],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let inventory_camera_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("inventory player camera"),
                contents: bytemuck::cast_slice(&[CameraUniform {
                    view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    lighting: [1.0, 0.0, 0.0, 0.0],
                }]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let inventory_camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("inventory player camera bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: inventory_camera_buffer.as_entire_binding(),
            }],
        });
        let atlas_bind_group = upload_atlas(&device, &queue, &texture_bgl, atlas);
        let entity_atlas_bind_group = upload_texture(
            &device,
            &queue,
            &texture_bgl,
            &entity_atlas.rgba,
            entity_atlas.width,
            entity_atlas.height,
        );

        // Item atlas bound in the world pass (block-pipeline layout) for
        // dropped-item billboards.
        let item_world_bind_group = upload_texture(
            &device,
            &queue,
            &texture_bgl,
            &item_atlas.rgba,
            item_atlas.width,
            item_atlas.height,
        );

        let hud = build_hud_pipelines(&device, config.format);
        let item_atlas_bind_group = upload_texture(
            &device,
            &queue,
            &hud.atlas_layout,
            &item_atlas.rgba,
            item_atlas.width,
            item_atlas.height,
        );
        let gui_atlas_bind_group = upload_texture(
            &device,
            &queue,
            &hud.atlas_layout,
            &gui_atlas.rgba,
            gui_atlas.width,
            gui_atlas.height,
        );

        // Block-breaking crack overlay atlas (drawn with the world pipeline:
        // crack pixels are opaque, the rest is transparent and discarded).
        let crack_bind_group =
            crack.map(|(rgba, w, h)| upload_texture(&device, &queue, &texture_bgl, rgba, *w, *h));

        let depth_view = create_depth(&device, width, height);

        Self {
            window,
            surface,
            device,
            queue,
            config,
            pipeline,
            camera_buffer,
            camera_bind_group,
            inventory_camera_buffer,
            inventory_camera_bind_group,
            atlas_bind_group,
            texture_layout: texture_bgl,
            depth_view,
            chunk_meshes: HashMap::new(),
            hud,
            hud_color_buffer: None,
            hud_gui_buffer: None,
            hud_item_buffer: None,
            hud_text_buffer: None,
            item_atlas_bind_group,
            gui_atlas_bind_group,
            entity_atlas_bind_group,
            item_world_bind_group,
            box_entity_buffer: None,
            model_entity_buffer: None,
            item_entity_buffer: None,
            crack_bind_group,
            crack_buffer: None,
            inventory_player_buffer: None,
            inventory_player_bounds: None,
        }
    }

    fn reload_assets(
        &mut self,
        atlas: &Atlas,
        entity_atlas: &EntityAtlas,
        item_atlas: &ItemAtlas,
        gui_atlas: &GuiAtlas,
        crack: Option<&(Vec<u8>, u32, u32)>,
    ) {
        self.atlas_bind_group =
            upload_atlas(&self.device, &self.queue, &self.texture_layout, atlas);
        self.entity_atlas_bind_group = upload_texture(
            &self.device,
            &self.queue,
            &self.texture_layout,
            &entity_atlas.rgba,
            entity_atlas.width,
            entity_atlas.height,
        );
        self.item_world_bind_group = upload_texture(
            &self.device,
            &self.queue,
            &self.texture_layout,
            &item_atlas.rgba,
            item_atlas.width,
            item_atlas.height,
        );
        self.item_atlas_bind_group = upload_texture(
            &self.device,
            &self.queue,
            &self.hud.atlas_layout,
            &item_atlas.rgba,
            item_atlas.width,
            item_atlas.height,
        );
        self.gui_atlas_bind_group = upload_texture(
            &self.device,
            &self.queue,
            &self.hud.atlas_layout,
            &gui_atlas.rgba,
            gui_atlas.width,
            gui_atlas.height,
        );
        self.crack_bind_group = crack.map(|(rgba, width, height)| {
            upload_texture(
                &self.device,
                &self.queue,
                &self.texture_layout,
                rgba,
                *width,
                *height,
            )
        });
        self.chunk_meshes.clear();
    }

    fn set_hud(
        &mut self,
        color_verts: &[[f32; 5]],
        gui_verts: &[[f32; 4]],
        item_verts: &[[f32; 4]],
        text_verts: &[[f32; 4]],
    ) {
        let tex_buf = |device: &wgpu::Device, label: &str, verts: &[[f32; 4]]| {
            (!verts.is_empty()).then(|| {
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytemuck::cast_slice(verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                (buffer, verts.len() as u32)
            })
        };
        self.hud_gui_buffer = tex_buf(&self.device, "hud gui", gui_verts);
        self.hud_item_buffer = tex_buf(&self.device, "hud item", item_verts);
        self.hud_text_buffer = tex_buf(&self.device, "hud text", text_verts);
        self.hud_color_buffer = (!color_verts.is_empty()).then(|| {
            let buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("hud color"),
                    contents: bytemuck::cast_slice(color_verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            (buffer, color_verts.len() as u32)
        });
    }

    fn make_vertex_buffer(&self, vertices: &[Vertex]) -> Option<(wgpu::Buffer, u32)> {
        if vertices.is_empty() {
            return None;
        }
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("entity vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        Some((buffer, vertices.len() as u32))
    }

    fn set_inventory_player(
        &mut self,
        vertices: &[Vertex],
        camera: Option<&crab_render::Camera>,
        bounds: Option<(f32, f32, f32, f32)>,
    ) {
        self.inventory_player_buffer = self.make_vertex_buffer(vertices);
        self.inventory_player_bounds = bounds;
        if let Some(camera) = camera {
            let uniform = CameraUniform::with_light(camera, 1.0);
            self.queue.write_buffer(
                &self.inventory_camera_buffer,
                0,
                bytemuck::cast_slice(&[uniform]),
            );
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth(&self.device, width, height);
    }

    fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height as f32
    }

    fn upload_chunk(&mut self, coord: (i32, i32), vertices: &[Vertex]) {
        if vertices.is_empty() {
            self.chunk_meshes.remove(&coord);
            return;
        }
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        self.chunk_meshes
            .insert(coord, (buffer, vertices.len() as u32));
    }

    fn render(&mut self, camera: &crab_render::Camera, clear: [f64; 3]) {
        let light = (clear[2] / 0.92).clamp(0.08, 1.0) as f32;
        let uniform = CameraUniform::with_light(camera, light);
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&[uniform]));

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0],
                            g: clear[1],
                            b: clear[2],
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(1, &self.atlas_bind_group, &[]);
            for (buffer, count) in self.chunk_meshes.values() {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            // Box entities (no model) — block atlas still bound at group 1.
            if let Some((buffer, count)) = &self.box_entity_buffer {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            // 3D model entities — rebind group 1 to the entity texture atlas.
            if let Some((buffer, count)) = &self.model_entity_buffer {
                pass.set_bind_group(1, &self.entity_atlas_bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            // Dropped-item billboards — rebind group 1 to the item atlas.
            if let Some((buffer, count)) = &self.item_entity_buffer {
                pass.set_bind_group(1, &self.item_world_bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            // Block-breaking crack overlay on the block being mined.
            if let (Some(bind), Some((buffer, count))) =
                (&self.crack_bind_group, &self.crack_buffer)
            {
                pass.set_bind_group(1, bind, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            // HUD backgrounds and GUI sprites are drawn before the inventory
            // player preview so the 3D model appears inside the panel cutout.
            if let Some((buf, count)) = &self.hud_color_buffer {
                pass.set_pipeline(&self.hud.color);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..*count, 0..1);
            }
            pass.set_pipeline(&self.hud.textured);
            for (buffer, bind) in [(&self.hud_gui_buffer, &self.gui_atlas_bind_group)] {
                if let Some((buf, count)) = buffer {
                    pass.set_bind_group(0, bind, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
        }
        if let (Some((buffer, count)), Some(bounds)) =
            (&self.inventory_player_buffer, self.inventory_player_bounds)
        {
            let width = self.config.width as f32;
            let height = self.config.height as f32;
            let x = ((bounds.0 + 1.0) * 0.5 * width).clamp(0.0, width);
            let y = ((1.0 - bounds.3) * 0.5 * height).clamp(0.0, height);
            let viewport_width = ((bounds.2 - bounds.0) * 0.5 * width).max(1.0);
            let viewport_height = ((bounds.3 - bounds.1) * 0.5 * height).max(1.0);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("inventory player pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_viewport(x, y, viewport_width, viewport_height, 0.0, 1.0);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.inventory_camera_bind_group, &[]);
            pass.set_bind_group(1, &self.entity_atlas_bind_group, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..*count, 0..1);
        }
        {
            // Item icons and text remain above the 3D inventory preview.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hud foreground pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.hud.textured);
            for (buffer, bind) in [
                (&self.hud_item_buffer, &self.item_atlas_bind_group),
                (&self.hud_text_buffer, &self.gui_atlas_bind_group),
            ] {
                if let Some((buf, count)) = buffer {
                    pass.set_bind_group(0, bind, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}

fn create_depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

#[derive(Clone, Debug)]
struct BookScreen {
    pages: Vec<String>,
    page: usize,
    writable: bool,
    dirty: bool,
    hotbar_slot: i32,
    title: Option<String>,
    author: Option<String>,
    signing: bool,
    sign_title: String,
    submitted_title: Option<String>,
}

struct App {
    shared: Arc<Shared>,
    atlas: Arc<Atlas>,
    /// Atlas snapshot source shared with the background mesher.
    atlas_slot: Arc<RwLock<Arc<Atlas>>>,
    entity_atlas: Arc<EntityAtlas>,
    item_atlas: Arc<ItemAtlas>,
    gui_atlas: Arc<GuiAtlas>,
    /// Finished chunk meshes from the background mesher thread.
    mesh_rx: Receiver<((i32, i32), Vec<Vertex>)>,
    gfx: Option<Graphics>,
    /// Look angles in degrees (Minecraft convention).
    yaw: f32,
    pitch: f32,
    look_init: bool,
    keys: HashSet<KeyCode>,
    last_frame: Instant,
    /// Previous non-repeating Space press, used for vanilla-style flight toggling.
    last_space_press: Option<Instant>,
    /// One-shot request consumed by the network/physics thread.
    flight_toggle_pending: bool,
    swap_hands_pending: bool,
    /// Remaining first-person attack/use animation time.
    hand_swing_time: f32,
    was_attacking: bool,
    was_using_item: bool,
    /// Selected hotbar slot (0..=8), driven by number keys / scroll.
    selected_slot: u8,
    last_named_slot: u8,
    selected_name_time: f32,
    debug_open: bool,
    smoothed_fps: f32,
    /// Whether the inventory panel is open (E toggles).
    inventory_open: bool,
    /// Client-side readable/writable book screen opened from the held item.
    book: Option<BookScreen>,
    /// Last observed server-container state, for cursor capture transitions.
    container_seen: bool,
    resource_pack_seen: bool,
    applied_resource_pack: Option<PathBuf>,
    /// Chat input state: open + the line being typed.
    chat_open: bool,
    chat_buffer: String,
    /// Cursor position in NDC (for inventory/menu hit-testing).
    cursor: (f32, f32),
    /// Whether the pause menu is open (Esc).
    menu_open: bool,
    controls_help_open: bool,
    options_open: bool,
    fov_degrees: f32,
    mouse_sensitivity: f32,
    perspective: Perspective,
    local_walk_phase: f32,
    /// Scroll page for loom patterns and stonecutter recipes.
    recipe_page: usize,
    recipe_book_open: bool,
    recipe_book_page: usize,
    /// Current protocol-768 bundle tooltip selection `(slot, nested index)`.
    bundle_selection: Option<(i32, i32)>,
    /// Text currently entered in the focused anvil rename field.
    anvil_name: String,
    /// Per-entity smoothed render state (interpolation + walk animation).
    entity_anim: HashMap<i32, EntityAnim>,
    /// Smoothed camera eye position (eases toward the player's stepped pos).
    render_eye: Option<Vec3>,
    /// Destroy-stage overlay atlas (`(rgba, w, h)`), uploaded in `Graphics::new`.
    crack: Option<(Vec<u8>, u32, u32)>,
}

/// Smoothed render state for one entity.
#[derive(Clone, Copy)]
struct EntityAnim {
    pos: [f32; 3],
    /// Accumulated walk-cycle phase (radians).
    phase: f32,
    /// Smoothed limb-swing amplitude (0 = still).
    amount: f32,
    /// Smoothed facing yaw (degrees, Minecraft convention).
    yaw: f32,
    head_yaw: f32,
    swing_sequence: u64,
    hurt_sequence: u64,
    swing_time: f32,
    hurt_time: f32,
    age: f32,
}

impl App {
    fn new(
        shared: Arc<Shared>,
        atlas_slot: Arc<RwLock<Arc<Atlas>>>,
        entity_atlas: Arc<EntityAtlas>,
        item_atlas: Arc<ItemAtlas>,
        gui_atlas: Arc<GuiAtlas>,
        mesh_rx: Receiver<((i32, i32), Vec<Vertex>)>,
        crack: Option<(Vec<u8>, u32, u32)>,
    ) -> Self {
        let atlas = Arc::clone(&atlas_slot.read().unwrap());
        Self {
            shared,
            atlas,
            atlas_slot,
            entity_atlas,
            item_atlas,
            gui_atlas,
            mesh_rx,
            crack,
            gfx: None,
            yaw: 0.0,
            pitch: 0.0,
            look_init: false,
            keys: HashSet::new(),
            last_frame: Instant::now(),
            last_space_press: None,
            flight_toggle_pending: false,
            swap_hands_pending: false,
            hand_swing_time: 0.0,
            was_attacking: false,
            was_using_item: false,
            selected_slot: 0,
            last_named_slot: 0,
            selected_name_time: 0.0,
            debug_open: false,
            smoothed_fps: 60.0,
            inventory_open: false,
            book: None,
            container_seen: false,
            resource_pack_seen: false,
            applied_resource_pack: None,
            chat_open: false,
            chat_buffer: String::new(),
            cursor: (0.0, 0.0),
            menu_open: false,
            controls_help_open: false,
            options_open: false,
            fov_degrees: 70.0,
            mouse_sensitivity: 0.12,
            perspective: Perspective::FirstPerson,
            local_walk_phase: 0.0,
            recipe_page: 0,
            recipe_book_open: false,
            recipe_book_page: 0,
            bundle_selection: None,
            anvil_name: String::new(),
            entity_anim: HashMap::new(),
            render_eye: None,
        }
    }

    /// Advances per-entity interpolation + walk animation by `dt` and builds the
    /// entity meshes: `(box, model, item-billboard)`.
    fn step_entities(&mut self, dt: f32) -> (Vec<Vertex>, Vec<Vertex>, Vec<Vertex>) {
        let entities = self.shared.entities.lock().unwrap();
        let ease = 1.0 - (-dt * 12.0).exp();
        let mut alive = HashSet::new();
        for (&id, e) in entities.iter() {
            alive.insert(id);
            let mounted = e.vehicle.and_then(|vehicle| entities.get(&vehicle));
            let (x, y, z) = mounted.map_or((e.x, e.y, e.z), |vehicle| {
                (
                    vehicle.x,
                    vehicle.y + f64::from(vehicle.height) * 0.75,
                    vehicle.z,
                )
            });
            let target = [
                (x + e.velocity[0] * 0.05) as f32,
                (y + e.velocity[1] * 0.05) as f32,
                (z + e.velocity[2] * 0.05) as f32,
            ];
            let a = self.entity_anim.entry(id).or_insert(EntityAnim {
                pos: target,
                phase: 0.0,
                amount: 0.0,
                yaw: e.yaw,
                head_yaw: e.head_yaw,
                swing_sequence: e.swing_sequence,
                hurt_sequence: e.hurt_sequence,
                swing_time: 0.0,
                hurt_time: 0.0,
                age: 0.0,
            });
            a.age += dt;
            if a.swing_sequence != e.swing_sequence {
                a.swing_sequence = e.swing_sequence;
                a.swing_time = 0.35;
            }
            if a.hurt_sequence != e.hurt_sequence {
                a.hurt_sequence = e.hurt_sequence;
                a.hurt_time = 0.4;
            }
            a.swing_time = (a.swing_time - dt).max(0.0);
            a.hurt_time = (a.hurt_time - dt).max(0.0);
            let before = a.pos;
            for (k, &t) in target.iter().enumerate() {
                a.pos[k] += (t - a.pos[k]) * ease;
            }
            let (dx, dz) = (a.pos[0] - before[0], a.pos[2] - before[2]);
            let moved = (dx * dx + dz * dz).sqrt();
            a.phase += moved * 2.2;
            let target_amount = (moved / dt.max(1e-3) * 0.10).min(0.7);
            a.amount += (target_amount - a.amount) * ease;
            // Ease the facing yaw along the shortest arc (wrap at +-180).
            let mut dyaw = (e.yaw - a.yaw).rem_euclid(360.0);
            if dyaw > 180.0 {
                dyaw -= 360.0;
            }
            a.yaw += dyaw * ease;
            let mut dhead = (e.head_yaw - a.head_yaw).rem_euclid(360.0);
            if dhead > 180.0 {
                dhead -= 360.0;
            }
            a.head_yaw += dhead * ease;
        }
        self.entity_anim.retain(|id, _| alive.contains(id));

        let white = self.atlas.white_uv();
        let dims = [
            self.entity_atlas.width as f32,
            self.entity_atlas.height as f32,
        ];
        let (mut box_v, mut model_v, mut item_v) = (Vec::new(), Vec::new(), Vec::new());
        for (&id, e) in entities.iter() {
            let a = self.entity_anim[&id];
            // Falling Block's Spawn Entity data is a global block-state ID.
            // Render it at full block scale instead of using a generic entity
            // bounds box; gravity/position remain server-authoritative.
            if let Some(block_name) = e.block_state.and_then(crab_registry::block_name) {
                box_v.extend(block_item_mesh(
                    &self.atlas,
                    block_name,
                    [a.pos[0], a.pos[1] + 0.5, a.pos[2]],
                    1.0,
                    0.0,
                ));
                continue;
            }
            // Dropped item: a camera-facing billboard of its icon.
            if let Some(item_id) = e.item {
                if let Some(name) = u32::try_from(item_id)
                    .ok()
                    .and_then(crab_registry::item_name)
                {
                    if crab_registry::block_by_name(name).is_some() {
                        box_v.extend(block_item_mesh(
                            &self.atlas,
                            name,
                            [
                                a.pos[0],
                                a.pos[1] + 0.18 + (a.age * 2.0).sin() * 0.05,
                                a.pos[2],
                            ],
                            0.36,
                            a.age * 45.0,
                        ));
                        continue;
                    }
                    if let Some(uv) = self.item_atlas.icon(name) {
                        push_item_billboard(&mut item_v, a.pos, uv, self.yaw);
                        continue;
                    }
                }
            }
            if !e.invisible || e.glowing {
                if let Some(m) = self.entity_atlas.models.get(&e.type_id) {
                    let hurt_wobble = if a.hurt_time > 0.0 {
                        (a.hurt_time * 45.0).sin() * 8.0
                    } else {
                        0.0
                    };
                    model_v.extend(entity_mesh_with_pose(
                        &m.geo,
                        a.pos,
                        [m.atlas_x, m.atlas_y],
                        dims,
                        a.phase,
                        a.amount,
                        e.scale,
                        a.yaw + hurt_wobble,
                        a.head_yaw,
                        (a.swing_time / 0.35).clamp(0.0, 1.0),
                        e.pose,
                    ));
                } else {
                    let hw = e.half_width;
                    let min = [a.pos[0] - hw, a.pos[1], a.pos[2] - hw];
                    let max = [a.pos[0] + hw, a.pos[1] + e.height, a.pos[2] + hw];
                    box_v.extend(box_mesh(min, max, white, box_color(e.type_id)));
                }
            }
            if let Some(item_id) = e.equipment[0] {
                if let Some(uv) = u32::try_from(item_id)
                    .ok()
                    .and_then(crab_registry::item_name)
                    .and_then(|name| self.item_atlas.icon(name))
                {
                    push_item_billboard(
                        &mut item_v,
                        [
                            a.pos[0] + e.half_width,
                            a.pos[1] + e.height * 0.65,
                            a.pos[2],
                        ],
                        uv,
                        self.yaw,
                    );
                }
            }
            if let Some(item_id) = e.equipment[1] {
                if let Some(uv) = u32::try_from(item_id)
                    .ok()
                    .and_then(crab_registry::item_name)
                    .and_then(|name| self.item_atlas.icon(name))
                {
                    push_item_billboard(
                        &mut item_v,
                        [
                            a.pos[0] - e.half_width,
                            a.pos[1] + e.height * 0.65,
                            a.pos[2],
                        ],
                        uv,
                        self.yaw,
                    );
                }
            }
            for (slot, y0, y1, inset) in [
                (2usize, 0.0, 0.24, 0.45),
                (3, 0.22, 0.62, 0.25),
                (4, 0.58, 0.88, 0.12),
                (5, 0.78, 1.02, 0.18),
            ] {
                let Some(item_id) = e.equipment[slot] else {
                    continue;
                };
                let Some(name) = u32::try_from(item_id)
                    .ok()
                    .and_then(crab_registry::item_name)
                else {
                    continue;
                };
                let radius = e.half_width * (1.0 - inset);
                box_v.extend(box_mesh(
                    [
                        a.pos[0] - radius,
                        a.pos[1] + e.height * y0,
                        a.pos[2] - radius,
                    ],
                    [
                        a.pos[0] + radius,
                        a.pos[1] + e.height * y1,
                        a.pos[2] + radius,
                    ],
                    white,
                    armour_color(name),
                ));
            }
        }
        drop(entities);
        for particle in self.shared.particles.lock().unwrap().iter() {
            let size = 0.035;
            box_v.extend(box_mesh(
                [
                    particle.position[0] - size,
                    particle.position[1] - size,
                    particle.position[2] - size,
                ],
                [
                    particle.position[0] + size,
                    particle.position[1] + size,
                    particle.position[2] + size,
                ],
                white,
                particle.color,
            ));
        }
        (box_v, model_v, item_v)
    }

    /// Builds the destroy-stage crack overlay cube for the block currently being
    /// mined (slightly inflated to sit just outside the block's faces), or an
    /// empty mesh when not digging / no crack atlas is loaded.
    fn crack_mesh(&self) -> Vec<Vertex> {
        if self
            .gfx
            .as_ref()
            .is_none_or(|g| g.crack_bind_group.is_none())
        {
            return Vec::new();
        }
        let Some(d) = *self.shared.dig.lock().unwrap() else {
            return Vec::new();
        };
        let n = f32::from(d.stage.min(9));
        let (v0, v1) = (n / 10.0, (n + 1.0) / 10.0);
        let e = 0.003; // inflate to avoid z-fighting with the block faces
        let min = [
            d.block[0] as f32 - e,
            d.block[1] as f32 - e,
            d.block[2] as f32 - e,
        ];
        let max = [
            d.block[0] as f32 + 1.0 + e,
            d.block[1] as f32 + 1.0 + e,
            d.block[2] as f32 + 1.0 + e,
        ];
        box_mesh(min, max, [0.0, v0, 1.0, v1], [1.0, 1.0, 1.0])
    }

    fn server_container_open(&self) -> bool {
        self.shared.container.lock().unwrap().is_some()
    }

    fn anvil_open(&self) -> bool {
        self.shared
            .container
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|container| container.menu_type == 7)
    }

    fn item_screen_open(&self) -> bool {
        self.inventory_open || self.server_container_open() || self.book.is_some()
    }

    fn open_held_book(&mut self) -> bool {
        use crab_protocol::nbt::Nbt;

        let hotbar_slot = self.selected_slot as usize;
        let inventory_slot = 36 + hotbar_slot;
        let item = self
            .shared
            .inventory
            .lock()
            .unwrap()
            .get(inventory_slot)
            .copied()
            .flatten();
        let Some(name) = item
            .and_then(|item| u32::try_from(item.item_id).ok())
            .and_then(crab_registry::item_name)
            .filter(|name| matches!(*name, "writable_book" | "written_book"))
        else {
            return false;
        };
        let metadata = self
            .shared
            .inventory_nbt
            .lock()
            .unwrap()
            .get(inventory_slot)
            .cloned()
            .flatten();
        let mut pages = match metadata.as_ref().and_then(|nbt| nbt.get("pages")) {
            Some(Nbt::List(pages)) => pages
                .iter()
                .filter_map(|page| match page {
                    Nbt::String(text) if name == "written_book" => {
                        Some(crate::client::plain_text(text))
                    }
                    Nbt::String(text) => Some(text.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        if pages.is_empty() {
            pages.push(String::new());
        }
        let string_tag = |key| match metadata.as_ref().and_then(|nbt| nbt.get(key)) {
            Some(Nbt::String(value)) => Some(value.clone()),
            _ => None,
        };
        self.book = Some(BookScreen {
            pages,
            page: 0,
            writable: name == "writable_book",
            dirty: false,
            hotbar_slot: hotbar_slot as i32,
            title: string_tag("title"),
            author: string_tag("author"),
            signing: false,
            sign_title: String::new(),
            submitted_title: None,
        });
        self.set_cursor_free(true);
        true
    }

    fn finish_book(&mut self) {
        let Some(book) = self.book.take() else {
            return;
        };
        if book.writable && book.dirty {
            self.shared.edit_book_outbox.lock().unwrap().push(
                crab_protocol::versions::v1_20_1::play::EditBook {
                    slot: book.hotbar_slot,
                    pages: book.pages,
                    title: book.submitted_title,
                },
            );
        }
        self.set_cursor_free(false);
    }

    fn book_click(&mut self) {
        let Some(book) = self.book.as_mut() else {
            return;
        };
        let aspect = self.gfx.as_ref().map_or(1.0, Graphics::aspect);
        let indices: &[usize] = if book.signing {
            &[4, 5]
        } else if book.writable {
            &[0, 1, 2, 3]
        } else {
            &[0, 1, 3]
        };
        let mut finish = false;
        for &index in indices {
            let rect = book_button_rect(aspect, index);
            if self.cursor.0 < rect.0
                || self.cursor.0 > rect.2
                || self.cursor.1 < rect.1
                || self.cursor.1 > rect.3
            {
                continue;
            }
            match index {
                0 => book.page = book.page.saturating_sub(1),
                1 if book.page + 1 < book.pages.len() => book.page += 1,
                1 if book.writable && book.pages.len() < 100 => {
                    book.pages.push(String::new());
                    book.page += 1;
                    book.dirty = true;
                }
                2 if book.writable => book.signing = true,
                3 => finish = true,
                4 if !book.sign_title.trim().is_empty() => {
                    book.submitted_title = Some(book.sign_title.trim().to_string());
                    book.dirty = true;
                    finish = true;
                }
                5 => book.signing = false,
                _ => {}
            }
            break;
        }
        if finish {
            self.finish_book();
        }
    }

    fn resource_pack_prompt(&self) -> Option<crate::client::ResourcePackRequest> {
        if !*self.shared.resource_pack_prompt_open.lock().unwrap() {
            return None;
        }
        self.shared.resource_pack_request.lock().unwrap().clone()
    }

    fn choose_resource_pack(&self, accepted: bool) {
        self.shared
            .resource_pack_outbox
            .lock()
            .unwrap()
            .push(accepted);
        *self.shared.resource_pack_prompt_open.lock().unwrap() = false;
    }

    fn resource_pack_click(&self) {
        let aspect = self.gfx.as_ref().map_or(1.0, Graphics::aspect);
        let (cx, cy) = self.cursor;
        for index in 0..2 {
            let rect = crab_render::menu_button_rect(aspect, index, 2);
            if cx >= rect.0 && cx <= rect.2 && cy >= rect.1 && cy <= rect.3 {
                self.choose_resource_pack(index == 0);
                return;
            }
        }
    }

    fn crafting_recipe_context(&self) -> Option<RecipeContext> {
        let aspect = self.gfx.as_ref().map_or(1.0, Graphics::aspect);
        if self.inventory_open && !self.server_container_open() {
            return Some(RecipeContext {
                panel: crab_render::inventory_rect(aspect),
                window_id: 0,
                grid: 2,
            });
        }
        self.shared
            .container
            .lock()
            .unwrap()
            .as_ref()
            .filter(|container| container.menu_type == 11)
            .and_then(|container| {
                i8::try_from(container.window_id)
                    .ok()
                    .map(|window_id| RecipeContext {
                        panel: crab_render::simple_container_rect("crafting_table", aspect),
                        window_id,
                        grid: 3,
                    })
            })
    }

    fn visible_crafting_recipes(&self, grid: usize) -> Vec<CraftingRecipe> {
        let unlocked = self.shared.unlocked_recipes.lock().unwrap();
        self.shared
            .crafting_recipes
            .lock()
            .unwrap()
            .iter()
            .filter(|recipe| {
                (unlocked.is_empty() || unlocked.contains(&recipe.id))
                    && if recipe.width == 0 {
                        recipe.ingredients.len() <= grid * grid
                    } else {
                        usize::from(recipe.width) <= grid && usize::from(recipe.height) <= grid
                    }
            })
            .cloned()
            .collect()
    }

    /// Hit-tests the cursor against the active item screen and queues a click.
    fn inventory_click(&mut self, button: i8) {
        let aspect = self.gfx.as_ref().map_or(1.0, Graphics::aspect);
        let (cx, cy) = self.cursor;
        let shift =
            self.keys.contains(&KeyCode::ShiftLeft) || self.keys.contains(&KeyCode::ShiftRight);
        if let Some(context) = self.crafting_recipe_context() {
            let toggle = recipe_book_toggle_rect(context.panel, aspect);
            if cx >= toggle.0 && cx <= toggle.2 && cy >= toggle.1 && cy <= toggle.3 {
                self.recipe_book_open = !self.recipe_book_open;
                self.recipe_book_page = 0;
                return;
            }
            if self.recipe_book_open {
                let recipes = self.visible_crafting_recipes(context.grid);
                let rect = recipe_book_panel_rect(context.panel, aspect);
                for visible in 0..20 {
                    let index = self.recipe_book_page * 20 + visible;
                    let Some(recipe) = recipes.get(index) else {
                        break;
                    };
                    let cell = recipe_book_cell_rect(rect, visible);
                    if cx >= cell.0 && cx <= cell.2 && cy >= cell.1 && cy <= cell.3 {
                        self.shared
                            .place_recipe_outbox
                            .lock()
                            .unwrap()
                            .push(PlaceRecipe {
                                window_id: context.window_id,
                                recipe: recipe.id.clone(),
                                make_all: shift,
                            });
                        return;
                    }
                }
            }
        }
        if let Some(container) = self.shared.container.lock().unwrap().as_ref() {
            let rows = container.generic_rows();
            let rect = if let Some(rows) = rows {
                crab_render::container_rect(rows, aspect)
            } else if container.furnace_texture().is_some() {
                crab_render::inventory_rect(aspect)
            } else if let Some(texture) = container.simple_container_texture() {
                crab_render::simple_container_rect(texture, aspect)
            } else {
                return;
            };
            if matches!(container.menu_type, 17 | 23) {
                let texture = if container.menu_type == 17 {
                    "loom"
                } else {
                    "stonecutter"
                };
                let per_page = if texture == "loom" { 16 } else { 12 };
                let total = if texture == "loom" {
                    34
                } else {
                    applicable_stonecutter_recipes(&self.shared, container).len()
                };
                let page = self.recipe_page.min(total.saturating_sub(1) / per_page);
                for visible in 0..per_page {
                    let index = page * per_page + visible;
                    if index >= total {
                        continue;
                    }
                    let (x0, y0, x1, y1) = recipe_button_rect(texture, page, visible, aspect);
                    if cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1 {
                        if let Ok(button_id) = i8::try_from(index) {
                            self.shared
                                .menu_button_outbox
                                .lock()
                                .unwrap()
                                .push((container.window_id, button_id));
                        }
                        return;
                    }
                }
            }
            if container.menu_type == 12 {
                for option in 0..3 {
                    let (x0, y0, x1, y1) = crab_render::enchantment_option_rect(rect, option);
                    if cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1 {
                        self.shared
                            .enchant_outbox
                            .lock()
                            .unwrap()
                            .push((container.window_id, option as i8));
                        return;
                    }
                }
            }
            for slot in 0..container.slots.len() {
                let (x0, y0, x1, y1) = if let Some(rows) = rows {
                    crab_render::container_slot_rect(rect, rows, slot)
                } else if let Some(texture) = container.simple_container_texture() {
                    crab_render::simple_container_slot_rect(rect, texture, slot)
                } else {
                    crab_render::furnace_slot_rect(rect, slot)
                };
                if cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1 {
                    self.shared.click_outbox.lock().unwrap().push((
                        container.window_id,
                        slot as i16,
                        button,
                        i32::from(shift),
                    ));
                    return;
                }
            }
            return;
        }
        let rect = crab_render::inventory_rect(aspect);
        for slot in 0..46usize {
            let (x0, y0, x1, y1) = crab_render::inventory_slot_rect(rect, slot);
            if cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1 {
                self.shared.click_outbox.lock().unwrap().push((
                    0,
                    slot as i16,
                    button,
                    i32::from(shift),
                ));
                return;
            }
        }
    }

    fn hovered_item(&self) -> Option<crab_protocol::versions::v1_20_1::play::SlotItem> {
        if let (Some((slot, items)), Some((selected_slot, selected))) =
            (self.hovered_bundle(), self.bundle_selection)
        {
            if slot == selected_slot {
                if let Ok(index) = usize::try_from(selected) {
                    if let Some(item) = items.get(index) {
                        return Some(*item);
                    }
                }
            }
        }
        let aspect = self.gfx.as_ref().map_or(1.0, Graphics::aspect);
        let (cx, cy) = self.cursor;
        if let Some(container) = self.shared.container.lock().unwrap().as_ref() {
            let rows = container.generic_rows();
            let rect = if let Some(rows) = rows {
                crab_render::container_rect(rows, aspect)
            } else if container.furnace_texture().is_some() {
                crab_render::inventory_rect(aspect)
            } else {
                let texture = container.simple_container_texture()?;
                crab_render::simple_container_rect(texture, aspect)
            };
            for (slot, item) in container.slots.iter().enumerate() {
                let bounds = if let Some(rows) = rows {
                    crab_render::container_slot_rect(rect, rows, slot)
                } else if let Some(texture) = container.simple_container_texture() {
                    crab_render::simple_container_slot_rect(rect, texture, slot)
                } else {
                    crab_render::furnace_slot_rect(rect, slot)
                };
                if cx >= bounds.0 && cx <= bounds.2 && cy >= bounds.1 && cy <= bounds.3 {
                    return *item;
                }
            }
            return None;
        }
        if self.inventory_open {
            let rect = crab_render::inventory_rect(aspect);
            let inventory = self.shared.inventory.lock().unwrap();
            for (slot, item) in inventory.iter().enumerate() {
                let bounds = crab_render::inventory_slot_rect(rect, slot);
                if cx >= bounds.0 && cx <= bounds.2 && cy >= bounds.1 && cy <= bounds.3 {
                    return *item;
                }
            }
        }
        None
    }

    /// Returns the visible slot id and decoded contents of a hovered 1.21.2+
    /// bundle. Bundle contents are retained as synthetic metadata by the
    /// protocol decoder so the rendering/input layer stays version-agnostic.
    fn hovered_bundle(
        &self,
    ) -> Option<(i32, Vec<crab_protocol::versions::v1_20_1::play::SlotItem>)> {
        let aspect = self.gfx.as_ref().map_or(1.0, Graphics::aspect);
        let (cx, cy) = self.cursor;
        if let Some(container) = self.shared.container.lock().unwrap().as_ref() {
            let rows = container.generic_rows();
            let rect = if let Some(rows) = rows {
                crab_render::container_rect(rows, aspect)
            } else if container.furnace_texture().is_some() {
                crab_render::inventory_rect(aspect)
            } else {
                crab_render::simple_container_rect(container.simple_container_texture()?, aspect)
            };
            for slot in 0..container.slots.len() {
                let bounds = if let Some(rows) = rows {
                    crab_render::container_slot_rect(rect, rows, slot)
                } else if let Some(texture) = container.simple_container_texture() {
                    crab_render::simple_container_slot_rect(rect, texture, slot)
                } else {
                    crab_render::furnace_slot_rect(rect, slot)
                };
                if cx >= bounds.0 && cx <= bounds.2 && cy >= bounds.1 && cy <= bounds.3 {
                    let items =
                        bundle_items(container.slot_metadata.get(slot).and_then(Option::as_ref));
                    return (!items.is_empty()).then_some((i32::try_from(slot).ok()?, items));
                }
            }
            return None;
        }
        if self.inventory_open {
            let rect = crab_render::inventory_rect(aspect);
            let metadata = self.shared.inventory_nbt.lock().unwrap();
            for slot in 0..metadata.len() {
                let bounds = crab_render::inventory_slot_rect(rect, slot);
                if cx >= bounds.0 && cx <= bounds.2 && cy >= bounds.1 && cy <= bounds.3 {
                    let items = bundle_items(metadata.get(slot).and_then(Option::as_ref));
                    return (!items.is_empty()).then_some((i32::try_from(slot).ok()?, items));
                }
            }
        }
        None
    }

    /// Frees the cursor (for a UI) or recaptures it (for gameplay).
    fn set_cursor_free(&self, free: bool) {
        if let Some(gfx) = self.gfx.as_ref() {
            if free {
                let _ = gfx.window.set_cursor_grab(CursorGrabMode::None);
                gfx.window.set_cursor_visible(true);
            } else {
                let _ = gfx
                    .window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| gfx.window.set_cursor_grab(CursorGrabMode::Confined));
                gfx.window.set_cursor_visible(false);
            }
        }
    }

    /// Handles a click in the pause menu: returns true if it should quit.
    fn menu_click(&mut self, event_loop: &ActiveEventLoop) {
        let aspect = self.gfx.as_ref().map_or(1.0, Graphics::aspect);
        let (cx, cy) = self.cursor;
        if self.options_open {
            for i in 0..4 {
                let (x0, y0, x1, y1) = crab_render::menu_button_rect(aspect, i, 4);
                if cx < x0 || cx > x1 || cy < y0 || cy > y1 {
                    continue;
                }
                match i {
                    0 => self.fov_degrees = next_choice(self.fov_degrees, &FOV_CHOICES),
                    1 => {
                        self.mouse_sensitivity =
                            next_choice(self.mouse_sensitivity, &SENSITIVITY_CHOICES);
                    }
                    2 => {
                        if let Some(gfx) = self.gfx.as_ref() {
                            let fullscreen = if gfx.window.fullscreen().is_some() {
                                None
                            } else {
                                Some(Fullscreen::Borderless(None))
                            };
                            gfx.window.set_fullscreen(fullscreen);
                        }
                    }
                    _ => self.options_open = false,
                }
                return;
            }
            return;
        }
        let buttons: &[&str] = if self.controls_help_open {
            &DONE_BUTTON
        } else {
            &MENU_BUTTONS
        };
        for (i, _) in buttons.iter().enumerate() {
            let (x0, y0, x1, y1) = crab_render::menu_button_rect(aspect, i, buttons.len());
            if cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1 {
                if self.controls_help_open {
                    self.controls_help_open = false;
                    return;
                }
                match i {
                    0 => {
                        self.menu_open = false;
                        self.set_cursor_free(false);
                    }
                    1 => self.options_open = true,
                    2 => self.controls_help_open = true,
                    _ => event_loop.exit(),
                }
                return;
            }
        }
    }

    /// Opens/closes the inventory, freeing or recapturing the cursor.
    fn set_inventory_open(&mut self, open: bool) {
        self.inventory_open = open;
        self.set_cursor_free(open);
    }

    fn close_item_screen(&mut self) {
        if self.book.is_some() {
            self.finish_book();
            return;
        }
        let server_id = self
            .shared
            .container
            .lock()
            .unwrap()
            .take()
            .map(|c| c.window_id);
        if let Some(window_id) = server_id {
            self.shared
                .close_container_outbox
                .lock()
                .unwrap()
                .push(window_id);
            *self.shared.carried.lock().unwrap() = None;
            self.container_seen = false;
            self.set_cursor_free(false);
        } else {
            self.set_inventory_open(false);
        }
    }

    /// Updates look from arrow keys and publishes movement intent to the shared
    /// `Controls` for the network thread to apply via physics.
    fn update_input(&mut self, dt: f32) {
        // While a UI is open, freeze movement/look input.
        if self.item_screen_open()
            || self.chat_open
            || self.menu_open
            || self.resource_pack_prompt().is_some()
        {
            self.flight_toggle_pending = false;
            self.swap_hands_pending = false;
            let mut controls = self.shared.controls.lock().unwrap();
            controls.forward = 0.0;
            controls.strafe = 0.0;
            controls.jump = false;
            controls.sprint = false;
            controls.sneak = false;
            controls.attack = false;
            controls.use_item = false;
            controls.toggle_flight = false;
            controls.swap_hands = false;
            return;
        }
        let pressed = |c: KeyCode| self.keys.contains(&c);
        if pressed(KeyCode::ArrowLeft) {
            self.yaw -= LOOK_SPEED * dt;
        }
        if pressed(KeyCode::ArrowRight) {
            self.yaw += LOOK_SPEED * dt;
        }
        if pressed(KeyCode::ArrowUp) {
            self.pitch = (self.pitch - LOOK_SPEED * dt).clamp(-89.0, 89.0);
        }
        if pressed(KeyCode::ArrowDown) {
            self.pitch = (self.pitch + LOOK_SPEED * dt).clamp(-89.0, 89.0);
        }

        // Number keys 1..9 select a hotbar slot.
        const DIGITS: [KeyCode; 9] = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ];
        for (i, key) in DIGITS.iter().enumerate() {
            if pressed(*key) {
                self.selected_slot = i as u8;
            }
        }

        let axis = |pos: KeyCode, neg: KeyCode| (pressed(pos) as i32 - pressed(neg) as i32) as f32;
        let mut controls = self.shared.controls.lock().unwrap();
        controls.forward = axis(KeyCode::KeyW, KeyCode::KeyS);
        controls.strafe = axis(KeyCode::KeyD, KeyCode::KeyA);
        controls.jump = pressed(KeyCode::Space);
        controls.sprint = pressed(KeyCode::ControlLeft) || pressed(KeyCode::ControlRight);
        controls.sneak = pressed(KeyCode::ShiftLeft) || pressed(KeyCode::ShiftRight);
        if std::mem::take(&mut self.flight_toggle_pending) {
            controls.toggle_flight = true;
        }
        if std::mem::take(&mut self.swap_hands_pending) {
            controls.swap_hands = true;
        }
        controls.yaw = self.yaw;
        controls.pitch = self.pitch;
        controls.selected_slot = self.selected_slot;
    }

    /// Uploads chunk meshes finished by the background mesher (GPU upload only;
    /// the actual meshing happens off the render thread, so frames stay smooth).
    fn process_meshes(&mut self) {
        let Some(gfx) = self.gfx.as_mut() else {
            return;
        };
        for _ in 0..REMESH_BUDGET {
            match self.mesh_rx.try_recv() {
                Ok((coord, verts)) => gfx.upload_chunk(coord, &verts),
                Err(_) => break,
            }
        }
    }

    /// Applies a freshly downloaded layered pack to every live atlas. The
    /// server is acknowledged only after these CPU models and GPU textures are
    /// installed; failures produce status 2 and leave the previous assets live.
    fn apply_pending_resource_pack(&mut self) {
        let pending = self.shared.cached_resource_pack.lock().unwrap().clone();
        let Some(path) = pending.filter(|path| self.applied_resource_pack.as_ref() != Some(path))
        else {
            return;
        };
        self.applied_resource_pack = Some(path.clone());

        let result = (|| -> anyhow::Result<()> {
            let block_names: Vec<String> = crab_registry::blocks()
                .iter()
                .map(|block| block.name.to_owned())
                .collect();
            let item_names: Vec<String> = crab_registry::items()
                .iter()
                .map(|item| item.name.to_owned())
                .collect();
            let atlas = Arc::new(crab_assets::load_block_atlas(&path, &block_names)?);
            let item_atlas = Arc::new(crab_assets::load_item_atlas(&path, &item_names)?);
            let gui_atlas = Arc::new(crab_assets::load_gui_atlas(&path)?);
            let crack = crab_assets::load_destroy_stages(&path);
            let entity_atlas = load_resource_entity_atlas(&path)
                .map(Arc::new)
                .unwrap_or_else(|| Arc::clone(&self.entity_atlas));

            if let Some(gfx) = self.gfx.as_mut() {
                gfx.reload_assets(
                    &atlas,
                    &entity_atlas,
                    &item_atlas,
                    &gui_atlas,
                    crack.as_ref(),
                );
            }
            *self.atlas_slot.write().unwrap() = Arc::clone(&atlas);
            self.atlas = atlas;
            self.entity_atlas = entity_atlas;
            self.item_atlas = item_atlas;
            self.gui_atlas = gui_atlas;
            self.crack = crack;

            let chunks: Vec<_> = self.shared.world.lock().unwrap().chunk_coords().collect();
            self.shared.dirty_chunks.lock().unwrap().extend(chunks);
            Ok(())
        })();

        let status = match result {
            Ok(()) => {
                tracing::info!(pack = %path.display(), "applied server resource pack live");
                0
            }
            Err(error) => {
                tracing::warn!(%error, pack = %path.display(), "resource-pack reload failed");
                2
            }
        };
        if self
            .shared
            .resource_pack_reload_ack
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.shared
                .resource_pack_status_outbox
                .lock()
                .unwrap()
                .push(status);
        }
    }
}

fn load_resource_entity_atlas(path: &std::path::Path) -> Option<EntityAtlas> {
    let mut models = PathBuf::from(std::env::var_os("CRABCRAFT_ENTITY_MODELS")?);
    if !models.join("cow.geo.json").exists() {
        for sub in ["resource_pack/models/entity", "models/entity", "entity"] {
            if models.join(sub).join("cow.geo.json").exists() {
                models = models.join(sub);
                break;
            }
        }
    }
    let types: Vec<(i32, String)> = crab_registry::entities()
        .iter()
        .map(|entity| (entity.id as i32, entity.name.to_owned()))
        .collect();
    Some(crab_assets::load_entity_atlas(path, &models, &types))
}

/// Background thread: drains dirty chunks, meshes them (skipping air-only
/// sections), and ships the vertices to the render thread. Keeps the heavy
/// meshing work off the frame loop.
fn mesher_loop(
    shared: Arc<Shared>,
    atlas_slot: Arc<RwLock<Arc<Atlas>>>,
    tx: Sender<((i32, i32), Vec<Vertex>)>,
) {
    while shared.running.load(Ordering::SeqCst) {
        let batch: Vec<(i32, i32)> = {
            let mut dirty = shared.dirty_chunks.lock().unwrap();
            let take: Vec<_> = dirty.iter().take(8).copied().collect();
            for c in &take {
                dirty.remove(c);
            }
            take
        };
        if batch.is_empty() {
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }
        for (cx, cz) in batch {
            let atlas = Arc::clone(&atlas_slot.read().unwrap());
            let verts = {
                let world = shared.world.lock().unwrap();
                match world.occupied_y_bounds(cx, cz) {
                    Some((min_y, max_y)) => {
                        mesh_region(
                            &world,
                            &atlas,
                            [cx * 16, min_y, cz * 16],
                            [cx * 16 + 15, max_y, cz * 16 + 15],
                        )
                        .vertices
                    }
                    None => Vec::new(),
                }
            };
            if tx.send(((cx, cz), verts)).is_err() {
                return;
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Crabcraft")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        // Capture the cursor for mouse-look (Locked on macOS; Confined elsewhere).
        let _ = window
            .set_cursor_grab(CursorGrabMode::Locked)
            .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
        window.set_cursor_visible(false);
        self.gfx = Some(Graphics::new(
            window,
            &self.atlas,
            &self.entity_atlas,
            &self.item_atlas,
            &self.gui_atlas,
            self.crack.as_ref(),
        ));
        self.last_frame = Instant::now();
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if self.item_screen_open() || self.chat_open || self.menu_open {
            return; // don't turn the view while a UI is focused
        }
        if let DeviceEvent::MouseMotion { delta } = event {
            self.yaw += delta.0 as f32 * self.mouse_sensitivity;
            self.pitch = (self.pitch + delta.1 as f32 * self.mouse_sensitivity).clamp(-89.0, 89.0);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gfx) = self.gfx.as_mut() {
                    gfx.resize(size.width, size.height);
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                // The framebuffer size changes with the DPI scale; adopt the new
                // physical inner size so the surface aspect stays correct.
                if let Some(gfx) = self.gfx.as_mut() {
                    let size = gfx.window.inner_size();
                    gfx.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat,
                        text,
                        ..
                    },
                ..
            } => {
                let pressed = state == ElementState::Pressed;
                // Chat input swallows all keys while open.
                if self.chat_open {
                    if pressed {
                        match code {
                            KeyCode::Escape => {
                                self.chat_open = false;
                                self.chat_buffer.clear();
                            }
                            KeyCode::Enter | KeyCode::NumpadEnter => {
                                let msg = std::mem::take(&mut self.chat_buffer);
                                if !msg.trim().is_empty() {
                                    self.shared.chat_outbox.lock().unwrap().push(msg);
                                }
                                self.chat_open = false;
                            }
                            KeyCode::Backspace => {
                                self.chat_buffer.pop();
                            }
                            _ => {
                                if let Some(t) = &text {
                                    for ch in t.chars() {
                                        if !ch.is_control() && self.chat_buffer.len() < 200 {
                                            self.chat_buffer.push(ch);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return;
                }
                if self.book.is_some() {
                    if !pressed {
                        return;
                    }
                    if code == KeyCode::Escape {
                        if self.book.as_ref().is_some_and(|book| book.signing) {
                            self.book.as_mut().unwrap().signing = false;
                        } else {
                            self.finish_book();
                        }
                        return;
                    }
                    let book = self.book.as_mut().unwrap();
                    if book.signing {
                        match code {
                            KeyCode::Backspace => {
                                book.sign_title.pop();
                            }
                            KeyCode::Enter | KeyCode::NumpadEnter
                                if !book.sign_title.trim().is_empty() =>
                            {
                                book.submitted_title = Some(book.sign_title.trim().to_string());
                                book.dirty = true;
                            }
                            _ => {
                                if let Some(input) = &text {
                                    for ch in input.chars().filter(|ch| !ch.is_control()) {
                                        if book.sign_title.chars().count() >= 16 {
                                            break;
                                        }
                                        book.sign_title.push(ch);
                                    }
                                }
                            }
                        }
                        if book.submitted_title.is_some() {
                            self.finish_book();
                        }
                        return;
                    }
                    match code {
                        KeyCode::ArrowLeft | KeyCode::PageUp => {
                            book.page = book.page.saturating_sub(1);
                        }
                        KeyCode::ArrowRight | KeyCode::PageDown => {
                            if book.page + 1 < book.pages.len() {
                                book.page += 1;
                            } else if book.writable && book.pages.len() < 100 {
                                book.pages.push(String::new());
                                book.page += 1;
                                book.dirty = true;
                            }
                        }
                        KeyCode::Backspace if book.writable => {
                            if book.pages[book.page].pop().is_some() {
                                book.dirty = true;
                            }
                        }
                        KeyCode::Enter | KeyCode::NumpadEnter if book.writable => {
                            if book.pages[book.page].chars().count() < 1_024 {
                                book.pages[book.page].push('\n');
                                book.dirty = true;
                            }
                        }
                        _ if book.writable => {
                            if let Some(input) = &text {
                                for ch in input.chars().filter(|ch| !ch.is_control()) {
                                    if book.pages[book.page].chars().count() >= 1_024 {
                                        break;
                                    }
                                    book.pages[book.page].push(ch);
                                    book.dirty = true;
                                }
                            }
                        }
                        _ => {}
                    }
                    return;
                }
                if self.anvil_open() && pressed && code != KeyCode::Escape {
                    let mut changed = false;
                    if code == KeyCode::Backspace {
                        changed = self.anvil_name.pop().is_some();
                    } else if let Some(t) = &text {
                        for ch in t.chars() {
                            if !ch.is_control() && self.anvil_name.chars().count() < 50 {
                                self.anvil_name.push(ch);
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        self.shared
                            .rename_outbox
                            .lock()
                            .unwrap()
                            .push(self.anvil_name.clone());
                    }
                    return;
                }
                if code == KeyCode::Escape && pressed && !repeat {
                    // Esc closes the inventory, else toggles the pause menu.
                    if self.resource_pack_prompt().is_some() {
                        self.choose_resource_pack(false);
                    } else if self.controls_help_open {
                        self.controls_help_open = false;
                    } else if self.options_open {
                        self.options_open = false;
                    } else if self.item_screen_open() {
                        self.close_item_screen();
                    } else {
                        self.menu_open = !self.menu_open;
                        self.set_cursor_free(self.menu_open);
                    }
                } else if code == KeyCode::Escape {
                    // ignore Esc release / auto-repeat
                } else if pressed {
                    if !repeat && code == KeyCode::F3 {
                        self.debug_open = !self.debug_open;
                        return;
                    }
                    if !repeat && code == KeyCode::F5 && !self.item_screen_open() {
                        self.perspective = self.perspective.next();
                        return;
                    }
                    if !self.item_screen_open()
                        && !self.menu_open
                        && !repeat
                        && code == KeyCode::KeyF
                    {
                        self.swap_hands_pending = true;
                        return;
                    }
                    if !self.item_screen_open()
                        && !self.menu_open
                        && !repeat
                        && code == KeyCode::Space
                    {
                        let now = Instant::now();
                        if self.last_space_press.is_some_and(|previous| {
                            now.duration_since(previous) <= Duration::from_millis(350)
                        }) {
                            self.flight_toggle_pending = true;
                            self.last_space_press = None;
                        } else {
                            self.last_space_press = Some(now);
                        }
                    }
                    if !self.item_screen_open() && !repeat && code == KeyCode::KeyT {
                        self.chat_open = true;
                        self.chat_buffer.clear();
                        return;
                    }
                    if !self.item_screen_open() && !repeat && code == KeyCode::Slash {
                        self.chat_open = true;
                        self.chat_buffer = "/".to_string();
                        return;
                    }
                    if code == KeyCode::KeyE && !repeat {
                        if self.server_container_open() {
                            self.close_item_screen();
                        } else {
                            let open = !self.inventory_open;
                            self.set_inventory_open(open);
                        }
                    }
                    if code == KeyCode::KeyR && !repeat && self.crafting_recipe_context().is_some()
                    {
                        self.recipe_book_open = !self.recipe_book_open;
                        self.recipe_book_page = 0;
                        return;
                    }
                    self.keys.insert(code);
                } else {
                    self.keys.remove(&code);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(gfx) = self.gfx.as_ref() {
                    let (w, h) = (
                        f64::from(gfx.config.width.max(1)),
                        f64::from(gfx.config.height.max(1)),
                    );
                    self.cursor = (
                        (position.x / w * 2.0 - 1.0) as f32,
                        (1.0 - position.y / h * 2.0) as f32,
                    );
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                if self.resource_pack_prompt().is_some() {
                    if pressed && button == MouseButton::Left {
                        self.resource_pack_click();
                    }
                    return;
                }
                // While the pause menu is open, clicks hit buttons.
                if self.menu_open {
                    if pressed && button == MouseButton::Left {
                        self.menu_click(event_loop);
                    }
                    return;
                }
                // While the inventory is open, clicks move items, not the world.
                if self.item_screen_open() {
                    if pressed {
                        if self.book.is_some() && button == MouseButton::Left {
                            self.book_click();
                        } else {
                            match button {
                                MouseButton::Left => self.inventory_click(0),
                                MouseButton::Right => self.inventory_click(1),
                                _ => {}
                            }
                        }
                    }
                    return;
                }
                if pressed && button == MouseButton::Right && self.open_held_book() {
                    return;
                }
                let mut controls = self.shared.controls.lock().unwrap();
                match button {
                    // Left mouse is held: attack stays true until release so the
                    // network thread can run hold-to-dig / continuous attack.
                    MouseButton::Left => controls.attack = pressed,
                    // Right mouse is held so food/bows/shields can finish or release.
                    MouseButton::Right => controls.use_item = pressed,
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                if self.item_screen_open() {
                    if let Some((slot, items)) = self.hovered_bundle() {
                        let count = i32::try_from(items.len()).unwrap_or(i32::MAX);
                        let selected = match self.bundle_selection {
                            Some((selected_slot, selected)) if selected_slot == slot => selected,
                            _ if dy > 0.01 => 1,
                            _ => -1,
                        };
                        let next = if dy > 0.01 {
                            (selected - 1).rem_euclid(count)
                        } else if dy < -0.01 {
                            (selected + 1).rem_euclid(count)
                        } else {
                            return;
                        };
                        self.bundle_selection = Some((slot, next));
                        self.shared
                            .bundle_selection_outbox
                            .lock()
                            .unwrap()
                            .push((slot, next));
                        return;
                    }
                    if self.recipe_book_open && self.crafting_recipe_context().is_some() {
                        let recipes = self
                            .visible_crafting_recipes(self.crafting_recipe_context().unwrap().grid);
                        let max_page = recipes.len().saturating_sub(1) / 20;
                        if dy > 0.01 {
                            self.recipe_book_page = self.recipe_book_page.saturating_sub(1);
                        } else if dy < -0.01 {
                            self.recipe_book_page = (self.recipe_book_page + 1).min(max_page);
                        }
                        return;
                    }
                    let max_page = match self.shared.container.lock().unwrap().as_ref() {
                        Some(container) if container.menu_type == 17 => 2,
                        Some(container) if container.menu_type == 23 => {
                            applicable_stonecutter_recipes(&self.shared, container)
                                .len()
                                .saturating_sub(1)
                                / 12
                        }
                        _ => return,
                    };
                    if dy > 0.01 {
                        self.recipe_page = self.recipe_page.saturating_sub(1);
                    } else if dy < -0.01 {
                        self.recipe_page = (self.recipe_page + 1).min(max_page);
                    }
                    return;
                }
                if self.chat_open || self.menu_open {
                    return;
                }
                // Scroll cycles the hotbar slot (up = previous, down = next).
                if dy.abs() > 0.01 {
                    let step = if dy > 0.0 { 8 } else { 1 }; // +8 == -1 (mod 9)
                    self.selected_slot = (self.selected_slot + step) % 9;
                }
            }
            WindowEvent::RedrawRequested => {
                // Adopt the server's spawn look the first time we're placed.
                let player = *self.shared.player.lock().unwrap();
                let environment = *self.shared.environment.lock().unwrap();
                let clear = sky_color(environment);
                let mut effect_ids: Vec<i32> = self
                    .shared
                    .effects
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(|(&id, effect)| effect.show_icon.then_some(id))
                    .collect();
                effect_ids.sort_unstable();
                let action_bar = self
                    .shared
                    .action_bar
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|(text, _)| text.clone());
                let title_overlay = self.shared.title.lock().unwrap().clone();
                let mut boss_bars: Vec<(String, crate::client::BossBarState)> = self
                    .shared
                    .boss_bars
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(id, bar)| (id.clone(), bar.clone()))
                    .collect();
                boss_bars.sort_by(|a, b| a.0.cmp(&b.0));
                let world_border = *self.shared.world_border.lock().unwrap();
                let scoreboard = self.shared.scoreboard.lock().unwrap().clone();
                let player_list = self
                    .keys
                    .contains(&KeyCode::Tab)
                    .then(|| self.shared.player_list.lock().unwrap().clone());
                let resource_pack_prompt = self.resource_pack_prompt();
                let book_screen = self.book.clone();
                if !self.look_init && player.spawned {
                    self.yaw = player.yaw;
                    self.pitch = player.pitch;
                    self.look_init = true;
                }

                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;
                let instant_fps = 1.0 / dt.max(0.001);
                self.smoothed_fps += (instant_fps - self.smoothed_fps) * 0.08;
                if player.selected_slot != self.last_named_slot {
                    self.last_named_slot = player.selected_slot;
                    self.selected_name_time = 2.0;
                } else {
                    self.selected_name_time = (self.selected_name_time - dt).max(0.0);
                }
                self.update_input(dt);
                let (attacking, using_item, moving) = {
                    let controls = self.shared.controls.lock().unwrap();
                    (
                        controls.attack,
                        controls.use_item,
                        controls.forward != 0.0 || controls.strafe != 0.0,
                    )
                };
                if (attacking && !self.was_attacking) || (using_item && !self.was_using_item) {
                    self.hand_swing_time = 0.32;
                } else {
                    self.hand_swing_time = (self.hand_swing_time - dt).max(0.0);
                }
                self.was_attacking = attacking;
                self.was_using_item = using_item;
                self.process_meshes();

                let (mut box_v, mut model_v, item_v) = self.step_entities(dt);
                let crack_v = self.crack_mesh();
                let hotbar = hotbar_icons(&self.shared, &self.item_atlas);
                let hovered_bundle = self.hovered_bundle();
                let hovered_item = self.hovered_item();
                let container = self.shared.container.lock().unwrap().clone();
                let inv_icons = (self.inventory_open && container.is_none())
                    .then(|| inventory_icons(&self.shared, &self.item_atlas));
                let container_icons = container
                    .as_ref()
                    .filter(|c| {
                        c.generic_rows().is_some()
                            || c.furnace_texture().is_some()
                            || c.simple_container_texture().is_some()
                    })
                    .map(|c| slot_icons(&c.slots, &self.item_atlas));
                let recipe_context = self.crafting_recipe_context();
                let crafting_recipes = recipe_context
                    .map(|context| self.visible_crafting_recipes(context.grid))
                    .unwrap_or_default();
                let selected = player.selected_slot as usize;
                let main_hand_icon =
                    inventory_item_icon(&self.shared, &self.item_atlas, 36 + selected);
                let offhand_icon = inventory_item_icon(&self.shared, &self.item_atlas, 45);
                let holding_filled_map = {
                    let inventory = self.shared.inventory.lock().unwrap();
                    [36 + selected, 45].into_iter().any(|slot| {
                        inventory
                            .get(slot)
                            .copied()
                            .flatten()
                            .and_then(|item| u32::try_from(item.item_id).ok())
                            .and_then(crab_registry::item_name)
                            == Some("filled_map")
                    })
                };
                let held_map = holding_filled_map
                    .then(|| *self.shared.latest_map.lock().unwrap())
                    .flatten()
                    .and_then(|map_id| self.shared.maps.lock().unwrap().get(&map_id).cloned());
                let hand_swing = if self.hand_swing_time > 0.0 {
                    1.0 - self.hand_swing_time / 0.32
                } else {
                    0.0
                };
                let show_first_person_items =
                    !self.item_screen_open() && self.perspective == Perspective::FirstPerson;
                let selected_name = (self.selected_name_time > 0.0)
                    .then(|| {
                        self.shared
                            .inventory
                            .lock()
                            .unwrap()
                            .get(36 + selected)
                            .copied()
                            .flatten()
                    })
                    .flatten()
                    .and_then(|item| {
                        u32::try_from(item.item_id)
                            .ok()
                            .and_then(crab_registry::item_name)
                            .map(pretty_item_name)
                    });
                // Smooth the camera toward the 20 Hz-stepped player position.
                let target_eye = Vec3::new(player.x as f32, player.y as f32, player.z as f32);
                let eye = match self.render_eye {
                    Some(prev) if player.spawned => {
                        prev + (target_eye - prev) * (1.0 - (-dt * 22.0).exp())
                    }
                    _ => target_eye,
                };
                self.render_eye = Some(eye);
                if self.perspective != Perspective::FirstPerson {
                    self.local_walk_phase += dt * if moving { 8.0 } else { 2.0 };
                    let player_model = crab_registry::entities()
                        .iter()
                        .find(|entity| entity.name == "player")
                        .and_then(|entity| self.entity_atlas.models.get(&(entity.id as i32)));
                    if let Some(model) = player_model {
                        let pose = if player.gliding {
                            4
                        } else if player.swimming {
                            3
                        } else if player.sneaking {
                            5
                        } else {
                            0
                        };
                        model_v.extend(entity_mesh_with_pose(
                            &model.geo,
                            [eye.x, eye.y, eye.z],
                            [model.atlas_x, model.atlas_y],
                            [
                                self.entity_atlas.width as f32,
                                self.entity_atlas.height as f32,
                            ],
                            self.local_walk_phase,
                            if moving { 0.65 } else { 0.0 },
                            1.0,
                            self.yaw,
                            self.yaw,
                            hand_swing,
                            pose,
                        ));
                    }
                }
                box_v.extend(celestial_mesh(eye, environment, self.atlas.white_uv()));
                if let Some(gfx) = self.gfx.as_mut() {
                    // Reconcile the surface with the window's real drawable size
                    // every frame. winit/macOS can deliver a stale initial size
                    // (or miss a scale-factor change), leaving the surface at a
                    // different aspect than the framebuffer -> a stretched/squished
                    // HUD and world. Re-configuring when they differ keeps
                    // `aspect()` matching what's actually on screen.
                    let real = gfx.window.inner_size();
                    if real.width != 0
                        && real.height != 0
                        && (real.width != gfx.config.width || real.height != gfx.config.height)
                    {
                        gfx.resize(real.width, real.height);
                    }
                    let aspect = gfx.aspect();
                    gfx.set_inventory_player(&[], None, None);
                    let eye_height = if player.swimming || player.gliding {
                        0.4
                    } else if player.sneaking {
                        1.27
                    } else {
                        EYE_HEIGHT
                    };
                    let camera = first_person_camera(
                        eye,
                        self.yaw,
                        self.pitch,
                        aspect,
                        eye_height,
                        self.fov_degrees,
                        self.perspective,
                    );
                    gfx.box_entity_buffer = gfx.make_vertex_buffer(&box_v);
                    gfx.model_entity_buffer = gfx.make_vertex_buffer(&model_v);
                    gfx.item_entity_buffer = gfx.make_vertex_buffer(&item_v);
                    gfx.crack_buffer = gfx.make_vertex_buffer(&crack_v);
                    let gui = &self.gui_atlas;
                    if let Some(request) = resource_pack_prompt.as_ref() {
                        let labels: [&str; 2] = if request.forced {
                            ["Proceed", "Disconnect"]
                        } else {
                            ["Accept", "Decline"]
                        };
                        let hovered = (0..2).find(|&index| {
                            let rect = crab_render::menu_button_rect(aspect, index, 2);
                            self.cursor.0 >= rect.0
                                && self.cursor.0 <= rect.2
                                && self.cursor.1 >= rect.1
                                && self.cursor.1 <= rect.3
                        });
                        let (color, mut gui_vertices, _) =
                            crab_render::menu_geometry(gui, &labels, hovered, aspect);
                        let title = "Server Resource Pack";
                        let title_h = 0.075;
                        let title_w = gui.text_width(title) * (title_h / 8.0) / aspect.max(0.01);
                        crab_render::push_text(
                            &mut gui_vertices,
                            gui,
                            title,
                            -title_w / 2.0,
                            0.62,
                            title_h,
                            aspect,
                        );
                        let message = request.prompt.as_deref().unwrap_or(if request.forced {
                            "This server requires its resource pack."
                        } else {
                            "This server recommends its resource pack."
                        });
                        let message_h = 0.045;
                        let message_w =
                            gui.text_width(message) * (message_h / 8.0) / aspect.max(0.01);
                        crab_render::push_text(
                            &mut gui_vertices,
                            gui,
                            message,
                            -message_w / 2.0,
                            0.45,
                            message_h,
                            aspect,
                        );
                        gfx.set_hud(&color, &gui_vertices, &[], &[]);
                        gfx.render(&camera, clear);
                    } else if let Some(book) = book_screen.as_ref() {
                        let (color, text) = book_geometry(gui, book, self.cursor, aspect);
                        gfx.set_hud(&color, &[], &[], &text);
                        gfx.render(&camera, clear);
                    } else if self.menu_open {
                        // Pause menu replaces the HUD; highlight the hovered button.
                        let option_labels = [
                            format!("FOV: {}", self.fov_degrees as i32),
                            format!(
                                "Sensitivity: {}%",
                                (self.mouse_sensitivity / 0.12 * 100.0).round() as i32
                            ),
                            format!(
                                "Fullscreen: {}",
                                if gfx.window.fullscreen().is_some() {
                                    "On"
                                } else {
                                    "Off"
                                }
                            ),
                            "Done".to_string(),
                        ];
                        let option_refs: Vec<&str> =
                            option_labels.iter().map(String::as_str).collect();
                        let buttons: &[&str] = if self.options_open {
                            &option_refs
                        } else if self.controls_help_open {
                            &DONE_BUTTON
                        } else {
                            &MENU_BUTTONS
                        };
                        let (cx, cy) = self.cursor;
                        let hovered = (0..buttons.len()).find(|&i| {
                            let (x0, y0, x1, y1) =
                                crab_render::menu_button_rect(aspect, i, buttons.len());
                            cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1
                        });
                        let (mc, mut mg, _) =
                            crab_render::menu_geometry(gui, buttons, hovered, aspect);
                        if self.controls_help_open || self.options_open {
                            let title = if self.options_open {
                                "Options"
                            } else {
                                "Controls"
                            };
                            let title_h = 0.085;
                            let title_w =
                                gui.text_width(title) * (title_h / 8.0) / aspect.max(0.01);
                            crab_render::push_text(
                                &mut mg,
                                gui,
                                title,
                                -title_w / 2.0,
                                0.93,
                                title_h,
                                aspect,
                            );
                            for (line, text) in CONTROL_HELP
                                .iter()
                                .enumerate()
                                .filter(|_| !self.options_open)
                            {
                                let h = 0.043;
                                let width = gui.text_width(text) * (h / 8.0) / aspect.max(0.01);
                                crab_render::push_text(
                                    &mut mg,
                                    gui,
                                    text,
                                    -width / 2.0,
                                    0.78 - line as f32 * 0.07,
                                    h,
                                    aspect,
                                );
                            }
                        }
                        gfx.set_hud(&mc, &mg, &[], &[]);
                        gfx.render(&camera, clear);
                    } else {
                        let (mut hud_c, mut hud_g, mut hud_i) = hud_geometry(
                            gui,
                            player.health,
                            player.food,
                            player.xp_bar,
                            player.xp_level,
                            selected,
                            &hotbar,
                            aspect,
                        );
                        let mut hud_text = count_text(
                            &self.shared,
                            gui,
                            aspect,
                            self.inventory_open && container.is_none(),
                        );
                        if !self.inventory_open && container.is_none() && !self.chat_open {
                            hud_text.extend(sign_text_geometry(&self.shared, gui, &camera, aspect));
                        }
                        if show_first_person_items {
                            if let Some(map) = held_map.as_ref() {
                                held_map_geometry(&mut hud_c, &mut hud_text, gui, map, aspect);
                            } else {
                                first_person_items_geometry(
                                    &mut hud_i,
                                    main_hand_icon,
                                    offhand_icon,
                                    hand_swing,
                                    aspect,
                                );
                            }
                        }
                        let border_distance = world_border.diameter / 2.0
                            - (player.x - world_border.x)
                                .abs()
                                .max((player.z - world_border.z).abs());
                        if border_distance < f64::from(world_border.warning_blocks.max(1)) {
                            let intensity = (1.0
                                - border_distance / f64::from(world_border.warning_blocks.max(1)))
                            .clamp(0.0, 1.0) as f32;
                            let thickness = 0.015 + intensity * 0.04;
                            let red = [0.35 + intensity * 0.45, 0.02, 0.02];
                            push_color2d(&mut hud_c, -1.0, -1.0, -1.0 + thickness, 1.0, red);
                            push_color2d(&mut hud_c, 1.0 - thickness, -1.0, 1.0, 1.0, red);
                            push_color2d(&mut hud_c, -1.0, -1.0, 1.0, -1.0 + thickness, red);
                            push_color2d(&mut hud_c, -1.0, 1.0 - thickness, 1.0, 1.0, red);
                        }
                        hud_g.extend(status_effect_geometry(gui, &effect_ids, aspect));
                        if let Some(name) = selected_name.as_ref() {
                            let h = 0.052;
                            let width = gui.text_width(name) * (h / 8.0) / aspect.max(0.01);
                            crab_render::push_text(
                                &mut hud_text,
                                gui,
                                name,
                                -width / 2.0,
                                -0.72,
                                h,
                                aspect,
                            );
                        }
                        if self.debug_open {
                            let lines = [
                                format!("Crabcraft 1.20.1  {:.0} fps", self.smoothed_fps),
                                format!("XYZ {:.2} / {:.2} / {:.2}", player.x, player.y, player.z),
                                format!("Yaw {:.1}  Pitch {:.1}", self.yaw, self.pitch),
                                format!(
                                    "Mode {}  {}{}{}",
                                    player.gamemode,
                                    if player.flying { "Flying " } else { "" },
                                    if player.swimming { "Swimming " } else { "" },
                                    if player.gliding { "Gliding" } else { "" }
                                ),
                            ];
                            let h = 0.042;
                            for (index, line) in lines.iter().enumerate() {
                                crab_render::push_text(
                                    &mut hud_text,
                                    gui,
                                    line,
                                    -0.98,
                                    0.97 - index as f32 * (h + 0.012),
                                    h,
                                    aspect,
                                );
                            }
                        }
                        if let Some(text) = &action_bar {
                            let h = 0.055;
                            let w = gui.text_width(text) * (h / 8.0) / aspect.max(0.01);
                            crab_render::push_text(
                                &mut hud_text,
                                gui,
                                text,
                                -w / 2.0,
                                -0.52,
                                h,
                                aspect,
                            );
                        }
                        if title_overlay.remaining > 0 {
                            if !title_overlay.title.is_empty() {
                                let h = 0.13;
                                let w = gui.text_width(&title_overlay.title) * (h / 8.0)
                                    / aspect.max(0.01);
                                crab_render::push_text(
                                    &mut hud_text,
                                    gui,
                                    &title_overlay.title,
                                    -w / 2.0,
                                    0.16,
                                    h,
                                    aspect,
                                );
                            }
                            if !title_overlay.subtitle.is_empty() {
                                let h = 0.07;
                                let w = gui.text_width(&title_overlay.subtitle) * (h / 8.0)
                                    / aspect.max(0.01);
                                crab_render::push_text(
                                    &mut hud_text,
                                    gui,
                                    &title_overlay.subtitle,
                                    -w / 2.0,
                                    0.02,
                                    h,
                                    aspect,
                                );
                            }
                        }
                        for (index, (_, bar)) in boss_bars.iter().take(6).enumerate() {
                            const COLORS: [[f32; 3]; 7] = [
                                [0.85, 0.25, 0.55],
                                [0.25, 0.4, 0.9],
                                [0.85, 0.2, 0.2],
                                [0.25, 0.75, 0.25],
                                [0.9, 0.8, 0.2],
                                [0.55, 0.25, 0.75],
                                [0.9, 0.9, 0.9],
                            ];
                            let width = 0.7 / aspect.max(0.01);
                            let y1 = 0.86 - index as f32 * 0.11;
                            let y0 = y1 - 0.025;
                            push_color2d(
                                &mut hud_c,
                                -width / 2.0,
                                y0,
                                width / 2.0,
                                y1,
                                [0.08, 0.02, 0.08],
                            );
                            let fill = width * bar.health.clamp(0.0, 1.0);
                            let color = usize::try_from(bar.color)
                                .ok()
                                .and_then(|i| COLORS.get(i))
                                .copied()
                                .unwrap_or(COLORS[0]);
                            push_color2d(
                                &mut hud_c,
                                -width / 2.0,
                                y0 + 0.004,
                                -width / 2.0 + fill,
                                y1 - 0.004,
                                color,
                            );
                            let divisions = match bar.divisions {
                                1 => 6,
                                2 => 10,
                                3 => 12,
                                4 => 20,
                                _ => 0,
                            };
                            for division in 1..divisions {
                                let x = -width / 2.0 + width * division as f32 / divisions as f32;
                                push_color2d(
                                    &mut hud_c,
                                    x - 0.001,
                                    y0,
                                    x + 0.001,
                                    y1,
                                    [0.05, 0.01, 0.05],
                                );
                            }
                            let h = 0.045;
                            let text_width =
                                gui.text_width(&bar.title) * (h / 8.0) / aspect.max(0.01);
                            crab_render::push_text(
                                &mut hud_text,
                                gui,
                                &bar.title,
                                -text_width / 2.0,
                                y1 + h + 0.008,
                                h,
                                aspect,
                            );
                        }
                        if let Some(objective) = scoreboard.sidebar.as_ref() {
                            let title = scoreboard
                                .objectives
                                .get(objective)
                                .map_or(objective.as_str(), String::as_str);
                            let mut scores: Vec<(String, i32)> = scoreboard
                                .scores
                                .get(objective)
                                .into_iter()
                                .flat_map(|scores| scores.iter())
                                .map(|(name, score)| (scoreboard.decorated_name(name), *score))
                                .collect();
                            scores.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                            scores.truncate(15);
                            let text_h = 0.045;
                            let max_pixels =
                                scores.iter().fold(gui.text_width(title), |width, row| {
                                    width.max(
                                        gui.text_width(&row.0)
                                            + 8.0
                                            + gui.text_width(&row.1.to_string()),
                                    )
                                });
                            let panel_width = max_pixels * (text_h / 8.0) / aspect.max(0.01) + 0.04;
                            let x1 = 0.98;
                            let x0 = x1 - panel_width;
                            let top = 0.5;
                            let bottom = top - text_h * (scores.len() as f32 + 1.5);
                            push_color2d(&mut hud_c, x0, bottom, x1, top, [0.06, 0.06, 0.08]);
                            let title_width =
                                gui.text_width(title) * (text_h / 8.0) / aspect.max(0.01);
                            crab_render::push_text(
                                &mut hud_text,
                                gui,
                                title,
                                (x0 + x1 - title_width) / 2.0,
                                top - 0.01,
                                text_h,
                                aspect,
                            );
                            for (row, (name, score)) in scores.iter().enumerate() {
                                let y = top - text_h * (row as f32 + 1.25);
                                crab_render::push_text(
                                    &mut hud_text,
                                    gui,
                                    name,
                                    x0 + 0.012,
                                    y,
                                    text_h,
                                    aspect,
                                );
                                let value = score.to_string();
                                let value_width =
                                    gui.text_width(&value) * (text_h / 8.0) / aspect.max(0.01);
                                crab_render::push_text(
                                    &mut hud_text,
                                    gui,
                                    &value,
                                    x1 - value_width - 0.012,
                                    y,
                                    text_h,
                                    aspect,
                                );
                            }
                        }
                        if let Some(list) = player_list.as_ref() {
                            let mut entries: Vec<_> =
                                list.entries.values().filter(|entry| entry.listed).collect();
                            entries.sort_by_key(|entry| entry.name.to_ascii_lowercase());
                            entries.truncate(80);
                            let rows = entries.len().clamp(1, 20);
                            let columns = entries.len().div_ceil(rows).max(1);
                            let row_h = 0.052;
                            let col_w = 0.48 / aspect.max(0.01);
                            let panel_w = col_w * columns as f32 + 0.04;
                            let x0 = -panel_w / 2.0;
                            let x1 = panel_w / 2.0;
                            let top = 0.92;
                            let header_rows = usize::from(!list.header.is_empty());
                            let footer_rows = usize::from(!list.footer.is_empty());
                            let panel_h = row_h * (rows + header_rows + footer_rows) as f32 + 0.05;
                            push_color2d(
                                &mut hud_c,
                                x0,
                                top - panel_h,
                                x1,
                                top,
                                [0.05, 0.05, 0.07],
                            );
                            let mut content_top = top - 0.02;
                            if !list.header.is_empty() {
                                let width =
                                    gui.text_width(&list.header) * (row_h / 8.0) / aspect.max(0.01);
                                crab_render::push_text(
                                    &mut hud_text,
                                    gui,
                                    &list.header,
                                    -width / 2.0,
                                    content_top,
                                    row_h,
                                    aspect,
                                );
                                content_top -= row_h;
                            }
                            for (index, entry) in entries.iter().enumerate() {
                                let column = index / rows;
                                let row = index % rows;
                                let x = x0 + 0.02 + column as f32 * col_w;
                                let y = content_top - row as f32 * row_h;
                                let decorated;
                                let name = if let Some(display_name) = entry.display_name.as_ref() {
                                    display_name.as_str()
                                } else {
                                    decorated = scoreboard.decorated_name(&entry.name);
                                    &decorated
                                };
                                crab_render::push_text(
                                    &mut hud_text,
                                    gui,
                                    name,
                                    x,
                                    y,
                                    row_h * 0.82,
                                    aspect,
                                );
                                let latency = format!("{}ms", entry.latency.max(0));
                                let latency_w = gui.text_width(&latency) * (row_h * 0.82 / 8.0)
                                    / aspect.max(0.01);
                                crab_render::push_text(
                                    &mut hud_text,
                                    gui,
                                    &latency,
                                    x + col_w - latency_w - 0.02,
                                    y,
                                    row_h * 0.82,
                                    aspect,
                                );
                            }
                            if !list.footer.is_empty() {
                                let y = content_top - rows as f32 * row_h;
                                let width =
                                    gui.text_width(&list.footer) * (row_h / 8.0) / aspect.max(0.01);
                                crab_render::push_text(
                                    &mut hud_text,
                                    gui,
                                    &list.footer,
                                    -width / 2.0,
                                    y,
                                    row_h,
                                    aspect,
                                );
                            }
                        }
                        let (chat_c, chat_t) = chat_geometry(
                            &self.shared,
                            gui,
                            self.chat_open,
                            &self.chat_buffer,
                            aspect,
                        );
                        hud_c.extend(chat_c);
                        hud_text.extend(chat_t);
                        if let Some(inv) = &inv_icons {
                            let (ic, ig, ii) = inventory_geometry(gui, inv, aspect);
                            hud_c.extend(ic);
                            hud_g.extend(ig);
                            hud_i.extend(ii);
                        }
                        if let (Some(container), Some(icons), Some(rows)) = (
                            container.as_ref(),
                            container_icons.as_ref(),
                            container.as_ref().and_then(ContainerState::generic_rows),
                        ) {
                            let (cc, cg, ci) = container_geometry(gui, icons, rows, aspect);
                            hud_c.extend(cc);
                            hud_g.extend(cg);
                            hud_i.extend(ci);
                            hud_text.extend(container_text(
                                container,
                                gui,
                                aspect,
                                &self.anvil_name,
                            ));
                        }
                        if let (Some(container), Some(icons), Some(texture)) = (
                            container.as_ref(),
                            container_icons.as_ref(),
                            container.as_ref().and_then(ContainerState::furnace_texture),
                        ) {
                            let (fc, fg, fi) =
                                furnace_geometry(gui, icons, texture, container.properties, aspect);
                            hud_c.extend(fc);
                            hud_g.extend(fg);
                            hud_i.extend(fi);
                            hud_text.extend(container_text(
                                container,
                                gui,
                                aspect,
                                &self.anvil_name,
                            ));
                        }
                        if let (Some(container), Some(icons), Some(texture)) = (
                            container.as_ref(),
                            container_icons.as_ref(),
                            container
                                .as_ref()
                                .and_then(ContainerState::simple_container_texture),
                        ) {
                            let (sc, sg, si) =
                                simple_container_geometry(gui, icons, texture, aspect);
                            hud_c.extend(sc);
                            hud_g.extend(sg);
                            hud_i.extend(si);
                            hud_text.extend(container_text(
                                container,
                                gui,
                                aspect,
                                &self.anvil_name,
                            ));
                            if matches!(container.menu_type, 17 | 23) {
                                let recipes = if container.menu_type == 23 {
                                    applicable_stonecutter_recipes(&self.shared, container)
                                } else {
                                    Vec::new()
                                };
                                let total = if container.menu_type == 17 {
                                    34
                                } else {
                                    recipes.len()
                                };
                                let per_page = if container.menu_type == 17 { 16 } else { 12 };
                                let page = self.recipe_page.min(total.saturating_sub(1) / per_page);
                                let icons: Vec<Option<[f32; 4]>> = recipes
                                    .iter()
                                    .map(|recipe| {
                                        recipe.result.and_then(|item| {
                                            u32::try_from(item.item_id)
                                                .ok()
                                                .and_then(crab_registry::item_name)
                                                .and_then(|name| self.item_atlas.icon(name))
                                        })
                                    })
                                    .collect();
                                recipe_selection_geometry(
                                    &mut hud_c,
                                    &mut hud_i,
                                    &mut hud_text,
                                    gui,
                                    texture,
                                    page,
                                    container.properties[0],
                                    total,
                                    &icons,
                                    self.cursor,
                                    aspect,
                                );
                            }
                        }
                        if let Some(context) = recipe_context {
                            let toggle = recipe_book_toggle_rect(context.panel, aspect);
                            push_color2d(
                                &mut hud_c,
                                toggle.0,
                                toggle.1,
                                toggle.2,
                                toggle.3,
                                if self.recipe_book_open {
                                    [0.35, 0.68, 0.30]
                                } else {
                                    [0.25, 0.48, 0.22]
                                },
                            );
                            crab_render::push_text(
                                &mut hud_text,
                                gui,
                                "R",
                                toggle.0 + 0.026 / aspect.max(0.01),
                                toggle.3 - 0.025,
                                0.06,
                                aspect,
                            );
                            if self.recipe_book_open {
                                let page_count = crafting_recipes.len().max(1).div_ceil(20);
                                let page = self.recipe_book_page.min(page_count - 1);
                                let (colors, items, text) = recipe_book_geometry(
                                    gui,
                                    &self.item_atlas,
                                    &crafting_recipes,
                                    page,
                                    self.cursor,
                                    context.panel,
                                    aspect,
                                );
                                hud_c.extend(colors);
                                hud_i.extend(items);
                                hud_text.extend(text);
                            }
                        }
                        if inv_icons.is_some() || container_icons.is_some() {
                            // Item held on the cursor, drawn at the mouse position.
                            if let Some(it) = *self.shared.carried.lock().unwrap() {
                                if let Some(uv) = u32::try_from(it.item_id)
                                    .ok()
                                    .and_then(crab_registry::item_name)
                                    .and_then(|n| self.item_atlas.icon(n))
                                {
                                    let (cx, cy) = self.cursor;
                                    let s = 0.055;
                                    let hw = s / aspect;
                                    push_tex2d(&mut hud_i, cx - hw, cy - s, cx + hw, cy + s, uv);
                                }
                            }
                        }
                        if let Some(item) = hovered_item {
                            if let Some(name) = u32::try_from(item.item_id)
                                .ok()
                                .and_then(crab_registry::item_name)
                            {
                                let mut lines = vec![pretty_item_name(name)];
                                if let Some((slot, contents)) = hovered_bundle.as_ref() {
                                    let selected = self
                                        .bundle_selection
                                        .filter(|(selected_slot, _)| selected_slot == slot)
                                        .and_then(|(_, index)| usize::try_from(index).ok())
                                        .filter(|index| *index < contents.len());
                                    lines[0] = "Bundle".to_string();
                                    if let Some(index) = selected {
                                        let nested = contents[index];
                                        let nested_name = u32::try_from(nested.item_id)
                                            .ok()
                                            .and_then(crab_registry::item_name)
                                            .map(pretty_item_name)
                                            .unwrap_or_else(|| format!("Item #{}", nested.item_id));
                                        lines.push(format!("{nested_name} x{}", nested.count));
                                        lines.push(format!(
                                            "Scroll to select  {} / {}",
                                            index + 1,
                                            contents.len()
                                        ));
                                    } else {
                                        lines.push(format!(
                                            "{} stack{}  -  scroll to select",
                                            contents.len(),
                                            if contents.len() == 1 { "" } else { "s" }
                                        ));
                                    }
                                }
                                let h = 0.052;
                                let width = lines
                                    .iter()
                                    .map(|line| gui.text_width(line) * (h / 8.0) / aspect.max(0.01))
                                    .fold(0.0, f32::max);
                                let x = (self.cursor.0 + 0.025).clamp(-0.98, 0.98 - width - 0.025);
                                let y = (self.cursor.1 + 0.075).clamp(-0.9, 0.95);
                                let height = h * lines.len() as f32;
                                push_color2d(
                                    &mut hud_c,
                                    x - 0.012,
                                    y - height - 0.012,
                                    x + width + 0.012,
                                    y + 0.012,
                                    [0.06, 0.02, 0.09],
                                );
                                for (line, text) in lines.iter().enumerate() {
                                    crab_render::push_text(
                                        &mut hud_text,
                                        gui,
                                        text,
                                        x,
                                        y - h * line as f32,
                                        h,
                                        aspect,
                                    );
                                }
                            }
                        }
                        if self.inventory_open && container.is_none() {
                            if let Some(preview) =
                                inventory_player_preview(&self.entity_atlas, self.cursor, aspect)
                            {
                                gfx.set_inventory_player(
                                    &preview.mesh,
                                    Some(&preview.camera),
                                    Some(preview.bounds),
                                );
                            }
                        }
                        gfx.set_hud(&hud_c, &hud_g, &hud_i, &hud_text);
                        gfx.render(&camera, clear);
                    }
                }

                if !self
                    .shared
                    .running
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.apply_pending_resource_pack();
        let server_open = self.server_container_open();
        let resource_pack_open = self.resource_pack_prompt().is_some();
        if server_open != self.container_seen || resource_pack_open != self.resource_pack_seen {
            self.container_seen = server_open;
            self.resource_pack_seen = resource_pack_open;
            if server_open {
                self.inventory_open = false;
                self.anvil_name.clear();
                self.recipe_page = 0;
            }
            self.set_cursor_free(
                server_open
                    || self.inventory_open
                    || self.chat_open
                    || self.menu_open
                    || resource_pack_open,
            );
        }
        if let Some(gfx) = &self.gfx {
            gfx.window.request_redraw();
        }
    }
}

/// Runs the windowed renderer (blocking; must be called on the main thread).
pub fn run(
    shared: Arc<Shared>,
    atlas: Atlas,
    entity_atlas: EntityAtlas,
    item_atlas: ItemAtlas,
    gui_atlas: GuiAtlas,
    crack: Option<(Vec<u8>, u32, u32)>,
) -> anyhow::Result<()> {
    let atlas = Arc::new(atlas);
    let atlas_slot = Arc::new(RwLock::new(Arc::clone(&atlas)));
    let entity_atlas = Arc::new(entity_atlas);
    let item_atlas = Arc::new(item_atlas);
    let gui_atlas = Arc::new(gui_atlas);
    // Spawn the background mesher.
    let (mesh_tx, mesh_rx) = std::sync::mpsc::channel();
    {
        let shared = Arc::clone(&shared);
        let atlas_slot = Arc::clone(&atlas_slot);
        std::thread::spawn(move || mesher_loop(shared, atlas_slot, mesh_tx));
    }

    let event_loop = EventLoop::new()?;
    let mut app = App::new(
        shared,
        atlas_slot,
        entity_atlas,
        item_atlas,
        gui_atlas,
        mesh_rx,
        crack,
    );
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_cycle_and_camera_apply_selected_fov() {
        assert_eq!(next_choice(70.0, &FOV_CHOICES), 90.0);
        assert_eq!(next_choice(110.0, &FOV_CHOICES), 60.0);
        assert_eq!(next_choice(0.12, &SENSITIVITY_CHOICES), 0.18);

        let camera = first_person_camera(
            Vec3::ZERO,
            0.0,
            0.0,
            16.0 / 9.0,
            1.62,
            90.0,
            Perspective::FirstPerson,
        );
        assert!((camera.fovy_radians - 90f32.to_radians()).abs() < f32::EPSILON);
        let rear = first_person_camera(
            Vec3::ZERO,
            0.0,
            0.0,
            1.0,
            1.62,
            70.0,
            Perspective::ThirdPersonBack,
        );
        assert!(rear.eye.z < rear.target.z);
        assert_eq!(
            Perspective::FirstPerson.next(),
            Perspective::ThirdPersonBack
        );
        assert_eq!(
            Perspective::ThirdPersonFront.next(),
            Perspective::FirstPerson
        );
    }

    #[test]
    fn sky_tracks_day_night_and_storm_darkening() {
        let noon = sky_color(EnvironmentState {
            time_of_day: 6_000,
            ..EnvironmentState::default()
        });
        let midnight = sky_color(EnvironmentState {
            time_of_day: 18_000,
            ..EnvironmentState::default()
        });
        let storm = sky_color(EnvironmentState {
            time_of_day: 6_000,
            rain_level: 1.0,
            thunder_level: 1.0,
            ..EnvironmentState::default()
        });
        assert!(noon[2] > midnight[2]);
        assert!(storm[0] < noon[0]);
        assert_eq!(
            sky_color(EnvironmentState {
                time_of_day: -6_000,
                ..EnvironmentState::default()
            }),
            noon
        );
        let eye = Vec3::new(0.0, 80.0, 0.0);
        let day_mesh = celestial_mesh(
            eye,
            EnvironmentState {
                time_of_day: 6_000,
                ..EnvironmentState::default()
            },
            [0.0, 0.0, 1.0, 1.0],
        );
        let night_mesh = celestial_mesh(
            eye,
            EnvironmentState {
                time_of_day: 18_000,
                ..EnvironmentState::default()
            },
            [0.0, 0.0, 1.0, 1.0],
        );
        assert!(night_mesh.len() > day_mesh.len());
    }

    #[test]
    fn item_names_are_human_readable() {
        assert_eq!(pretty_item_name("diamond_pickaxe"), "Diamond Pickaxe");
        assert_eq!(pretty_item_name("oak_log"), "Oak Log");
        assert_ne!(
            armour_color("diamond_chestplate"),
            armour_color("golden_chestplate")
        );
    }

    #[test]
    fn bundle_metadata_drives_interactive_tooltip_items() {
        use crab_protocol::nbt::Nbt;

        let entry = Nbt::Compound(HashMap::from([
            ("id".to_string(), Nbt::Int(42)),
            ("count".to_string(), Nbt::Byte(5)),
        ]));
        let metadata = Nbt::Compound(HashMap::from([(
            "bundle_contents".to_string(),
            Nbt::List(vec![entry]),
        )]));
        assert_eq!(
            bundle_items(Some(&metadata)),
            vec![crab_protocol::versions::v1_20_1::play::SlotItem {
                item_id: 42,
                count: 5,
            }]
        );
        assert!(bundle_items(None).is_empty());
    }

    #[test]
    fn book_pages_wrap_words_newlines_and_long_runs() {
        assert_eq!(
            wrapped_book_lines("one two three", 7, 10),
            vec!["one two", "three"]
        );
        assert_eq!(
            wrapped_book_lines("first\nsecond", 20, 10),
            vec!["first", "second"]
        );
        assert_eq!(wrapped_book_lines("abcdefgh", 4, 10), vec!["abcd", "efgh"]);
        assert_eq!(wrapped_book_lines("a\nb\nc", 20, 2), vec!["a", "b"]);
    }

    #[test]
    fn first_person_items_emit_one_quad_per_visible_hand() {
        let mut vertices = Vec::new();
        first_person_items_geometry(
            &mut vertices,
            Some([0.0, 0.0, 0.5, 0.5]),
            Some([0.5, 0.5, 1.0, 1.0]),
            0.5,
            16.0 / 9.0,
        );
        assert_eq!(vertices.len(), 12);
        assert!(vertices.iter().all(|vertex| vertex[0].is_finite()));
    }

    #[test]
    fn recipe_grids_page_loom_patterns_and_stonecutter_choices() {
        let gui = GuiAtlas::empty();
        let mut color = Vec::new();
        let mut items = Vec::new();
        let mut text = Vec::new();
        recipe_selection_geometry(
            &mut color,
            &mut items,
            &mut text,
            &gui,
            "loom",
            2,
            33,
            34,
            &[],
            (2.0, 2.0),
            16.0 / 9.0,
        );
        assert_eq!(color.len(), 12); // final page contains patterns 33 and 34

        color.clear();
        items.clear();
        text.clear();
        recipe_selection_geometry(
            &mut color,
            &mut items,
            &mut text,
            &gui,
            "stonecutter",
            0,
            0,
            12,
            &[],
            (2.0, 2.0),
            16.0 / 9.0,
        );
        assert_eq!(color.len(), 72); // twelve visible buttons, six vertices each
    }

    #[test]
    fn held_map_geometry_draws_pixels_and_markers() {
        let gui = GuiAtlas::empty();
        let mut map = crate::client::MapState::default();
        map.colors[0] = 6; // sand, brightest shade
        map.markers.push(crate::client::MapMarker {
            kind: 0,
            x: 0,
            z: 0,
            direction: 0,
            label: Some("Spawn".into()),
        });
        let mut color = Vec::new();
        let mut text = Vec::new();
        held_map_geometry(&mut color, &mut text, &gui, &map, 16.0 / 9.0);
        assert!(color.len() >= 24); // frame, paper, one pixel run, one marker
        assert!(map_pixel_color(6).is_some());
        assert!(map_pixel_color(0).is_none());
    }
}
