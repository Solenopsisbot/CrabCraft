# Changelog

This project is pre-release. Notable changes are recorded here; the Git history
remains the detailed source of truth.

## Unreleased

### Added

- A renderer-neutral `crab-core` with immutable session profiles, bounded typed
  commands, deterministic events/snapshots, a modal screen stack, and redacted
  semantic replay recording.
- Revisioned copy-on-write world snapshots, event-driven bounded meshing, stale
  result rejection, and transactional asynchronous resource generations.
- An extension guide covering protocol, command, state, UI, and worker seams.
- Protocol profiles for Java Edition 1.20.2 and 1.20.3/1.20.4, including the
  Configuration state, registry data, chunk batches, NBT components, and
  versioned resource-pack packets. The 1.20.4 core path is live-tested against
  an official vanilla server.
- Protocol 766 for Java Edition 1.20.5/1.20.6, including split configuration
  registries, revised play/configuration packet maps, component-era item stacks,
  recipes and container clicks. The core path and component-rich inventory
  updates are live-tested against an official vanilla 1.20.6 server.
- Protocol 767 for Java Edition 1.21/1.21.1, with Tricky Trials registries,
  VarInt-count item stacks, revised attribute/jukebox components, configuration
  additions, and official-server core/component live validation.
- Protocol 768 for Java Edition 1.21.2/1.21.3, including generated registries,
  bundle-era packet maps, teleport velocity/movement flags, particle settings,
  split inventory updates, all 67 item components, and official 1.21.3 live
  core/component validation.
- Version-selected generated block/state, item, and entity registries for the
  763 through 770 wire profiles.
- Protocol 770 for Java Edition 1.21.5, including Spring to Life registries,
  shifted game-test/play packet maps, typed chunk heightmap arrays, the chat
  checksum trailer, all 96 reorganized item components, and official-server
  core plus component-rich inventory validation.
- Protocol 771 for Java Edition 1.21.6, including generated registries, the
  fixed-length paletted chunk-container decoder, the shifted Change Game Mode
  packet, and an official-server core-path smoke validation.
- Protocol-aware entity metadata components and scoreboard teams, including
  team prefixes/suffixes in sidebar and Tab-list names.
- UUID-addressed server resource-pack stacks that can remove and rebuild any
  active layer while retaining vanilla fallback assets.
- Live validated resource-pack layering and renderer atlas replacement.
- Signs, editable/readable books, maps, recipe books, biome tinting, transparent
  fluids, sky rendering, particles, expanded menus, vehicles, swimming/Elytra,
  scoreboards, tab list, entity poses/equipment, and dropped block-item models.
- Public contribution, security, architecture, protocol, and agent documentation.
- Registry-driven vanilla blockstate loading for variants and multipart models,
  including conditional parts, weighted alternatives, rotations, and UV locking.
- Inherited 3D item-model ground transforms, exact-state falling-block models,
  and bone-following animated humanoid armour layers.
- Deduplicated, version-selected vanilla collision boxes for every global block
  state across all supported registry profiles.
- Registry-complete entity geometry/texture resolution, built-in textured
  boats and rafts, item/block-shaped entity rendering, and ambient wing, fin,
  tail, and paddle animation.
- Vanilla-derived mining-tool tags from the selected client jar, including
  material speeds and status-effect modifiers for predicted break timing.
- Protocol-backed special-entity rendering for item frames, paintings, area
  clouds, lightning, and block/item/text Displays, including jar-sourced
  painting textures and alpha-blended effect/text streams.
- Bundled vanilla rest-pose entity geometry generated from decompiled Java model
  builders; client jars now provide textures without requiring Bedrock geometry
  assets or a separate geometry checkout.

### Changed

- Expanded collision shapes, blockstate rendering, entity animation, audio,
  movement, HUD, inventory, and workstation behavior toward vanilla parity.
- Vanilla blockstate rotations now use the model JSON axis convention, so
  multipart plants and directional models face the requested wall/top/side;
  waterlogged geometry gets its translucent fluid layer and fluid faces no
  longer incorrectly occlude solid blocks.
- Replaced block-family state guesses in the primary rendering path with
  generated property schemas for every supported registry profile.
- Creative flight uses vanilla-style double-Space toggling; `F` swaps hands.
- Swimming now requires the player's head/eyes to be in water. Creative and
  Spectator HUDs hide survival hearts, hunger, and oxygen bubbles.
- Section Blocks Update packets are decoded for every supported protocol profile,
  so mob, fluid, piston, and other multi-block server changes reach the world
  model and invalidate the right chunk meshes.
- The inventory screen includes a cursor-facing 3D local-player preview with an
  isolated camera/depth viewport and correct HUD layering.
- The pause menu now includes an options screen with live FOV, mouse
  sensitivity, and fullscreen controls.
- F5 now cycles first-person, rear third-person, and front third-person cameras;
  third-person views render the local animated/posed player model with camera
  yaw and pitch, and shorten the camera arm around walls to prevent clipping.
- Entity asset resolution now covers Java family texture directories, shared
  projectile and minecart geometry, and Bedrock's plain `.json` model files,
  substantially reducing generic-box fallbacks with the documented asset setup.
  third-person views render the local animated/posed player model and shorten
  the camera arm around walls to prevent clipping.
- Entity asset resolution now covers Java family texture directories and shared
  projectile/minecart texture variants, substantially reducing generic-box
  fallbacks with the documented asset setup.
- Entity loading now reads textures and optional custom geometry from the client
  jar directly; the launcher no longer downloads a separate geometry checkout,
  and vanilla models without resource geometry use deterministic registry-sized
  generated meshes.
- Falling-block entities retain their Spawn Entity block-state ID and render as
  full-scale textured block models instead of anonymous bounds boxes.
- Entity Pose metadata now uses the correct serializer on component-era
  protocols and drives crouching, swimming/fall-flying, sleeping, dying,
  long-jump, and sitting model transforms.
- Protocol 768 recipe-book add/remove displays now populate the existing
  crafting/stonecutter UI and send numeric display-ID placement requests.
- Protocol 768 bundle contents are retained across inventory synchronization;
  scrolling over a bundle selects nested stacks, updates its tooltip, and sends
  the vanilla bundle-selection packet.
- Protocol 769 for Java Edition 1.21.4, with generated Pale Garden registries,
  Player Loaded, split pick-item mapping, direct component slots, revised
  held-item/particle/player-list/vehicle payloads, and official-server core plus
  component live validation.
- Protocol 768/769 held-item use now includes the required camera rotation, and
  modern component-era particle packets decode their particle ID after the fixed
  header instead of using the legacy prefix order.
- Jump collision now follows vanilla rising-before-gravity ordering; swimming
  projects movement through camera pitch, climbables require explicit vertical
  input, and local underwater air supply is tracked for the HUD.
- Underwater presentation now uses camera-point detection, smoothed pose
  transitions, and distance fog. Atlas UVs address outer texel centers to avoid
  subpixel texture bleed, and directional block placement plus attached 3D
  held-item geometry follow the active registry.
