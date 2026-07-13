# Contributing to Crabcraft

Thank you for helping build a native Rust Minecraft client. Small, well-tested
changes are welcome, especially protocol fixtures, blockstate coverage, rendering
correctness, accessibility, and documentation.

## Before you begin

- Search existing issues and open a design issue before a large architectural change.
- Do not submit Mojang jars, textures, sounds, world saves, account data, or code
  copied from decompiled Minecraft. Implement behavior from public specifications,
  clean-room observation, and redistributable generated data.
- By contributing, you agree that your change is licensed under MIT OR Apache-2.0.

## Development setup

Install stable Rust with `rustfmt` and `clippy`, then run:

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Windowed rendering requires a GPU supported by `wgpu`. Set `CRABCRAFT_JAR` to a
client jar you obtained legitimately. Optional sound and entity-model paths are
described in the README; none are required for the unit tests.

## Making a change

Keep crate boundaries intact: wire codecs belong in `crab-protocol`, transport in
`crab-net`, decoded terrain in `crab-world`, simulation in `crab-physics`, assets
and rendering in their respective crates, and orchestration/UI in `crabcraft`.

For protocol work, cite the protocol number in code comments or the PR, preserve
older profiles, bound every server-controlled allocation, and add a byte-level
fixture or mapping test. Update `docs/PROTOCOL.md` in the same commit.

For visual work, include a reproducible example or screenshot when practical and
test pure geometry/state-selection code independently of the GPU.

## Pull requests

A good pull request contains one coherent change, tests, updated documentation,
and a description of the observed bug or parity gap. CI requires formatting,
Clippy with warnings denied, and the full workspace test suite.

Security-sensitive findings should follow [SECURITY.md](SECURITY.md), not a public issue.
