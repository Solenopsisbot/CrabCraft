# Architecture

Crabcraft is a Cargo workspace organized so protocol input, simulation, and
presentation remain independently testable.

## Runtime data flow

```text
TCP -> framing/compression/encryption -> versioned packet decoding
    -> Shared game state -> physics/input outboxes -> serverbound packets
                         -> meshing/assets -> wgpu renderer + HUD
                         -> audio event resolver -> output device
```

`crab-net` owns framing, compression, encryption, connection state, and packet-ID
translation. `crab-protocol` owns primitive codecs, NBT, and versioned packet
layouts. The `crabcraft` network task parses play packets into `Shared`, a set of
small mutex-protected stores and bounded outboxes. The window loop reads those
stores without giving rendering ownership of authoritative game state.

Chunk meshing runs off the render thread. Network updates mark a chunk and its
edge neighbours dirty; worker results are uploaded under a per-frame budget.
Resource-pack changes construct a validated vanilla-fallback archive, rebuild CPU
atlases, upload replacement GPU resources, remesh loaded chunks, and only then
acknowledge success to the server.

Entity assets are assembled at startup from user-provided sources: Java textures
from the selected client jar and compatible box/bone geometry from Mojang's
Bedrock sample pack. Name aliases bridge Java family texture directories and
shared/versioned geometry names. The resulting atlas is keyed by the active
protocol's generated entity IDs; unresolved entries deliberately retain a
registry-sized diagnostic box. See [Asset pipeline](ASSETS.md).
Special entities stay data-driven where their appearance is not a mob model:
dropped stacks use item/block atlases, while falling blocks retain Spawn Entity's
global block-state ID and resolve it through the active versioned block registry.
For modelled entities, interpolation, limb swing, attack/hurt reactions, head
yaw, and Pose metadata are combined when CPU mesh vertices are rebuilt. Protocol
766+ uses the shifted Pose metadata serializer introduced with the 1.20.5
particle-list insertion; older profiles retain the preceding serializer ID.

The window renderer orders its frame as world geometry, HUD backgrounds, the
inventory's depth-cleared 3D player viewport, then HUD item/text foregrounds.
The preview reuses the entity atlas and humanoid mesh but owns a separate camera
uniform and a viewport derived from the vanilla inventory texture's pixel
bounds. This keeps world depth from clipping the player and keeps slot icons and
tooltips above the model.

Window-local presentation preferences stay outside authoritative game state.
The pause/options UI updates camera FOV, mouse sensitivity, and the winit
fullscreen mode immediately; none of those settings mutate network-owned
`Shared` data or packet behavior.

Camera perspective is also window-local. F5 cycles first person and two
third-person orbit directions; third-person views append the local humanoid to
the model vertex stream using the same pose/walk/swing transforms as remote
players while suppressing first-person hand quads.

Recipe UI state normalizes versioned declarations into the same crafting and
stonecutter records. Protocols through 767 use namespaced recipe IDs; protocol
768 recursively decodes slot/recipe displays, stores numeric display IDs as the
stable UI key, and converts the selected key back to a VarInt placement request.

Inventory metadata is kept parallel to lightweight, copyable item stacks for both
the player inventory and an open server container. Protocol 768's recursive bundle
component decoder projects nested IDs/counts into that metadata; the window layer
uses it for tooltip selection while the network layer owns the version-specific
selection packet.

Protocol 769 reuses 768's allow-listed clientbound ID map because the numeric IDs
are stable, but branches at the payload boundary for changed fields. Its
serverbound mapper layers the split Pick Item and Player Loaded insertions over
the 768 map; packets with new bodies or semantic timing use dedicated codecs and
unmapped sends. This keeps older packet structs honest and makes the next-version
diff auditable.

Protocol 770 has its own allow-listed clientbound and serverbound maps because
the experience-orb removal and game-test insertions shift separate numeric
ranges. Changed bodies stay at explicit version boundaries: chunk decoding owns
the typed heightmap-array prefix, the chat codec owns its checksum trailer, and
the item codec owns the reorganized 96-entry component registry. Generated
1.21.5 block/state, item, and entity tables are selected before either world or
presentation code resolves numeric IDs.

## Crate boundaries

- `crab-protocol`: byte codecs, classic/network NBT, packet traits, per-version packets.
- `crab-net`: async transport, framing, zlib, AES-CFB8, protocol-state tracking.
- `crab-world`: section palettes, chunks, block updates, biomes and dimension data.
- `crab-registry`: generated block/item/entity tables and state interpretation.
- `crab-physics`: collision, ray casting, movement and fluid forces.
- `crab-assets`: jar/resource-pack models, textures, GUI and entity atlas loading.
- `crab-render`: mesh generation, shaders, GPU state, HUD geometry.
- `crab-audio`: sound-event resolution and playback.
- `crab-auth`: Microsoft authentication and Minecraft session encryption helpers.
- `crabcraft`: session orchestration, shared state, input, menus and window loop.

## Invariants

- Network-controlled lengths are validated before allocation.
- Protocol profiles translate canonical 1.20.1 packet IDs only when payloads are
  unchanged; changed payloads use explicit version-aware decoders.
- The repository never embeds proprietary Minecraft assets.
- Optional presentation packet failures should degrade a feature, not corrupt the
  stream. Core packets such as Join Game and Chunk Data fail loudly on bad layouts.
