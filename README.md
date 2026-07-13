# Crabcraft

A Minecraft **Java Edition 1.20.1–1.21.5** client written from scratch in **pure Rust**.

[![CI](https://github.com/Solenopsisbot/CrabCraft/actions/workflows/ci.yml/badge.svg)](https://github.com/Solenopsisbot/CrabCraft/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

The long-term goal is a full, expandable, multi-version client that can join
vanilla servers. Crabcraft connects to 1.20.1 servers, logs in, holds the
connection like a real player, simulates physics, renders the world, and
(in online mode) authenticates with a Microsoft account over an encrypted
connection.

Protocol 763 (1.20/1.20.1) is the default. Set `CRABCRAFT_PROTOCOL=764`, `765`,
`766`, `767`, `768`, `769`, `770`, or a matching version string (`1.20.2` through
`1.21.5`) for newer servers. Protocol 764+ includes the Configuration state, registry transfer,
network-NBT chunk data, chunk-batch acknowledgement, and versioned packet-ID
profiles. Protocol 765 additionally handles NBT text components, UUID-addressed
resource packs and removal, score reset/format packets, and play-to-configuration
transitions.

Protocol 766 adds split configuration registries, revised packet maps, and the
data-component item-stack format used by inventory, equipment, dropped items,
recipes, maps, books, and container clicks.

Protocol 767 keeps the 766 play packet map while selecting Tricky Trials block
and item registries and its VarInt-count component stack revision.

Protocol 768 adds the 1.21.2/1.21.3 registries, bundle-era packet maps, revised
teleport velocity and movement/input payloads, particle preferences, split
cursor/player-inventory updates, the expanded 67-type component registry, and
scroll-selectable bundle tooltips.

Protocol 769 adds the 1.21.4 Pale Garden block/item/entity registries, the
`Player Loaded` acknowledgement, split pick-item packet map, direct component
slot updates, revised player-list/particle fields, and changed held-item and
vehicle payloads.

Protocol 770 adds the 1.21.5 Spring to Life registries, the revised chunk
heightmap array, chat checksum, shifted game-test/play packet maps, and the
reorganized 96-type item-component registry.

Crabcraft is under active development. The feature inventory below distinguishes
implemented behavior from the remaining parity work; it is not yet a drop-in
replacement for Mojang's client.

## What works today

The core path is verified end-to-end against vanilla 1.20.1, 1.20.4, 1.20.6,
1.21.1, 1.21.3, 1.21.4, and 1.21.5 servers (offline mode unless noted), with protocol codecs and mappings
tested for every supported profile:

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
  the physics sim and sent to the server, with **Control sprint** and
  **Shift sneak** (including crouched speed, eye height, and server pose state),
  plus reliable vanilla-style **double-Space Creative/Spectator flight**,
  Spectator noclip, and **F main/offhand swapping** with local prediction
- **Camera perspectives**: **F5** cycles first person, third-person rear, and
  third-person front views; third person renders the local animated player and
  hides first-person hand overlays, with camera distance pulled forward by
  nearby blocks to avoid wall clipping
- **Advanced movement**: sprint-swimming with low pose/eye height and
  equipped-Elytra fall-flying initiation with reduced glide gravity
- **Vehicles**: local mount/dismount tracking, camera/seat synchronization,
  server-authoritative ridden-entity movement, horse-style steering/jump input,
  and predicted boat turning, travel, and individual paddle controls
- **Fluid movement**: water/lava detection, reduced movement, buoyant gravity,
  terminal speed, **Space** ascent, and **Shift** descent
- **World ambience**: server-synchronized day/night sky brightness plus
  rain/thunder darkening from vanilla game-state events, with matching dynamic
  terrain illumination and depth-tested moving sun/moon geometry
- **Particles and fluids**: bounded server-particle simulation with motion,
  gravity, lifetime, and distance filtering, plus alpha-blended water/lava
 - **Block editing**: raycast-targeted **breaking** and **placing**, reconciled
   with the server, with the **destroy-stage crack overlay** animating on the
   block you're mining
- **Entities**: tracks other players/mobs (spawn, relative move + rotation,
   teleport, metadata, destroy) and renders them as **3D models** (Bedrock
   geometry incl. bone rest rotations + jar textures, falling back to boxes),
   **smoothly interpolated** with a **procedural walk animation** and **facing
   their body yaw plus independent server-driven head yaw**; shared-model/variant mobs are aliased to the right geo+skin so
   far fewer render as boxes; slimes/magma cubes scale to their size and
   **dropped items show their item icon**
- **Entity presentation**: packet-driven arm swings, hurt reactions, independent
  head rotation, metadata-driven crouch/swim/glide/sleep/death/sit poses,
  main/offhand items, visible material-coloured armour layers,
  bobbing/rotating 3D per-face dropped block models, and full-scale falling
  blocks selected from the server's exact block-state ID
- **First-person presentation**: visible main/offhand items with attack/use swing
  motion, including immediate feedback when swapping hands
- **Maps**: authoritative map IDs, scale/lock state, partial pixel updates,
  markers and labels, plus a two-handed filled-map view using vanilla map-color
  shading when a map is held
- **Server resource packs**: optional/forced prompt UI, accept/decline protocol
  statuses, bounded HTTPS download, SHA-1 and `pack.mcmeta` validation,
  vanilla-fallback asset layering, UUID-addressed multi-pack priority stacks and
  removal, and live block/item/GUI/entity/crack atlas replacement with
  loaded-chunk remeshing before success is acknowledged
- **Signs and books**: chunk and incremental block-entity sign text (both 1.20
  sides) rendered in-world; held written books are readable, while writable
  books support editing, pagination, saving, title entry, and signing through
  the vanilla Edit Book packet
- **Combat**: **left-click attacks** the mob you're aiming at (within reach) via
  a swing + InteractEntity; verified to damage and kill a mob on a live server
- **Survival vitals**: tracks **health/food**, and on death sends a respawn
  request automatically
- **HUD**: rendered with the **real Minecraft GUI textures** + bitmap font —
  hotbar widget (number-key/scroll selection), **hearts/hunger + XP bar & level**
  (from `icons.png`), and **stack-size numbers** on items
- **Inventory**: open with **E** (real container texture); **click or Shift-click
  to move/swap items** (left/right-click semantics) across all 46 slots — including the
  **2×2 crafting** grid (the server returns the result) and **armour** slots
- **Containers**: server-opened chests, barrels, ender chests, and shulker boxes
  use their vanilla 1–6 row GUI, with clicks, Shift-click, and close synchronization
- **Furnaces**: furnace, blast-furnace, and smoker GUIs with input/fuel/output
  routing plus server-driven flame and cooking progress
- **Workstations**: server-authoritative 3×3 crafting table, dispenser/dropper,
  hopper, brewing stand, and enchanting-table menus with vanilla textures,
  slot layouts, clicks, Shift-click, live enchantment costs, and selectable offers
- **Item workstations**: anvil, grindstone, smithing table, and cartography table
  menus with their vanilla textures and authoritative input/output slots;
  anvils include focused rename text and server rename synchronization
- **Recipe workstations**: loom and stonecutter backgrounds, authoritative
  input/output/player slots, paged selectable pattern/recipe grids, hover and
  selected-state feedback, server menu-button synchronization, and parsing of
  the server-declared recipe registry so stonecutter pages show only matching
  recipes with their real result-item icons
- **Crafting recipe book**: shaped/shapeless declarations and server unlocks
  are retained, filtered to the active 2×2 or 3×3 grid, displayed as a paged
  result-icon panel, and placed authoritatively (Shift requests craft-all),
  including protocol 768's numeric recipe-display add/remove packets
- **Bundles**: protocol 768 retains nested component stacks, shows an interactive
  contents tooltip, and sends the selected nested item when scrolling over a bundle
- **Status effects**: authoritative add/remove/expiry tracking with Speed,
  Slowness, Haste, Mining Fatigue, Jump Boost, Levitation, and Slow Falling
  applied to local movement, mining, and gravity, plus vanilla HUD icons
- **Item/block use**: empty-hand interactions, doors/buttons/containers while
  holding blocks, air-use for food/bows/buckets/shields, and release-use packets
- **Pause/options menus**: **Esc** opens a menu with the vanilla button sprites,
  an in-game controls reference, and live FOV, mouse-sensitivity, and fullscreen
  settings
- **Chat & commands**: **T** to chat, **/** for a command; messages send (chat +
  Chat Command packets) and incoming system chat shows in an on-screen log
- **Server overlays**: action-bar text, timed titles/subtitles, stacked boss
  bars with health/style updates, synchronized world-border warnings, sidebar
  scoreboards with team prefixes/suffixes, and a latency-aware Tab player list
- **Sounds**: per-block **break / place / mining-hit / footstep** plus hurt /
   attack sounds, resolved through the real `sounds.json` events
   (`block.<group>.<event>`) so each block uses its correct sound group — loaded
   from your launcher's asset store via `rodio` (set `CRABCRAFT_ASSETS`)
  plus server-issued registered/custom sound events (mob ambience, weather,
  portals, UI, and world effects) resolved through the 1.20.1 sound registry
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
| `crab-registry` | Data-driven registries (generated block/item/entity tables, break times) |
| `crab-physics` | AABB-vs-voxel collision and gravity |
| `crab-render` | `wgpu` renderer: chunk meshing, entity models, offscreen + windowed |
| `crab-assets` | Loads block + entity models/textures (jar + bedrock geometry); atlases |
| `crab-auth` | Session crypto (server hash, RSA) and Microsoft account login |
| `crab-audio` | Loads sounds from the launcher asset store; `rodio` playback |
| `crabcraft` | The client binary that wires it all together |

See [Architecture](docs/ARCHITECTURE.md) for data flow and crate boundaries,
[Protocol support](docs/PROTOCOL.md) for the version matrix,
[Asset pipeline](docs/ASSETS.md) for runtime asset resolution, and
[Contributing](CONTRIBUTING.md) before sending a change.

## Build & run

Requires a recent stable Rust toolchain.

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
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

Entity geometry is parsed from Bedrock `.geo.json`/`.json` and textured with the
Java textures from your jar. Family-folder skins, shared mob models, projectile
models, and minecart variants are resolved explicitly; entities without both a
loaded model and texture render as coloured registry-sized boxes. See the
[asset pipeline](docs/ASSETS.md) for resolution and licensing details.

Windowed controls: **WASD** move · **Space** jump / double-tap to fly when allowed
· **F** swap main/offhand
· **mouse** (or arrow keys)
look · **left-click** attack a mob in your sights, else break the targeted block
· **right-click** place · **1-9 / scroll** select hotbar slot · **E** inventory ·
**T** chat · **/** command · **F3** debug overlay · **Esc** pause menu (Quit to
exit). Movement is client-physics-simulated and sent to the server. (Run with `--release` for smooth
framerates — debug builds mesh chunks slowly.) For sounds, also set
`CRABCRAFT_ASSETS=<.../assets>`.

Render the bundled synthetic test world to a PNG (headless, no server needed):

```sh
cargo run -p crab-render --example offscreen -- out.png
```

Set `RUST_LOG=crab_net=trace` (etc.) for verbose packet logging.

### Spinning up a local test server

For a quick local protocol test, use an offline-mode vanilla server. Online-mode
authentication and encryption are also implemented but require a licensed
Microsoft account. Using the official server jar:

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
  login transitions straight to play. Later protocols retain separate packet-ID
  profiles and use unnamed network NBT where their schemas require it.

## Roadmap

- [x] `wgpu` rendering: meshing, camera, offscreen + windowed
- [x] Microsoft auth + AES encryption — join online-mode servers
- [x] Block collision + gravity (physics)
- [x] Texture atlas from the client jar (full-cube blocks)
- [x] First-person player movement (WASD/jump/mouse-look), sent to the server
- [x] Sprint/sneak, fluid movement, Creative flight, and Spectator noclip
- [x] Sprint-swimming and Elytra fall-flying initiation/glide physics
- [x] Server-driven day/night and rain/thunder sky ambience
- [x] Block breaking + placing (raycast) with a crosshair
- [x] Per-chunk mesh caching (only dirty chunks rebuild)
- [x] Entity tracking + 3D models (Bedrock geometry + jar textures)
- [x] Background chunk meshing (smooth frames)
- [x] Melee combat (attack mobs), health/food tracking + death-respawn
- [x] Minimal HUD (crosshair, hotbar outline, health/food bars)
- [x] Element block models (slabs/stairs/plants/lanterns render as real shapes)
 - [x] Entity bone rest rotations + interpolation + procedural walk animation
 - [x] Version-correct entity pose metadata with crouch/swim/glide/sleep/death transforms
- [x] Entity metadata: slime/magma-cube size scaling, dropped-item icons
- [x] Dropped 3D item models use inherited vanilla `ground` transforms; falling
  blocks retain their exact blockstate model
- [x] Real GUI textures + bitmap font; stack-size numbers
- [x] Inventory open + click-to-move/swap items; hotbar slot switching
- [x] Chat + commands (send/receive, on-screen log)
 - [x] Sounds: per-block break / place / mining-hit / footstep / hurt / attack via `sounds.json`
 - [x] Crafting (2×2) + armour via the full inventory window; left/right clicks
 - [x] Real HUD (hearts / hunger / XP+level); pause menu with vanilla buttons
 - [x] In-game options for FOV, mouse sensitivity, and fullscreen mode
 - [x] F5 first/rear/front camera cycling with an animated local-player model
 - [x] Player model (humanoid + default skin) for other players
 - [x] Entity body/head yaw facing; block-breaking crack overlay; aliased mob models
 - [x] Bone-following armour layers inherit entity animation and pose transforms
 - [x] Registry-driven blockstate variants, multipart models, weighted choices,
   model/face rotations, UV locking, and biome tint
   - [x] Vanilla blockstate JSON is matched against generated state-property
     schemas for every supported registry profile
   - [x] State-aware top/bottom, facing, straight/inner/outer stair models
   - [x] Multipart connected fence, pane/bar, and low/tall wall models
   - [x] Open/hinged doors and trapdoors; straight, corner, ascending, and powered rails
   - [x] Multipart redstone wire connections, vertical climbs, and power tint
   - [x] Axis-correct logs/pillars; facing workstations, ladders, pumpkins, anvils,
     glazed terracotta; lit/off campfires and furnace-family models
   - [x] Registry-driven per-biome grass, foliage, and water tint with quart-cell lookup and 3×3 smoothing
 - [x] Shift-click move
 - [x] Generic container GUIs (chests/barrels/ender chests/shulker boxes)
 - [x] Furnace-family GUIs (furnace/blast furnace/smoker + live progress)
 - [x] Crafting table, enchanting table, dispenser/dropper, hopper, and brewing-stand GUIs
 - [x] Anvil, grindstone, smithing, and cartography-table GUIs
 - [x] Core movement/mining status-effect behavior and expiry
 - [x] Cursor-facing 3D player preview in the inventory screen
 - [x] Server-driven spatial mob ambient sounds with range/volume attenuation
 - [x] Java-family texture aliases plus shared projectile and minecart geometry
 - [ ] Remaining per-mob geometry and hitbox models
- [x] Precise generated per-state collision shapes for every supported registry
  - [x] Empty shapes: fluids, plants, rails, torches, redstone, signs, and similar blocks
  - [x] State-aware top/bottom/double slabs and full stair variants + 0.6-block auto-step
  - [x] Connected fences/walls/panes, gates, doors/trapdoors, snow layers, chests, beds, pots, and common low blocks
  - [x] Specialized cauldron/composter, hopper, anvil, bell, head/skull, and candle shapes
  - [x] Rare utility/decorative shapes (grindstones, lecterns, chains, sea pickles, brewing stands, lanterns)
  - [x] Exhaustive deduplicated vanilla voxel shapes for every global block state
- [x] Protocol 764 / 1.20.2 login + Configuration state, registry transfer,
  configuration keepalive/ping/resource packs, chunk batching, and shifted play IDs
- [x] Protocol 765 / 1.20.3–1.20.4 login, configuration, packet mappings,
  NBT text components, network-NBT chunks/block entities, score formats,
  resource-pack UUIDs/removal, and reconfiguration entry, live-tested through
  login, registry transfer, chunks, spawn, inventory, entities, movement, and chat
- [x] Protocol 766 / 1.20.5–1.20.6 split registries, packet mappings,
  data-component inventory/equipment/dropped-item/recipe formats, and live core path
- [x] Protocol 767 / 1.21–1.21.1 Tricky Trials registries, component-stack
  changes, configuration additions, and live core path
- [x] Protocol 768 / 1.21.2–1.21.3 registries, packet maps, movement/teleport,
  settings, inventory split packets, expanded components, and live core path
  - [x] New recipe-display/book numeric-ID UI and placement requests
  - [x] Bundle contents, scroll selection, tooltip feedback, and selection packets
- [x] Protocol 769 / 1.21.4 Pale Garden registries, Player Loaded, packet-map and
  changed-payload codecs, component inventory, and official-server live validation
- [x] Protocol 770 / 1.21.5 Spring to Life registries, shifted packet maps,
  chunk heightmap arrays, chat checksum, reorganized components, and official-server
  core/component live validation
- [ ] Protocol 771+ (newer than 1.21.5) registries and packet schemas
- [ ] (Far future, maybe) Forge mod support — see the note below

### On Forge mods

Forge mods are JVM bytecode that hook directly into Minecraft's own Java classes
via Mixin/coremods. Supporting them in a pure-Rust client would require not just
a JVM but a byte-compatible reimplementation of Minecraft's internal API surface
— effectively a second multi-year project. It is deliberately parked at the very
end, and if pursued would more likely be a JVM-bridge hybrid than pure emulation.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

Not affiliated with Mojang or Microsoft. "Minecraft" is a trademark of Mojang AB.
