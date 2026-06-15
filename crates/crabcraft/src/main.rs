//! Crabcraft client.
//!
//! ```text
//! crabcraft [ADDR] [USERNAME] [SECONDS]   # offline headless
//! crabcraft render [ADDR] [USERNAME]      # offline windowed (fly around)
//! crabcraft online [ADDR]                 # online headless (Microsoft login)
//! crabcraft render online [ADDR]          # online windowed
//! ```
//! Offline defaults: `127.0.0.1:25565`, `Ferris`, `35`s. Online mode logs in
//! with a Microsoft account (device-code flow) and can join real servers.

mod client;
mod window;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use client::{connect_and_play, LoginMode, Shared};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    // Leading flags (any order): `render`, `online`.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut render = false;
    let mut online = false;
    while let Some(flag) = args.first() {
        match flag.as_str() {
            "render" => {
                render = true;
                args.remove(0);
            }
            "online" => {
                online = true;
                args.remove(0);
            }
            _ => break,
        }
    }

    let addr = args
        .first()
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:25565".to_string());

    if online {
        run_online(addr, render)
    } else {
        let username = args.get(1).cloned().unwrap_or_else(|| "Ferris".to_string());
        let login = LoginMode::Offline { username };
        if render {
            run_windowed(addr, login, None)
        } else {
            let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(35);
            run_headless(addr, login, Some(Duration::from_secs(secs)))
        }
    }
}

/// Online mode: Microsoft device-code login first, then connect.
fn run_online(addr: String, render: bool) -> Result<()> {
    if render {
        // Auth happens on the network thread; the window opens immediately.
        run_windowed_online(addr)
    } else {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        rt.block_on(async move {
            let session = crab_auth::device_code_login()
                .await
                .context("Microsoft login")?;
            tracing::info!(user = %session.username, "authenticated");
            let shared = Arc::new(Shared::new());
            init_sound(&shared);
            connect_and_play(&addr, LoginMode::Online(session), shared, None).await
        })
    }
}

/// Headless: run the client to completion on a tokio runtime.
fn run_headless(addr: String, login: LoginMode, deadline: Option<Duration>) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let shared = Arc::new(Shared::new());
    init_sound(&shared);
    rt.block_on(connect_and_play(&addr, login, shared, deadline))
}

/// Loads the block texture atlas from the client jar named by `CRABCRAFT_JAR`,
/// or falls back to flat colours if unset/unreadable.
fn load_atlas() -> crab_assets::Atlas {
    match std::env::var("CRABCRAFT_JAR") {
        Ok(jar) => {
            let names: Vec<String> = crab_registry::BLOCKS_1_20_1
                .iter()
                .map(|b| b.name.to_string())
                .collect();
            match crab_assets::load_block_atlas(std::path::Path::new(&jar), &names) {
                Ok(atlas) => {
                    tracing::info!(width = atlas.width, "loaded textures from {jar}");
                    atlas
                }
                Err(e) => {
                    tracing::warn!("texture load failed ({e}); using flat colours");
                    crab_assets::Atlas::debug_uniform()
                }
            }
        }
        Err(_) => {
            tracing::info!(
                "set CRABCRAFT_JAR=<path to 1.20.1.jar> for textures; using flat colours"
            );
            crab_assets::Atlas::debug_uniform()
        }
    }
}

/// Loads the flat item-icon atlas (for the hotbar) from `CRABCRAFT_JAR`, or an
/// empty atlas (no icons) if unset/unreadable.
fn load_item_atlas() -> crab_assets::ItemAtlas {
    let Ok(jar) = std::env::var("CRABCRAFT_JAR") else {
        return crab_assets::ItemAtlas::empty();
    };
    let names: Vec<String> = crab_registry::ITEMS_1_20_1
        .iter()
        .map(|i| i.name.to_string())
        .collect();
    match crab_assets::load_item_atlas(std::path::Path::new(&jar), &names) {
        Ok(atlas) => {
            tracing::info!("loaded {} item icons from {jar}", atlas.len());
            atlas
        }
        Err(e) => {
            tracing::warn!("item icon load failed ({e}); hotbar shows no icons");
            crab_assets::ItemAtlas::empty()
        }
    }
}

/// Loads 3D entity models + textures: geometry from `CRABCRAFT_ENTITY_MODELS`
/// (a bedrock-samples `models/entity` dir) and textures from `CRABCRAFT_JAR`.
/// Returns an empty atlas (entities render as boxes) if either is unset.
fn load_entity_atlas() -> crab_assets::EntityAtlas {
    let empty =
        || crab_assets::load_entity_atlas(std::path::Path::new(""), std::path::Path::new(""), &[]);
    let (Ok(jar), Ok(dir)) = (
        std::env::var("CRABCRAFT_JAR"),
        std::env::var("CRABCRAFT_ENTITY_MODELS"),
    ) else {
        tracing::info!(
            "set CRABCRAFT_ENTITY_MODELS=<bedrock-samples .../resource_pack/models/entity> \
             for 3D entity models; using boxes"
        );
        return empty();
    };

    // Be forgiving: accept the entity dir, or a bedrock-samples root/resource_pack.
    let mut dir = std::path::PathBuf::from(&dir);
    if !dir.join("cow.geo.json").exists() {
        for sub in ["resource_pack/models/entity", "models/entity", "entity"] {
            if dir.join(sub).join("cow.geo.json").exists() {
                dir = dir.join(sub);
                break;
            }
        }
    }
    let geo_count = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().ends_with(".geo.json"))
                .count()
        })
        .unwrap_or(0);

    let types: Vec<(i32, String)> = crab_registry::ENTITIES_1_20_1
        .iter()
        .map(|e| (e.id as i32, e.name.to_string()))
        .collect();
    let atlas = crab_assets::load_entity_atlas(std::path::Path::new(&jar), &dir, &types);

    if atlas.models.is_empty() {
        tracing::warn!(
            "no entity models loaded from {dir:?} ({geo_count} .geo.json files found) \u{2014} \
             entities render as boxes. Point CRABCRAFT_ENTITY_MODELS at \
             bedrock-samples/resource_pack/models/entity"
        );
    } else {
        tracing::info!("loaded {} entity models from {dir:?}", atlas.models.len());
    }
    atlas
}

/// If `CRABCRAFT_ASSETS` points at a launcher asset store, spawns a sound
/// thread (owning the audio device) and wires it into `shared.sfx`. The asset
/// index id defaults to 1.20.1's `"5"` (override with `CRABCRAFT_ASSET_INDEX`).
fn init_sound(shared: &Arc<Shared>) {
    let Ok(assets) = std::env::var("CRABCRAFT_ASSETS") else {
        tracing::info!("set CRABCRAFT_ASSETS=<.../assets> for sounds; running silent");
        return;
    };
    let id = std::env::var("CRABCRAFT_ASSET_INDEX").unwrap_or_else(|_| "5".to_string());
    let assets = std::path::PathBuf::from(assets);
    let index =
        match crab_audio::AssetIndex::load(&assets.join("indexes").join(format!("{id}.json"))) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("sound asset index failed ({e}); running silent");
                return;
            }
        };
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    *shared.sfx.lock().unwrap() = Some(tx);
    std::thread::spawn(move || {
        let player = crab_audio::SoundPlayer::new();
        tracing::info!(
            objects = index.len(),
            audio = player.available(),
            "sound enabled"
        );
        while let Ok(name) = rx.recv() {
            if let Some(bytes) = crab_audio::read_sound(&assets, &index, &name) {
                player.play_ogg(bytes);
            }
        }
    });
}

/// Windowed: networking on a background thread, rendering on the main thread.
fn run_windowed(addr: String, login: LoginMode, deadline: Option<Duration>) -> Result<()> {
    let shared = Arc::new(Shared::new());
    init_sound(&shared);
    let atlas = load_atlas();
    let entity_atlas = load_entity_atlas();
    let item_atlas = load_item_atlas();
    spawn_net_thread(addr, login, Arc::clone(&shared), deadline);
    window::run(shared, atlas, entity_atlas, item_atlas)
}

/// Windowed online: authenticate on the network thread, then connect.
fn run_windowed_online(addr: String) -> Result<()> {
    let shared = Arc::new(Shared::new());
    init_sound(&shared);
    let atlas = load_atlas();
    let entity_atlas = load_entity_atlas();
    let item_atlas = load_item_atlas();
    let net_shared = Arc::clone(&shared);
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("failed to build runtime: {e}");
                return;
            }
        };
        rt.block_on(async move {
            match crab_auth::device_code_login().await {
                Ok(session) => {
                    if let Err(e) =
                        connect_and_play(&addr, LoginMode::Online(session), net_shared, None).await
                    {
                        tracing::error!("client error: {e}");
                    }
                }
                Err(e) => tracing::error!("Microsoft login failed: {e}"),
            }
        });
    });
    window::run(shared, atlas, entity_atlas, item_atlas)
}

fn spawn_net_thread(
    addr: String,
    login: LoginMode,
    shared: Arc<Shared>,
    deadline: Option<Duration>,
) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("failed to build runtime: {e}");
                return;
            }
        };
        if let Err(e) = rt.block_on(connect_and_play(&addr, login, shared, deadline)) {
            tracing::error!("client error: {e}");
        }
    });
}
