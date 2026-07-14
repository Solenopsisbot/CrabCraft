# Extending Crabcraft

Crabcraft's extension points separate protocol evidence, deterministic game
state, background work, and presentation. Start at the boundary that owns the
new behavior instead of adding another shared outbox or process-global switch.

## Add a protocol version

1. Obtain authoritative packet schemas, packet-ID reports, and generated
   registries. Never infer an insertion point from a neighbouring release.
2. Add changed wire layouts under `crab-protocol/src/versions/`. Reuse a codec
   only when its complete byte representation is identical.
3. Add a `ProtocolVersion` profile and aliases in `crab-core/src/protocol.rs`.
   Bind it to an immutable `RegistrySet`.
4. Add verified unchanged-ID translations in `crab-core/src/wire.rs`. Return an
   unsupported mapping for inserted, removed, or changed packets and handle the
   changed body explicitly in the `crabcraft` adapter.
5. Exercise login, configuration, play, representative registry IDs, every
   mapping insertion boundary, and changed packet bodies. Update
   [Protocol support](PROTOCOL.md) only after the core path is live-tested.

This keeps packet bytes out of game state and makes a version diff reviewable in
one profile module.

## Add an input or UI action

1. Add a renderer-neutral variant to `ClientCommand` in
   `crab-core/src/command.rs`.
2. Enqueue it with `Shared::queue_command`; do not create a new mutex-protected
   outbox.
3. Handle the command in the configuration or play adapter and encode it with
   the selected protocol's explicit codec.
4. Test queue capacity, ordering, phase deferral, and the relevant wire body.

Commands should express intent (`CloseContainer`, `SendChat`), not a packet ID.
Keep strings and collections bounded before they reach an encoder.

## Add authoritative state

1. Add a semantic `ClientEvent` and any coherent snapshot field in
   `crab-core/src/state.rs`.
2. Implement its deterministic transition in `ClientCore::apply` without I/O,
   rendering, clocks, or global registry reads.
3. Decode the packet in `crabcraft`, construct the event, and publish the new
   `Arc<ClientSnapshot>` revision.
4. Test a sequence of events and its resulting snapshot. If the state matters
   across versions, replay the same semantic sequence under multiple
   `SessionContext`s.

Feature-specific legacy stores may be mirrored during migration, but new
consumers should prefer the coherent snapshot.

## Add a screen

Add a `UiScreen` variant in `crab-core/src/screen.rs` and drive it through
`ScreenStack`. Define whether it is pushed, replaced, or removed and test escape
and parent-screen behavior. Keep window preferences and drawing state local to
presentation; send gameplay intent through `ClientCommand`.

## Add background work

Use the same contract as meshing and resource preparation:

1. Capture an immutable, minimal input snapshot while briefly holding locks.
2. Submit through a bounded queue so producers cannot grow memory without
   limit.
3. Stamp work with every dependency revision or generation.
4. Perform CPU work off the window/network owner thread.
5. Reject stale results before committing side effects.

If a group of resources must agree, prepare a complete generation and commit it
atomically. Never partially replace an atlas, registry, or model set after an
error.

## Regression checklist

- Keep every network-controlled count and length bounded before allocation.
- Add byte-level tests for codecs and insertion-boundary tests for ID maps.
- Add deterministic state/replay tests for semantic behavior.
- Add stale-result tests for revisioned worker output.
- Keep `README.md`, `docs/ARCHITECTURE.md`, and `docs/PROTOCOL.md` aligned with
  observable behavior and compatibility.
- Run the repository's full formatting, Clippy, test, and diff checks from
  `AGENTS.md`.
