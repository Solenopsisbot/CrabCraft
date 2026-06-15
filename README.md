# Crabcraft

A Minecraft **Java Edition 1.20.1** client written from scratch in **pure Rust**.

The long-term goal is a full, expandable, multi-version client that can join
vanilla servers. Crabcraft connects to 1.20.1 servers, logs in, holds the
connection like a real player, simulates physics, renders the world, and
(in online mode) authenticates with a Microsoft account over an encrypted
connection.

## What works today

Verified end-to-end against a vanilla 1.20.1 server (offline mode unless noted):

- TCP connection, handshake, and **login** (with packet compression)
- Staying connected: **KeepAlive**, spawn (teleport confirm), position reporting
- **Chat** (sending; reading system chat)
- **World model**: decodes Chunk Data (palettes + bit-packed long arrays) into a
  queryable block world; tracks chunk load/unload and block updates
- **Block registry**: resolves block-state IDs to names like
  `minecraft:grass_block` (1003 blocks)
- **Dimension extents**: reads real `min_y`/`height` from the Join Game NBT codec
- **Physics**: AABB-vs-voxel collision + gravity; the client is physically
  simulated and the server accepts its movement
- **Rendering**: a `wgpu` voxel renderer (face-culled meshing, depth, lighting)
  with **textures loaded from your client jar** (atlas-stitched cube models +
  **element models** so slabs/stairs/plants/lanterns render as real shapes +
  grass/foliage tint); offscreen mode + a live windowed viewer
- **Player control**: first-person WASD/jump/look in the window, driven through
  the physics sim and sent to the server
- **Block editing**: raycast-targeted **breaking** and **placing**, reconciled
  with the server
- **Entities**: tracks other players/mobs (spawn, relative move, teleport,
  destroy) and renders them as **3D models** (Bedrock geometry + jar textures,
  falling back to boxes)
- **Combat**: **left-click attacks** the mob you're aiming at (within reach) via
  a swing + InteractEntity; verified to damage and kill a mob on a live server
- **Survival vitals**: tracks **health/food**, and on death sends a respawn
  request automatically
- **HUD**: crosshair, a 9-slot hotbar outline, and health/food bars
- **Online mode**: AES-128-CFB8 encryption + the Minecraft server hash + RSA
  handshake, with Microsoft device-code login (see caveats below)

### Verification caveats

The crypto is unit-tested against known-answer vectors (NIST CFB8; the canonical
Minecraft server-hash examples). The **windowed renderer** is compile-verified
but needs a display to run, and the **Microsoft/Mojang online flows** are
implemented to spec but require a real account, so they aren't exercised in CI.

## Architecture

A Cargo workspace of focused crates. Game state never depends on rendering, and
packet definitions are namespaced per protocol version so new versions are added
side-by-side rather than by rewriting the core.

| Crate | Responsibility |
|-------|----------------|
| `crab-protocol` | Wire codecs (VarInt/VarLong/String/UUID/Position), NBT, the `Packet` trait, and per-version packet definitions (`versions::v1_20_1`) |
| `crab-net` | Async connection: length framing, zlib compression sublayer, AES-128-CFB8 encryption, connection state |
| `crab-world` | Chunk/section decoding (paletted containers), the `World` block store, dimension extents |
| `crab-registry` | Data-driven registries (generated block-state → name table) |
| `crab-physics` | AABB-vs-voxel collision and gravity |
| `crab-render` | `wgpu` renderer: chunk meshing, entity models, offscreen + windowed |
| `crab-assets` | Loads block + entity models/textures (jar + bedrock geometry); atlases |
| `crab-auth` | Session crypto (server hash, RSA) and Microsoft account login |
| `crabcraft` | The client binary that wires it all together |

## Build & run

Requires a recent stable Rust toolchain.

```sh
cargo build
cargo test          # ~40 unit tests across the workspace
cargo clippy --all-targets
```

Run the client:

```sh
cargo run -p crabcraft -- [ADDR] [USERNAME] [SECONDS]  # offline, headless
cargo run -p crabcraft -- render [ADDR] [USERNAME]     # offline, windowed (walk around)
cargo run -p crabcraft -- online [ADDR]                # online: Microsoft login, then join
cargo run -p crabcraft -- render online [ADDR]         # online + windowed
# offline defaults: 127.0.0.1:25565  Ferris  35
```

For **textures** in windowed mode, point `CRABCRAFT_JAR` at your 1.20.1 client
jar, and (optionally) `CRABCRAFT_ENTITY_MODELS` at a local
[`bedrock-samples`](https://github.com/Mojang/bedrock-samples)
`resource_pack/models/entity` directory for **3D entity models** (assets are
loaded from your own copies, never bundled — both are Mojang/EULA content):

```sh
CRABCRAFT_JAR=/path/to/1.20.1.jar \
CRABCRAFT_ENTITY_MODELS=/path/to/bedrock-samples/resource_pack/models/entity \
  cargo run --release -p crabcraft -- render 127.0.0.1:25565
```

Entity geometry is parsed from Bedrock `.geo.json` and textured with the Java
textures from your jar; entities without a loaded model render as coloured boxes.

Windowed controls: **WASD** move · **Space** jump · **mouse** (or arrow keys)
look · **left-click** attack a mob in your sights, else break the targeted block
· **right-click** place · **Esc** quit. Movement is
client-physics-simulated and sent to the server. (Run with `--release` for
smooth framerates — debug builds mesh chunks slowly.)

Render the bundled synthetic test world to a PNG (headless, no server needed):

```sh
cargo run -p crab-render --example offscreen -- out.png
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

- [x] `wgpu` rendering: meshing, camera, offscreen + windowed
- [x] Microsoft auth + AES encryption — join online-mode servers
- [x] Block collision + gravity (physics)
- [x] Texture atlas from the client jar (full-cube blocks)
- [x] First-person player movement (WASD/jump/mouse-look), sent to the server
- [x] Block breaking + placing (raycast) with a crosshair
- [x] Per-chunk mesh caching (only dirty chunks rebuild)
- [x] Entity tracking + 3D models (Bedrock geometry + jar textures)
- [x] Background chunk meshing (smooth frames)
- [x] Melee combat (attack mobs), health/food tracking + death-respawn
- [x] Minimal HUD (crosshair, hotbar outline, health/food bars)
- [x] Element block models (slabs/stairs/plants/lanterns render as real shapes)
- [ ] Blockstate variants & multipart (fence/wall connections, rotated stairs), biome tint
- [ ] Entity animation/interpolation + per-mob hitbox models
- [ ] Precise per-block collision shapes (slabs/stairs/fluids)
- [ ] Inventory GUI, item rendering in the hotbar, crafting, sounds
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
