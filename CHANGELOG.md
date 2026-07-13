# Changelog

This project is pre-release. Notable changes are recorded here; the Git history
remains the detailed source of truth.

## Unreleased

### Added

- Protocol profiles for Java Edition 1.20.2 and 1.20.3/1.20.4, including the
  Configuration state, registry data, chunk batches, NBT components, and
  versioned resource-pack packets. The 1.20.4 core path is live-tested against
  an official vanilla server.
- Version-selected generated block/state, item, and entity registries for the
  763, 764, and 765 wire profiles.
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
