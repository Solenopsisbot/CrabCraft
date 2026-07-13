# Changelog

This project is pre-release. Notable changes are recorded here; the Git history
remains the detailed source of truth.

## Unreleased

### Added

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
  763 through 768 wire profiles.
- Protocol-aware entity metadata components and scoreboard teams, including
  team prefixes/suffixes in sidebar and Tab-list names.
- UUID-addressed server resource-pack stacks that can remove and rebuild any
  active layer while retaining vanilla fallback assets.
- Live validated resource-pack layering and renderer atlas replacement.
- Signs, editable/readable books, maps, recipe books, biome tinting, transparent
  fluids, sky rendering, particles, expanded menus, vehicles, swimming/Elytra,
  scoreboards, tab list, entity poses/equipment, and dropped block-item models.
- Public contribution, security, architecture, protocol, and agent documentation.

### Changed

- Expanded collision shapes, blockstate rendering, entity animation, audio,
  movement, HUD, inventory, and workstation behavior toward vanilla parity.
- Creative flight uses vanilla-style double-Space toggling; `F` swaps hands.
- The inventory screen includes a cursor-facing 3D local-player preview with an
  isolated camera/depth viewport and correct HUD layering.
- Entity asset resolution now covers Java family texture directories, shared
  projectile and minecart geometry, and Bedrock's plain `.json` model files,
  substantially reducing generic-box fallbacks with the documented asset setup.
- Falling-block entities retain their Spawn Entity block-state ID and render as
  full-scale textured block models instead of anonymous bounds boxes.
- Entity Pose metadata now uses the correct serializer on component-era
  protocols and drives crouching, swimming/fall-flying, sleeping, dying,
  long-jump, and sitting model transforms.
- Protocol 768 recipe-book add/remove displays now populate the existing
  crafting/stonecutter UI and send numeric display-ID placement requests.
