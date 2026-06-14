//! Crabcraft client.
//!
//! Two modes:
//! ```text
//! crabcraft [ADDR] [USERNAME] [SECONDS]   # headless: connect, play, log
//! crabcraft render [ADDR] [USERNAME]      # windowed: fly around the world
//! ```
//! Defaults: `127.0.0.1:25565`, `Ferris`, `35` seconds (headless only).
//!
//! Targets offline-mode 1.20.1 servers (no auth/encryption yet).

mod client;
mod window;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use client::{connect_and_play, Shared};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let render_mode = args.first().is_some_and(|s| s == "render");
    if render_mode {
        args.remove(0);
    }

    let addr = args
        .first()
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:25565".to_string());
    let username = args.get(1).cloned().unwrap_or_else(|| "Ferris".to_string());

    if render_mode {
        run_windowed(addr, username)
    } else {
        let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(35);
        run_headless(addr, username, secs)
    }
}

/// Headless: run the client to completion on a tokio runtime.
fn run_headless(addr: String, username: String, secs: u64) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let shared = Arc::new(Shared::new());
    rt.block_on(connect_and_play(
        &addr,
        &username,
        shared,
        Some(Duration::from_secs(secs)),
    ))
}

/// Windowed: networking on a background thread, rendering on the main thread
/// (winit requires the event loop to own the main thread).
fn run_windowed(addr: String, username: String) -> Result<()> {
    let shared = Arc::new(Shared::new());

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
        if let Err(e) = rt.block_on(connect_and_play(&addr, &username, net_shared, None)) {
            tracing::error!("client error: {e}");
        }
    });

    window::run(shared)
}
