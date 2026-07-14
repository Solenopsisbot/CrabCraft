# Architecture

Crabcraft is a Cargo workspace organized so protocol input, simulation, and
presentation remain independently testable.

## Runtime data flow

```text
TCP -> framing/compression/encryption -> versioned packet decoding
    -> semantic ClientEvent -> deterministic ClientCore -> Arc<ClientSnapshot>
                                                    \-> presentation readers
 input/UI -> bounded ClientCommand queue -> versioned wire adapter -> TCP
 world updates -> revisioned World -> immutable region snapshot -> mesh worker
 resource pack -> bounded CPU worker -> validated generation -> GPU commit
```

`crab-net` owns framing, compression, encryption, connection state, and applies
the packet-ID mapper selected for a session. `crab-protocol` owns primitive
codecs, NBT, and versioned packet layouts. `crab-core` owns the renderer-neutral session model: protocol profiles,
typed commands and events, deterministic state transitions, coherent snapshots,
the screen stack, and semantic replay records. The `crabcraft` crate adapts wire
packets to those types and temporarily mirrors feature-specific legacy stores as
their domains migrate. Rendering never owns or mutates authoritative game state.

Every connection creates one immutable `SessionContext`. It pairs a
`ProtocolVersion` with its `RegistrySet`, so concurrent or sequential sessions
cannot change each other's block, item, entity, collision, or packet mapping.
The old process-global registry accessors remain compatibility wrappers; new
code must accept a `SessionContext` or `RegistrySet` explicitly.

All presentation/network requests cross the bounded `ClientCommand` queue.
Configuration may consume its own commands while preserving the order of
deferred play commands. The play adapter is the only layer that turns commands
into version-specific bytes. Incoming lifecycle and player-state changes become
`ClientEvent`s and are applied serially by `ClientCore`; readers receive one
`Arc<ClientSnapshot>` revision rather than observing unrelated locks mid-update.

Chunk storage is copy-on-write. Network updates increment per-chunk revisions and
coalesce the chunk plus edge neighbours in an event-driven dirty set. Meshing
captures a structurally shared 3x3 `WorldSnapshot`, releases the world lock, and
runs off the render thread. Results travel through a bounded channel and carry
both world dependency revisions and an asset generation; stale results are
discarded before GPU upload.

Resource-pack changes are transactional. A bounded worker constructs and
validates the complete CPU `ResourceSet` without blocking winit. The render
thread commits one matching generation atomically, remeshes loaded chunks, and
only then acknowledges success to the server. A failed generation leaves the
currently displayed resources untouched.

Modal UI is represented by a tested `ScreenStack` rather than unrelated boolean
flags. This makes nesting explicit (for example Pause -> Options -> Controls)
and gives new screens one lifecycle model for open, close, replace, and escape.

Set `CRABCRAFT_REPLAY_OUT=/path/to/replay.json` to record a deterministic
semantic session trace. Replays contain protocol context, ordered events and
commands, and expected snapshot revisions. User-authored chat, command, book,
and rename text is redacted. Replays intentionally sit above packet bytes so
they remain useful when a protocol adapter changes.

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
The camera arm is ray-tested against the world and shortened before projection,
so both third-person directions stay on the player side of nearby solid blocks.

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

- `crab-core`: protocol/session profiles, commands, events, state snapshots,
  screen stack and semantic replay.
- `crab-protocol`: byte codecs, classic/network NBT, packet traits, per-version packets.
- `crab-net`: async transport, framing, zlib, AES-CFB8, protocol-state tracking.
- `crab-world`: section palettes, chunks, block updates, biomes and dimension data.
- `crab-registry`: generated block/item/entity tables and state interpretation.
- `crab-physics`: collision, ray casting, movement and fluid forces. Collision
  consumes deduplicated per-state voxel boxes selected by `crab-registry` for
  the active protocol profile; the committed tables are generated from
  minecraft-data's vanilla voxel-shape extraction and contain no game assets.
- `crab-assets`: jar/resource-pack models, textures, GUI and entity atlas loading.
- `crab-render`: mesh generation, shaders, GPU state, HUD geometry.
- `crab-audio`: sound-event resolution and playback.
- `crab-auth`: Microsoft authentication and Minecraft session encryption helpers.
- `crabcraft`: session orchestration, shared state, input, menus and window loop.

## Extension seams

New behavior should enter through the narrowest stable seam:

- Add wire layouts to `crab-protocol`, then expose only verified unchanged ID
  mappings in `crab-core::wire`.
- Add player intent as `ClientCommand`; encode it only in the network adapter.
- Add authoritative state as `ClientEvent` plus a deterministic `ClientCore`
  transition and snapshot field.
- Add modal UI with a `UiScreen` variant and `ScreenStack` transition.
- Add background computation with an immutable input snapshot, bounded queue,
  generation/revision stamp, and stale-result rejection.
- Add per-session lookup data to `SessionContext` rather than global mutable
  selection.

The complete checklist and examples are in [Extending Crabcraft](EXTENDING.md).

## Invariants

- Network-controlled lengths are validated before allocation.
- Commands, worker requests, and worker results are bounded.
- Snapshots are coherent and worker results are revision checked.
- Protocol profiles translate canonical 1.20.1 packet IDs only when payloads are
  unchanged; changed payloads use explicit version-aware decoders.
- The repository never embeds proprietary Minecraft assets.
- Optional presentation packet failures should degrade a feature, not corrupt the
  stream. Core packets such as Join Game and Chunk Data fail loudly on bad layouts.
