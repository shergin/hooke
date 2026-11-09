# Hooke

A flexbox-only layout engine for persistent, copy-on-write UI trees. The `hooke` crate is at the
repository root (a single crate, not a workspace). Design documents and research notes are in
`notes/` (`notes/reports/`, `notes/research_notes/`), which is ignored by git.

## Who writes what

- **Fable (the main session) owns decisions and production code.** Architecture and API
  decisions, and every file under `src/`, are written and reviewed by the main session
  directly. Do not delegate library code.
- **Opus subagents own non-production code.** Delegate to a subagent with `model: "opus"`:
  tests and test support, fixtures and fixture generators, benchmarks, examples, README and
  other docs, CI and scripts, build and lint checks, and the debugging of failing tests.
- When a failing test points at a library bug, the subagent diagnoses it and reports the root
  cause with a proposed minimal change; the change to `src/` is made in the main session.
- Every subagent brief names the files it may edit. A subagent may prototype a `src/` change to
  confirm a hypothesis only if it restores the file before finishing and hands back a diff.

## Checks

- `cargo +stable test` must pass.
- `cargo +stable build --no-default-features --features sync` and
  `cargo +stable build --no-default-features` (the `Rc` build) must compile: the crate is
  `no_std` with `alloc`.
- `tests/taffy_oracle.rs` is the correctness arbiter for the provisional flexbox algorithm:
  random trees are laid out by both engines and every frame is compared.
