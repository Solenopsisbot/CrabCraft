# Protocol support

Crabcraft defaults to protocol 763. Choose another profile with
`CRABCRAFT_PROTOCOL=<number-or-version>`.

| Minecraft | Protocol | Status | Notable differences |
|---|---:|---|---|
| 1.20 / 1.20.1 | 763 | Primary, live-tested | Direct Login-to-Play, classic root NBT |
| 1.20.2 | 764 | Implemented | Login acknowledgement, Configuration state, registry codec, network-NBT chunks, chunk batches, shifted packet IDs |
| 1.20.3 / 1.20.4 | 765 | Core path live-tested on 1.20.4 | NBT chat components, score reset/formats, UUID resource packs/removal, play reconfiguration |
| 1.20.5 / 1.20.6 | 766 | Core path live-tested on 1.20.6 | Split configuration registries, revised packet maps, data-component item stacks |
| 1.21 / 1.21.1 | 767 | Core path live-tested on 1.21.1 | Tricky Trials registries, VarInt item counts, jukebox component, configuration report/link packets |
| 1.21.2 / 1.21.3 | 768 | Core path live-tested on 1.21.3 | Bundle-era packet maps, movement flags/velocity corrections, particle preference, split inventory updates, expanded components |
| 1.21.4+ | 769+ | Not implemented | New registries and incremental packet schemas |

## Versioning approach

Protocol 763 is the canonical profile for unchanged play payloads. Later profiles
translate inbound and outbound IDs at known insertion points. A packet receives a
version-specific decoder whenever its payload changed; accepting a shifted ID is
not considered sufficient support.

Blocks, block states, items, and entities are also numeric wire registries. The
client selects committed generated 1.20.1, 1.20.2, 1.20.3, 1.20.5, 1.21, or 1.21.3
tables before it loads assets or decodes a world. This matters even within
1.20.x: 1.20.2 changed
some block-state ranges, while 1.20.3 inserted the crafter, new copper/tuff
families, the breeze, and wind charge, shifting most later IDs.

Protocol 766 adds the Armored Paws registries and therefore selects its own
blocks, items, and entities as well.

Protocol 767 selects generated Tricky Trials block/item tables while retaining
the 766 entity table, matching the authoritative data-set inheritance.

Protocol 768 selects independently generated 1.21.3 block/state, item, and
entity tables. Packet translation is deliberately allow-listed for consumed
clientbound packets because 768 inserts position/minecart/inventory/recipe-book
packets at several unrelated points. Changed payloads use explicit codecs for
teleport velocity, input bits, client particle settings, block-use border hits,
and chunk-batch acknowledgement.

Protocol 764+ spends time in `State::Configuration` after Login Success. Registry
data is retained for Join Game dimension and biome interpretation. Protocol 765
can transition from Play back into Configuration and acknowledges that transition
before reusing the configuration loop.

Classic NBT includes a root name. Network/anonymous NBT omits it. Using the wrong
reader shifts every following field, so chunk heightmaps, block entities, registry
data, and component-era text explicitly select the correct form.

The 765 live fixture uses an official vanilla 1.20.4 offline server and exercises
Login, Configuration registry transfer, Join Game, dimension lookup, chunk and
entity streams, spawn synchronization, inventory, movement/keepalive, and chat.
Optional presentation packets still retain byte-level regression coverage so a
server need not emit every UI feature during the smoke test.

The 766 fixture uses Mojang's official vanilla 1.20.6 server jar (SHA-1
`145ff0858209bcfc164859ba735d4199aafa1eea`). It has completed Login,
Configuration, split registry merge, Join Game, chunks, spawn, inventory,
entities, movement/keepalive, and chat. A second smoke pass injected a custom
named/unbreakable sword and a filled map to verify that typed component patches
remain aligned through live Set Container Slot packets.

Protocol 766 `Slot` values begin with an item count (zero means empty), followed
by an item registry ID and added/removed data-component lists. The bounded codec
consumes all 56 component layouts, including recursively nested item stacks, and
retains UI-relevant custom data, names, map IDs, and block-entity data in the
client's metadata representation. Never decode these packets with the legacy
present/id/count/classic-NBT layout.

The 767 fixture uses Mojang's official vanilla 1.21.1 server jar (SHA-1
`59353fb40c36d304f2035d51e7d6e6baa98dc05c`). It completed the same core
Login-to-Play path with 95 entities. A component pass injected a custom-named
mace, filled map, and Music Disc 5 and decoded all three inventory updates. In
767, Slot count is a VarInt, attribute modifiers omit their UUID field, and the
inserted `jukebox_playable` component shifts component IDs 42 and above.

The 768 fixture uses Mojang's official vanilla 1.21.3 server jar (SHA-1
`45810d238246d90e811d896f87b14695b7fb6839`). It completed Login,
Configuration, split registries, Join Game, teleport/velocity synchronization,
chunks and batch acknowledgements, spawn, chat, movement/keepalive, inventory,
and a 96-entity stream. A second pass injected a filled map and a Golden Apple
with overridden `food` and `consumable` components; both Set Container Slot
updates decoded and the session reached its deadline.

Protocol 768 expands the component registry from 57 to 67 entries. Its bounded
codec handles inserted item-model, consumable/remainder/cooldown, resistance,
enchantability/equippable/repairable/glider/tooltip/death-protection types,
changed custom-model and food layouts, and recursively nested 768 stacks. The
new recipe display/book packets use numeric display IDs. Crafting-shaped,
crafting-shapeless, stonecutter, furnace, and smithing displays are bounded and
recursively decoded; crafting and stonecutter entries feed the existing paged UI,
add/remove updates maintain unlock state, and placement sends the numeric ID.
Unresolvable tag-only displays remain safe empty alternatives rather than
guessing registry membership.

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
