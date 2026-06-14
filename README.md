# Crabcraft

A Minecraft **Java Edition 1.20.1** client written from scratch in **pure Rust**.

The long-term goal is a full, expandable, multi-version client that can join
vanilla servers. Today Crabcraft is a complete *headless* client: it connects to
1.20.1 servers, logs in, holds the connection like a real player, chats, and
maintains an accurate in-memory world model — all verified against a real
vanilla server. Rendering and authentication are the next milestones.

## What works today

Verified end-to-end against a vanilla 1.20.1 server (offline mode):

- TCP connection, handshake, and **login** (with packet compression)
- Staying connected: **KeepAlive**, spawn (teleport confirm), position reporting
- **Chat** (sending; reading system chat)
- **World model**: decodes Chunk Data (palettes + bit-packed long arrays) into a
  queryable block world; tracks chunk load/unload and block updates
- **Block registry**: resolves block-state IDs to names like
  `minecraft:grass_block` (1003 blocks)
- **Dimension extents**: reads real `min_y`/`height` from the Join Game NBT codec

Not yet: rendering, online-mode auth/encryption, physics/collision.

## Architecture

A Cargo workspace of focused crates. Game state never depends on rendering, and
packet definitions are namespaced per protocol version so new versions are added
side-by-side rather than by rewriting the core.

| Crate | Responsibility |
|-------|----------------|
| `crab-protocol` | Wire codecs (VarInt/VarLong/String/UUID/Position), NBT, the `Packet` trait, and per-version packet definitions (`versions::v1_20_1`) |
| `crab-net` | Async connection: length framing, the zlib compression sublayer, connection state |
| `crab-world` | Chunk/section decoding (paletted containers), the `World` block store, dimension extents |
| `crab-registry` | Data-driven registries (generated block-state → name table) |
| `crabcraft` | The client binary that wires it all together |

## Build & run

Requires a recent stable Rust toolchain.

```sh
cargo build
cargo test          # ~40 unit tests across the workspace
cargo clippy --all-targets
```

Run the client against an **offline-mode** 1.20.1 server:

```sh
cargo run -p crabcraft -- [ADDR] [USERNAME] [SECONDS]
# defaults: 127.0.0.1:25565  Ferris  35
```

Set `RUST_LOG=crab_net=trace` (etc.) for verbose packet logging.

### Spinning up a local test server

Crabcraft currently targets offline-mode servers (no auth/encryption yet). Using
the official server jar:

1. Download the 1.20.1 server jar from Mojang into a local dir (e.g. `run/`).
2. `echo "eula=true" > eula.txt`
3. In `server.properties` set `online-mode=false`.
4. `java -jar server.jar nogui`

> The server jar, world saves, and any Minecraft assets are **not** part of this
> repo (see `.gitignore`) — they belong to Mojang and must not be redistributed.

## Design notes

- **Data-driven & multi-version.** Packet IDs/layouts and block tables come from
  authoritative data (the vanilla data-generator reports / PrismarineJS
  `minecraft-data`). The block table is committed generated Rust
  (`crab-registry/scripts/generate_blocks.py`), so the crate has no build-time or
  runtime data dependency.
- **1.20.1 has no configuration phase** (that arrived in 1.20.2 / protocol 764);
  login transitions straight to play. The version abstraction is built to absorb
  that difference when those versions are added.

## Roadmap

- [ ] `wgpu` rendering: window, camera, chunk meshing — see the world
- [ ] Microsoft auth + AES encryption — join online-mode servers
- [ ] Block collision/solidity data — physics and render culling
- [ ] More protocol versions (1.20.2+, 1.21, …) as sibling modules
- [ ] (Far future, maybe) Forge mod support — see the note below

### On Forge mods

Forge mods are JVM bytecode that hook directly into Minecraft's own Java classes
via Mixin/coremods. Supporting them in a pure-Rust client would require not just
a JVM but a byte-compatible reimplementation of Minecraft's internal API surface
— effectively a second multi-year project. It is deliberately parked at the very
end, and if pursued would more likely be a JVM-bridge hybrid than pure emulation.

## License

Dual-licensed under MIT or Apache-2.0, at your option.

Not affiliated with Mojang or Microsoft. "Minecraft" is a trademark of Mojang AB.
