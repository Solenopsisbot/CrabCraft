# Protocol support

Crabcraft defaults to protocol 763. Choose another profile with
`CRABCRAFT_PROTOCOL=<number-or-version>`.

| Minecraft | Protocol | Status | Notable differences |
|---|---:|---|---|
| 1.20 / 1.20.1 | 763 | Primary, live-tested | Direct Login-to-Play, classic root NBT |
| 1.20.2 | 764 | Implemented | Login acknowledgement, Configuration state, registry codec, network-NBT chunks, chunk batches, shifted packet IDs |
| 1.20.3 / 1.20.4 | 765 | Implemented; live validation pending | NBT chat components, score reset/formats, UUID resource packs/removal, play reconfiguration |
| 1.20.5 / 1.20.6 | 766 | Not implemented | Data-component item stack redesign |
| 1.21+ | 767+ | Not implemented | New registries, packets and incremental schema changes |

## Versioning approach

Protocol 763 is the canonical profile for unchanged play payloads. Later profiles
translate inbound and outbound IDs at known insertion points. A packet receives a
version-specific decoder whenever its payload changed; accepting a shifted ID is
not considered sufficient support.

Protocol 764+ spends time in `State::Configuration` after Login Success. Registry
data is retained for Join Game dimension and biome interpretation. Protocol 765
can transition from Play back into Configuration and acknowledges that transition
before reusing the configuration loop.

Classic NBT includes a root name. Network/anonymous NBT omits it. Using the wrong
reader shifts every following field, so chunk heightmaps, block entities, registry
data, and component-era text explicitly select the correct form.

## Adding a protocol

1. Obtain authoritative clientbound/serverbound maps and packet schemas.
2. Diff packet names and layouts against the nearest supported version.
3. Add constants and per-state packets under `crab-protocol/src/versions/`.
4. Add segmented ID mappings only for unchanged payloads.
5. Add explicit decoders for changed payloads and registry/item semantics.
6. Test real insertion points, login/configuration transitions, NBT forms, and at
   least one representative changed packet.
7. Update this table and the README. Do not label the version supported until a
   vanilla server can complete its core login/configuration/chunk path.
