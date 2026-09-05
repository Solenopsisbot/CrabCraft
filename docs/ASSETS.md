# Asset pipeline

Crabcraft does not redistribute Minecraft assets. Rendering and audio load data
at runtime from copies owned by the user, while generated registry tables in the
repository contain only names, numeric IDs, dimensions, and state metadata.

## Runtime inputs

For automatic setup, `cargo run -p crabcraft-launcher -- client` resolves the
selected version through Mojang's version manifest, verifies downloads by their
published SHA-1, downloads only the indexed audio objects needed by Crabcraft,
and supplies all variables below. Everything is stored under the gitignored
`assets-cache/`; no Mojang content is copied into the source tree or included in
builds.

| Variable | Input | Used for |
|---|---|---|
| `CRABCRAFT_JAR` | A Java Edition client jar | Block/item/entity textures and models, GUI sprites, fonts, particles, and destroy stages |
| `CRABCRAFT_ASSETS` | A launcher `assets` directory | Indexed sound objects |
| `CRABCRAFT_ASSET_INDEX` | Launcher asset-index name | Selects the JSON index under `assets/indexes`; defaults to `5` |

## Blockstate and model resolution

For every loaded block, the asset pipeline reads the vanilla
`blockstates/<name>.json` definition and matches its `variants` and `multipart`
conditions against the active protocol registry's generated property schema.
This preserves the registry's property order and values instead of inferring
state radices from block names. Multipart `OR`/`AND` conditions, pipe-separated
values, weighted alternatives, model rotations, per-face UV rotations, and
`uvlock` are carried into chunk meshing. The model loader then follows parent
chains and texture variables before adding the resolved textures to the atlas.
Face UVs omitted by a model are derived from the element bounds with vanilla's
direction-specific projection. Namespaced model and texture references resolve
from their owning `assets/<namespace>` tree rather than being redirected to
`minecraft`.

Legacy family-specific model lookups remain a non-fatal fallback for incomplete
or custom packs. A missing blockstate, model, or texture never causes a client
jar to be copied or extracted into the repository.

Water, bubble columns, and lava are vanilla special renderers and therefore do
not have ordinary block model JSON. The atlas loader explicitly maps their
`*_still` textures to top/bottom faces and `*_flow` textures to side faces;
water retains biome tint and the renderer's translucent opacity.
Fluid atlas entries are explicitly excluded from opaque face-culling decisions,
so a solid block face remains available to render behind a water volume.
The same client jar supplies the `mineable/*` block tags used for tool
effectiveness and the `icons.png` air-bubble sprites. Underwater visibility is
then bounded by renderer distance fog rather than treating water as an
unlimited clear volume.

Block atlas UVs address the centers of their outer texels. This keeps nearest
sampling inside the selected tile when small slab, stair, pane, or plant faces
move by subpixels, avoiding adjacent-texture flicker. Directional fallback
geometry reads generated `axis`, `facing`, and `rotation` state properties;
resolved blockstate JSON remains the primary source of model rotations.

Item models use the same parent-chain resolver. For 1.21.4 and newer assets the
loader starts at `assets/<namespace>/items/<path>.json`, follows plain models,
and selects the context-free fallback of condition, select, and range-dispatch
definitions. Generated flat models alpha-compose every declared `layerN`
texture in order instead of discarding overlays. Items with resolved element
geometry retain their inherited `ground` display rotation, translation, and
scale when rendered as dropped entities. The live window also fits resolved
element and block-state meshes into depth-cleared inventory, hotbar, recipe,
cursor, and first-person overlay passes. Block items take the block-state path
before their inventory model can flatten them; generated flat-layer tools in a
player's hand extrude the opaque 16x16 texture silhouette, including exposed
pixel edges, and use separate main/offhand transforms. Local third-person and
remote player equipment use the active protocol registry before attaching the
same held-item geometry. In-world dropped generated items remain camera-facing
sprites. Falling-block entities are different from dropped items:
their Spawn Entity data is an exact global block-state ID, so they select that
state's variant/multipart geometry rather than an inventory item model.

The GUI atlas includes the vanilla `recipe_book.png` and `recipe_button.png`
sheets from the user-provided client jar. Recipe backgrounds, cells, hover
states, and the toggle use exact source rectangles; missing sheets retain a
non-fatal colored fallback.

At startup Crabcraft reports unresolved block/item model counts and missing or
undecodable texture counts, with examples. A non-zero count usually means that
the client jar does not match the selected protocol or a resource pack has
incomplete references; Crabcraft does not guess a replacement model ID.

The remaining item fidelity boundary is explicit. Stack-dependent branches
(damage, use state, time/compass state, custom-model-data, trim, and similar
properties), special renderers, per-layer component tints, native-resolution
textures, and animated `.mcmeta` frame playback are not yet evaluated. The
context-free fallback makes the base icon/model available but is not a claim
that every runtime variant is identical to Java Edition.

## Entity model resolution

Java entity meshes are code-defined rather than JSON resources in a client jar.
The loader extracts the corresponding entity texture directly from the jar and
ships a generated Rust rest-pose table derived from the decompiled vanilla
model builders. If a custom/resource-pack archive contains compatible geometry
JSON, direct matches under `assets/minecraft/models/entity/` or
`assets/minecraft/geo/` are loaded first; otherwise the bundled vanilla table
is used, with a deterministic registry-sized model as the last fallback.
Player and boat models retain their built-in Java-style geometry.

`crates/crab-assets/src/vanilla_models.rs` is numeric generated data extracted
from the 1.20.1 client model-layer builders with CFR; the source jar and
decompiler output remain outside the repository. When a supported Minecraft
release changes model-layer builders, regenerate and review this table rather
than checking client binaries or downloaded asset archives into the project.

The atlas is keyed by the selected protocol registry's entity type ID. This is
important because IDs move between releases even when the asset names do not.
Missing textures are non-fatal: the entity retains its generated registry
dimensions and renders as a coloured bounds box. Missing geometry is covered by
the registry-sized fallback. Dropped items and falling blocks use their
item/block rendering paths rather than mob geometry.
Humanoid equipment uses inflated copies of the matching model bones, so armour
follows movement and authoritative pose metadata instead of remaining as
axis-aligned boxes around the entity.

The resolver covers every ordinary entity in the supported registry through an
explicit Java texture alias table (family folders, climate variants, shared
textures, vehicle variants, and projectiles) while retaining direct-path
fallbacks. Item-shaped projectiles use item-atlas sprites, primed TNT uses its
block model, and generated entity bones still receive walk, attack, hurt, head,
and pose animation.

The same jar supplies all 30 vanilla painting textures (`textures/painting/`),
indexed in protocol registry order. The live renderer has dedicated
protocol-backed paths for item frames (including framed item and eight-step
rotation), paintings, area-effect clouds (radius, color, and waiting state),
lightning, and block/item/text Display entities (content, transforms,
billboards, text wrapping, background, and opacity). Interaction and marker
entities are collision/selection volumes and intentionally have no visible
mesh. These paths do not require Bedrock geometry assets or a second checkout.

The bundled entity geometry is a compatibility rest-pose extraction, not full
Java model equivalence. Java Edition's animation controllers and some
renderer-specific transforms live in compiled client code, so the table cannot
guarantee vertex-, pose-, or timing-level identity for every model. Missing
textures continue to render as explicit registry-sized diagnostic boxes rather
than silently guessing a texture.

When adding a texture alias:

1. Confirm the entity name and ID in the matching generated registry.
2. Confirm the texture path inside a legitimately obtained client jar.
3. Add a resolver test; do not add source assets to the repository.

## Resource packs

Server resource packs are validated, layered above the local vanilla jar in
server order, and rebuilt as an atomic runtime asset set. A pack must contain a
valid `pack.mcmeta`; downloads are bounded and an advertised SHA-1 is verified.
Removing a UUID-addressed layer rebuilds the stack so lower-priority overrides
become visible again. Loaded chunks are remeshed before the client reports pack
success.

## Repository policy

Client/server jars, downloaded packs, launcher object stores, extracted textures,
world saves, and authentication data are ignored and must never be committed.
See [Contributing](../CONTRIBUTING.md) and
[Security](../SECURITY.md) for the public-repository rules.
