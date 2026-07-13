# Asset pipeline

Crabcraft does not redistribute Minecraft assets. Rendering and audio load data
at runtime from copies owned by the user, while generated registry tables in the
repository contain only names, numeric IDs, dimensions, and state metadata.

## Runtime inputs

| Variable | Input | Used for |
|---|---|---|
| `CRABCRAFT_JAR` | A Java Edition client jar | Block/item textures and models, entity skins, GUI sprites, fonts, particles, and destroy stages |
| `CRABCRAFT_ENTITY_MODELS` | Mojang `bedrock-samples/resource_pack/models/entity` | Box geometry and bone pivots for 3D entities |
| `CRABCRAFT_ASSETS` | A launcher `assets` directory | Indexed sound objects |
| `CRABCRAFT_ASSET_INDEX` | Launcher asset-index name | Selects the JSON index under `assets/indexes`; defaults to `5` |

The entity-model path may point at the sample repository root, its
`resource_pack` directory, or the final `models/entity` directory. The loader
locates the model directory without copying it into the project.

## Blockstate and model resolution

For every loaded block, the asset pipeline reads the vanilla
`blockstates/<name>.json` definition and matches its `variants` and `multipart`
conditions against the active protocol registry's generated property schema.
This preserves the registry's property order and values instead of inferring
state radices from block names. Multipart `OR`/`AND` conditions, pipe-separated
values, weighted alternatives, model rotations, per-face UV rotations, and
`uvlock` are carried into chunk meshing. The model loader then follows parent
chains and texture variables before adding the resolved textures to the atlas.

Legacy family-specific model lookups remain a non-fatal fallback for incomplete
or custom packs. A missing blockstate, model, or texture never causes a client
jar to be copied or extracted into the repository.

Item models use the same parent-chain resolver. Items with resolved element
geometry retain their inherited `ground` display rotation, translation, and
scale when rendered as dropped entities; generated flat-layer items remain
camera-facing sprites. Falling-block entities are different from dropped items:
their Spawn Entity data is an exact global block-state ID, so they select that
state's variant/multipart geometry rather than an inventory item model.

## Entity model resolution

Java entity models are code-defined and are not present in a client jar. The
renderer therefore combines Bedrock sample geometry with the corresponding Java
texture. Direct matches use `<entity>.geo.json`; an explicit alias table handles
shared models, versioned Bedrock filenames, Java family texture directories,
vehicle variants, and projectiles. Both `.geo.json` and the few plain `.json`
geometry files in the sample pack are accepted.

The atlas is keyed by the selected protocol registry's entity type ID. This is
important because IDs move between releases even when the asset names do not.
Missing geometry or textures are non-fatal: the entity retains its generated
registry dimensions and renders as a coloured bounds box. Dropped items and
falling blocks use their item/block rendering paths rather than mob geometry.
Humanoid equipment uses inflated copies of the matching model bones, so armour
follows movement and authoritative pose metadata instead of remaining as
axis-aligned boxes around the entity.

The resolver covers every ordinary entity in the supported registry through a
direct Mojang sample filename or an explicit tested alias. Java texture aliases
also cover climate variants and relocated projectile/family textures while
retaining older direct-path fallbacks. Because the Bedrock sample pack does not
publish boats, Crabcraft supplies a small Java-style hull/raft geometry and
still reads each wood/chest texture from the user's jar. Item-shaped projectiles
use item-atlas sprites, primed TNT uses its block model, and entity bones named
as wings, fins, tails, or paddles receive continuous procedural motion in
addition to walk, attack, hurt, and pose animation.

When adding an alias:

1. Confirm the entity name and ID in the matching generated registry.
2. Confirm the geometry filename in an unmodified `bedrock-samples` checkout.
3. Confirm the texture path inside a legitimately obtained client jar.
4. Add a resolver test; do not add either source asset to the repository.

## Resource packs

Server resource packs are validated, layered above the local vanilla jar in
server order, and rebuilt as an atomic runtime asset set. A pack must contain a
valid `pack.mcmeta`; downloads are bounded and an advertised SHA-1 is verified.
Removing a UUID-addressed layer rebuilds the stack so lower-priority overrides
become visible again. Loaded chunks are remeshed before the client reports pack
success.

## Repository policy

Client/server jars, downloaded packs, launcher object stores, extracted textures,
Bedrock samples, world saves, and authentication data are ignored and must never
be committed. See [Contributing](../CONTRIBUTING.md) and
[Security](../SECURITY.md) for the public-repository rules.
