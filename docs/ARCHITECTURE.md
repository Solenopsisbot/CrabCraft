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
