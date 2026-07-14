//! Downloads user-requested Minecraft runtime inputs and launches Crabcraft.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use sha1::{Digest, Sha1};
use tokio::sync::Semaphore;

const VERSION_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const ASSET_CDN: &str = "https://resources.download.minecraft.net";
const DEFAULT_VERSION: &str = "1.20.1";

#[derive(Deserialize)]
struct VersionManifest {
    versions: Vec<VersionEntry>,
}

#[derive(Deserialize)]
struct VersionEntry {
    id: String,
    url: String,
}

#[derive(Deserialize)]
struct VersionMetadata {
    #[serde(rename = "assetIndex")]
    asset_index: Download,
    downloads: Downloads,
}

#[derive(Deserialize)]
struct Downloads {
    client: Download,
    server: Option<Download>,
}

#[derive(Clone, Deserialize)]
struct Download {
    id: Option<String>,
    sha1: String,
    url: String,
}

#[derive(Deserialize)]
struct AssetIndex {
    objects: BTreeMap<String, AssetObject>,
}

#[derive(Clone, Deserialize)]
struct AssetObject {
    hash: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "client".to_owned());
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "client" => launch_client(&rest).await,
        "server" => launch_server(&rest).await,
        "both" => launch_both(&rest).await,
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => bail!("unknown command `{other}`; use `client`, `server`, `both`, or `help`"),
    }
}

async fn launch_client(args: &[String]) -> Result<()> {
    let (version, forwarded) = take_option(args, "--version", DEFAULT_VERSION)?;
    let mut command = prepare_client(&version, &forwarded).await?;
    exit_with(command.status().context("launch Crabcraft with cargo")?)
}

async fn prepare_client(version: &str, forwarded: &[String]) -> Result<Command> {
    let root = cache_root()?;
    let metadata = version_metadata(version).await?;
    let version_dir = root.join("versions").join(version);
    let jar = version_dir.join("client.jar");
    download_verified(&Client::new(), &metadata.downloads.client, &jar).await?;

    let asset_index_id = metadata
        .asset_index
        .id
        .as_deref()
        .context("version metadata has no asset-index id")?;
    let assets = root.join("assets");
    let index_path = assets
        .join("indexes")
        .join(format!("{asset_index_id}.json"));
    download_verified(&Client::new(), &metadata.asset_index, &index_path).await?;
    download_audio_assets(&assets, &index_path).await?;
    let entity_models = ensure_bedrock_models(&root)?;

    let mut command = Command::new("cargo");
    command
        .args(["run", "--release", "-p", "crabcraft", "--", "render"])
        .args(forwarded)
        .env("CRABCRAFT_PROTOCOL", version)
        .env("CRABCRAFT_JAR", &jar)
        .env("CRABCRAFT_ASSETS", &assets)
        .env("CRABCRAFT_ASSET_INDEX", asset_index_id)
        .env("CRABCRAFT_ENTITY_MODELS", entity_models);
    Ok(command)
}

async fn launch_server(args: &[String]) -> Result<()> {
    let (version, forwarded) = take_option(args, "--version", DEFAULT_VERSION)?;
    let accept_eula = forwarded.iter().any(|arg| arg == "--accept-eula");
    let offline = forwarded.iter().any(|arg| arg == "--offline");
    let java_args: Vec<&String> = forwarded
        .iter()
        .filter(|arg| *arg != "--accept-eula" && *arg != "--offline")
        .collect();
    let mut command = prepare_server(&version, accept_eula, offline, &java_args).await?;
    exit_with(
        command
            .status()
            .context("launch server; is Java installed and on PATH?")?,
    )
}

async fn prepare_server(
    version: &str,
    accept_eula: bool,
    offline: bool,
    java_args: &[&String],
) -> Result<Command> {
    let root = cache_root()?;
    let metadata = version_metadata(version).await?;
    let download = metadata
        .downloads
        .server
        .as_ref()
        .with_context(|| format!("Minecraft {version} has no server download"))?;
    let server_dir = root.join("servers").join(version);
    let jar = server_dir.join("server.jar");
    download_verified(&Client::new(), download, &jar).await?;

    let eula = server_dir.join("eula.txt");
    if !eula_accepted(&eula)? {
        if !accept_eula {
            bail!(
                "the Minecraft EULA must be accepted before first server launch; read \
                 https://www.minecraft.net/eula, then rerun with `server --accept-eula`"
            );
        }
        std::fs::create_dir_all(&server_dir)?;
        std::fs::write(
            &eula,
            "# Accepted explicitly through crabcraft-launcher\neula=true\n",
        )?;
    }
    set_property(
        &server_dir.join("server.properties"),
        "online-mode",
        if offline { "false" } else { "true" },
    )?;

    let mut command = Command::new("java");
    command
        .current_dir(&server_dir)
        .args(["-Xms1G", "-Xmx2G", "-jar"])
        .arg(&jar)
        .args(java_args)
        .arg("nogui");
    Ok(command)
}

async fn launch_both(args: &[String]) -> Result<()> {
    let (version, rest) = take_option(args, "--version", DEFAULT_VERSION)?;
    let (username, rest) = take_option(&rest, "--username", "Ferris")?;
    let accept_eula = rest.iter().any(|arg| arg == "--accept-eula");
    if let Some(argument) = rest.iter().find(|arg| *arg != "--accept-eula") {
        bail!("unknown `both` argument `{argument}`");
    }

    let mut server_command = prepare_server(&version, accept_eula, true, &[]).await?;
    server_command.stdin(Stdio::piped());
    let mut server = server_command
        .spawn()
        .context("launch server; is Java installed and on PATH?")?;
    if let Err(error) = wait_for_server(&mut server, "127.0.0.1:25565").await {
        let _ = server.kill();
        let _ = server.wait();
        return Err(error);
    }

    let forwarded = vec!["127.0.0.1:25565".to_owned(), username];
    let client_result = prepare_client(&version, &forwarded)
        .await
        .and_then(|mut command| {
            command
                .status()
                .context("launch Crabcraft with cargo")
                .and_then(exit_with)
        });

    if let Some(mut stdin) = server.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(b"stop\n");
    }
    let server_status = server.wait().context("wait for server shutdown")?;
    client_result?;
    exit_with(server_status)
}

async fn wait_for_server(server: &mut std::process::Child, address: &str) -> Result<()> {
    let parsed = address.parse().context("parse local server address")?;
    let started = Instant::now();
    loop {
        if let Some(status) = server.try_wait()? {
            bail!("server exited before accepting connections ({status})");
        }
        if std::net::TcpStream::connect_timeout(&parsed, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        if started.elapsed() >= Duration::from_secs(60) {
            bail!("server did not accept connections at {address} within 60 seconds");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn take_option(args: &[String], name: &str, default: &str) -> Result<(String, Vec<String>)> {
    let mut value = default.to_owned();
    let mut rest = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == name {
            value = args
                .get(index + 1)
                .with_context(|| format!("{name} requires a value"))?
                .clone();
            index += 2;
        } else {
            rest.push(args[index].clone());
            index += 1;
        }
    }
    Ok((value, rest))
}

fn cache_root() -> Result<PathBuf> {
    Ok(std::env::current_dir()
        .context("find workspace directory")?
        .join("assets-cache"))
}

async fn version_metadata(version: &str) -> Result<VersionMetadata> {
    let client = Client::new();
    let manifest: VersionManifest = get_json(&client, VERSION_MANIFEST).await?;
    let entry = manifest
        .versions
        .iter()
        .find(|entry| entry.id == version)
        .with_context(|| format!("Minecraft version `{version}` is not in Mojang's manifest"))?;
    get_json(&client, &entry.url).await
}

async fn get_json<T: for<'de> Deserialize<'de>>(client: &Client, url: &str) -> Result<T> {
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()?
        .json()
        .await
        .with_context(|| format!("parse {url}"))
}

async fn download_verified(client: &Client, download: &Download, path: &Path) -> Result<()> {
    if file_has_sha1(path, &download.sha1)? {
        return Ok(());
    }
    eprintln!("Downloading {}", path.display());
    let bytes = client
        .get(&download.url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let actual = format!("{:x}", Sha1::digest(&bytes));
    if actual != download.sha1 {
        bail!("SHA-1 mismatch for {}", path.display());
    }
    let parent = path.parent().context("download path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let part = path.with_extension("part");
    std::fs::write(&part, bytes)?;
    std::fs::rename(part, path)?;
    Ok(())
}

fn file_has_sha1(path: &Path, expected: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let bytes = std::fs::read(path)?;
    Ok(format!("{:x}", Sha1::digest(bytes)) == expected)
}

async fn download_audio_assets(assets: &Path, index_path: &Path) -> Result<()> {
    let index: AssetIndex = serde_json::from_slice(&std::fs::read(index_path)?)?;
    let wanted = audio_objects(index);
    eprintln!("Checking {} sound assets", wanted.len());
    let client = Client::new();
    let semaphore = Arc::new(Semaphore::new(16));
    let mut tasks = tokio::task::JoinSet::new();
    for object in wanted {
        let path = assets
            .join("objects")
            .join(&object.hash[..2])
            .join(&object.hash);
        if file_has_sha1(&path, &object.hash)? {
            continue;
        }
        let client = client.clone();
        let semaphore = Arc::clone(&semaphore);
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await?;
            let download = Download {
                id: None,
                sha1: object.hash.clone(),
                url: format!("{ASSET_CDN}/{}/{}", &object.hash[..2], object.hash),
            };
            download_verified(&client, &download, &path).await
        });
    }
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

fn audio_objects(index: AssetIndex) -> Vec<AssetObject> {
    index
        .objects
        .into_iter()
        .filter(|(name, _)| {
            name == "minecraft/sounds.json" || name.starts_with("minecraft/sounds/")
        })
        // Several logical sound names may point to the same content-addressed
        // object. Keying by hash prevents concurrent tasks from racing over the
        // same temporary file.
        .map(|(_, object)| (object.hash.clone(), object))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

fn ensure_bedrock_models(root: &Path) -> Result<PathBuf> {
    let checkout = root.join("bedrock-samples");
    let models = checkout.join("resource_pack/models/entity");
    if models.join("cow.geo.json").exists() {
        return Ok(models);
    }
    eprintln!("Downloading Mojang Bedrock entity samples");
    std::fs::create_dir_all(root)?;
    let status = Command::new("git")
        .args(["clone", "--depth=1", "--filter=blob:none", "--sparse"])
        .arg("https://github.com/Mojang/bedrock-samples.git")
        .arg(&checkout)
        .status()
        .context("run git to fetch Mojang bedrock-samples")?;
    if !status.success() {
        bail!("git could not fetch Mojang/bedrock-samples");
    }
    let status = Command::new("git")
        .current_dir(&checkout)
        .args(["sparse-checkout", "set", "resource_pack/models/entity"])
        .status()?;
    if !status.success() || !models.exists() {
        bail!("Bedrock sample checkout does not contain entity models");
    }
    Ok(models)
}

fn eula_accepted(path: &Path) -> Result<bool> {
    Ok(path.exists()
        && std::fs::read_to_string(path)?
            .lines()
            .any(|line| line.trim() == "eula=true"))
}

fn set_property(path: &Path, key: &str, value: &str) -> Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut found = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if line
            .split_once('=')
            .is_some_and(|(candidate, _)| candidate == key)
        {
            lines.push(format!("{key}={value}"));
            found = true;
        } else {
            lines.push(line.to_owned());
        }
    }
    if !found {
        lines.push(format!("{key}={value}"));
    }
    std::fs::create_dir_all(path.parent().context("property path has no parent")?)?;
    std::fs::write(path, format!("{}\n", lines.join("\n")))?;
    Ok(())
}

fn exit_with(status: ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        bail!("child process exited with {status}")
    }
}

fn print_help() {
    println!(
        "Crabcraft launcher\n\n\
         cargo run -p crabcraft-launcher -- client [--version 1.20.1] [ADDR] [USERNAME]\n\
         cargo run -p crabcraft-launcher -- server [--version 1.20.1] --accept-eula [--offline]\n\n\
         cargo run -p crabcraft-launcher -- both [--version 1.20.1] --accept-eula [--username Ferris]\n\n\
         Client assets are cached under assets-cache/. Server files and worlds are cached there too."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_version_option_and_forwards_client_arguments() {
        let args = vec![
            "localhost:25565".into(),
            "--version".into(),
            "1.21.5".into(),
        ];
        let (version, rest) = take_option(&args, "--version", DEFAULT_VERSION).unwrap();
        assert_eq!(version, "1.21.5");
        assert_eq!(rest, ["localhost:25565"]);
    }

    #[test]
    fn property_update_replaces_existing_value() {
        let dir = std::env::temp_dir().join(format!("crabcraft-launcher-{}", std::process::id()));
        let path = dir.join("server.properties");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "motd=test\nonline-mode=true\n").unwrap();
        set_property(&path, "online-mode", "false").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "motd=test\nonline-mode=false\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audio_downloads_are_deduplicated_by_content_hash() {
        let index = AssetIndex {
            objects: BTreeMap::from([
                (
                    "minecraft/sounds/one.ogg".to_owned(),
                    AssetObject {
                        hash: "aabb".to_owned(),
                    },
                ),
                (
                    "minecraft/sounds/two.ogg".to_owned(),
                    AssetObject {
                        hash: "aabb".to_owned(),
                    },
                ),
                (
                    "minecraft/textures/ignored.png".to_owned(),
                    AssetObject {
                        hash: "ccdd".to_owned(),
                    },
                ),
            ]),
        };

        let objects = audio_objects(index);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].hash, "aabb");
    }
}
