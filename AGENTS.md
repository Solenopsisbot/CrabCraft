# Repository instructions

These instructions apply to the entire Crabcraft workspace.

## Project principles

- Preserve vanilla wire behavior and observable client behavior; do not silently
  guess packet IDs, registry IDs, or field layouts.
- Treat Mojang assets as user-provided input. Never commit client/server jars,
  launcher assets, world saves, access tokens, or generated resource-pack archives.
- Keep protocol-version differences explicit. Shared codecs are appropriate only
  where the wire representation is genuinely identical.
- Keep game state independent from rendering. Network code updates `Shared`; the
  window and renderer consume snapshots and outboxes.
- Preserve unrelated working-tree changes.

## Required workflow

1. Inspect the affected crate and its tests before editing.
2. Prefer authoritative generated protocol/registry data over memory.
3. Add regression tests for codecs, ID mappings, parsers, physics, or geometry.
4. Update `README.md` and the relevant file under `docs/` whenever behavior,
   controls, configuration, or compatibility changes.
5. Run:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace
   git diff --check
   ```

## Code style

- Use stable Rust and workspace dependencies where possible.
- Keep packet parsing bounded; reject hostile counts and lengths before allocating.
- Avoid panics on network input. Propagate parse errors or log and ignore optional
  presentation packets when safe.
- Document public APIs and non-obvious wire-format/version decisions.
- Use `tracing` rather than stdout debugging.

## Commits and pull requests

- Make focused commits with imperative summaries.
- Never include copyrighted game assets or credentials.
- Explain user-visible impact, protocol evidence, and verification in the PR body.
- Keep the compatibility table in `docs/PROTOCOL.md` honest; “supported” means the
  login/configuration/play path is implemented and exercised, not merely that the
  handshake accepts the protocol number.
