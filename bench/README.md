# hooke-bench

Benchmarks comparing hooke with Taffy 0.15.1, Cephal 0.2 and (optionally) Yoga 0.5 on workloads
whose output is first checked to be equal across engines.

Public layout benchmarks time a cold layout of a tree and nothing else: there is no proof that
the engines computed the same layout, no incremental relayout, no memory and no text. hooke is
built for persistent, incremental work, so this suite measures that, and it checks equal output
before it times anything.

## Building

```sh
cargo build -p hooke-bench --release                    # hooke, Taffy, Cephal
cargo build -p hooke-bench --release --features yoga    # also Yoga (needs a C++ toolchain and libclang)
cargo test  -p hooke-bench                              # gate and adapter unit tests
```

`hooke-bench` is a workspace member; from the repository root always pass `-p hooke-bench`
(plain `cargo` commands there address the `hooke` crate alone).

## Running

```sh
cargo bench -p hooke-bench                              # every workload at 1k, 10k and 100k nodes
HOOKE_BENCH_QUICK=1 cargo bench -p hooke-bench          # 1k and 10k only, fewer samples (CI)
cargo bench -p hooke-bench -- --quick                   # the same, through criterion's flag
cargo bench -p hooke-bench -- edit_                     # criterion's filter: one family
cargo run -p hooke-bench --release --bin report         # writes notes/benchmarks/<YYYY-MM-DD>.md
```

The report runs the gate for every workload and size and the memory workload itself, and reads
criterion's results from `$CARGO_TARGET_DIR/criterion` (default `target/criterion`), taking
the mean point estimate of each benchmark; run the benches first, with the same
`CARGO_TARGET_DIR`, or the timing tables read "not run". Cargo runs bench binaries from
`bench/`, so criterion resolves a relative `CARGO_TARGET_DIR` against `bench/`; the report
looks there too, but an absolute path avoids the ambiguity. With `HOOKE_BENCH_QUICK=1` the report
also skips the 100k gate and memory runs. `notes/` is git-ignored.

The full run takes some minutes: sample sizes and measurement times shrink with tree size
(30 samples for about 1k nodes, 20 for 10k, 10 for 100k, with 1.5, 2 and 5 s of measurement). Every benchmark binary runs on a thread
with a 1 GiB stack, because the deep workloads recurse once per level in every engine.

## Methodology

**Equal-output gate** (`src/gate.rs`). Before an engine is timed on a workload, every engine
builds the scene and lays it out cold; its frames are compared with hooke's node by node, with
an absolute tolerance of 0.01 on x, y, width and height. The comparison is repeated after the
workload's text edit and after its resize, on the same engine instance, because engines can also
diverge in their incremental paths. An engine that differs is excluded from that workload's
tables (the report prints the first differing node with both frames); the other engines are
still timed. An engine whose adapter panics (for example, one not implemented yet) is reported
as "did not run".

**Slow-engine guard.** The gate times each engine's first cold layout. If it takes longer than
`HOOKE_BENCH_SLOW_SECS` seconds (default 2.0; hooke lays out 100k nodes in about 0.1 s), the
verdict is `too slow`: the edit and resize phases are skipped, the engine is excluded from every
timing family for that workload size, and criterion never samples it. The report prints
`too slow: N s per layout, not sampled` in the gate table, with the gate's one layout as the
measurement, and `not sampled (too slow)` in the timing tables. The memory workload is bounded
the same way: one layout, and a too-slow engine is reported as such. The gate's own layout cannot be
interrupted, so a full run still pays one slow layout per such cell (about a minute in all for
Yoga at 100k on an M1 Pro).

**One scene, one text model.** Each workload is a `Scene` (`src/scene.rs`): nodes in pre-order,
with a style subset that every engine supports, configured as `box-sizing: border-box`. Every
adapter (`src/engines/`) maps it onto its engine's own types and routes the engine's measure
callback through `src/measure.rs`, Taffy's Ahem test model: 10px per character, `U+200B` the only
break opportunity. All engines therefore see identical leaf sizes, and the gate compares layout
rather than text shaping. Rounding is disabled in every engine, and frames are compared
unrounded.

**Roots.** Every scene's root is `100%` of a definite viewport on both axes, so no engine's rule
for an auto-sized root affects the result, and a resize can change the root through the
viewport.

**Adapters.** hooke's adapter is a persistent host: immutable versioned props, `Arc` handles, a
shallow `Clone`. It keeps the committed revision while it builds the next one, so `set_text`
really path-copies from the root to the leaf (`Children::make_mut` and `Handle::make_mut`) and
gives the leaf a fresh content id; `layout` lays out the candidate and commits it. One `Scratch`
is reused across passes. Taffy's adapter is a `TaffyTree<String>` with the text as the leaf's
context: `set_text` replaces the context and calls `mark_dirty`.

**Workloads** (`src/workloads.rs`; seeded xorshift, deterministic):

| family | groups | what |
|---|---|---|
| cold | `cold_<template>` | arewelayoutyet's PanGUI templates (see below), 1k / 10k / 100k nodes |
| cold | `cold_random` | a wrapping row of random flex "cards" with one-word text leaves |
| deep | `deep_chain` | a chain of auto-sized containers alternating row and column, one text leaf at the bottom; 100 / 1k / 10k levels |
| deep | `deep_branch` | auto-sized containers with fan-out 2 to 3 and text leaves |
| text | `text_columns` | ten columns (10% of a 2000px root) of paragraphs of 3 to 20 words that wrap |
| edit | `edit_random`, `edit_text` | change one text leaf about halfway through pre-order and lay out again |
| resize | `resize_random`, `resize_text` | widen the viewport by 10% and lay out again |
| candidates | `candidates_text` | hooke's fork and path copy, one candidate and N = 1, 2, 4, 8 candidates on N threads, with and without a shared measure cache, against Taffy's deep clone |

The memory workload (in the report only) uses a counting global allocator (`src/alloc.rs`):
heap bytes per node after build plus one cold layout, allocations during an unchanged second
pass, and allocations during one edit pass, for every engine.

**What each iteration times.**

- Cold families: one `layout`. Between iterations, untimed, hooke and Taffy invalidate every
  node of the existing tree (Taffy's `mark_dirty` on every node, as arewelayoutyet does; hooke
  clears every node's caches and marks every child list). Engines without such an operation in
  their adapter (the `Engine` trait has none) are rebuilt between iterations, untimed.
- Edit: one `set_text` plus one `layout`, alternating between two texts for the same leaf, so
  every iteration is exactly one edit against warm caches. For hooke this includes forking the
  committed revision, the path copy, the layout and releasing the old revision.
- Resize: one `resize` plus one `layout`, alternating between the viewport and one 10% wider.
- Candidates: one fork (or clone), edit and layout; outputs are dropped outside the timing.
  `hooke-xN` includes spawning N scoped threads and a fresh `Scratch` per thread. Taffy's
  `taffy-xN` runs its N clones sequentially, because `TaffyTree` 0.15 is neither `Send` nor
  `Sync`.

**Per-node numbers.** Every benchmark sets `Throughput::Elements(node count)`, so criterion
reports time per node; the report divides the mean time per iteration by the scene's node count
(including the root and, for the arewelayoutyet templates, our one-node wrapper). For edits
and resizes the report also gives the total microseconds per operation, which is the number
that matters there: the per-node figure of an incremental pass only says how it scales.

## arewelayoutyet

[arewelayoutyet](https://github.com/marc2332/arewelayoutyet) times the PanGUI templates from
Taffy's `benches/pangui`, under the MIT license (`benchmarks/LICENSE`, copyright Visly Inc. and
the Taffy authors). The template trees and their translation to flexbox (`style` in
`benches/benches/pangui.rs`: an `Expand` main size is `flex: <weight> 1 0` with a zero minimum,
an `Expand` minimum is `flex: <weight> 0 auto`, everything else `flex: 0 0 auto`, a cross-axis
`Expand` is the default stretch, content and lines packed to the start) are ported here, with
the repeat counts scaled to reach 1k, 10k and 100k nodes. The harness lays a template root out
into its own px size, or 1000 where it has none, "as the child of a column"; here that column is
an explicit wrapper node of that size with `align-items: flex-start`.

Sizes that equal arewelayoutyet's own node counts, so per-node numbers there are directly
comparable to its public table: `fit_nesting` 100k (101,111 nodes), `wide_no_wrap_simple` 1k
and 100k (its `_few` and `_many`), `wide_wrapping` 10k and `nested_vertical_stack` 10k (10,001).
The others are rescaled from 101 (`padding_and_margin`), 3,001 (`expand_with_*`), 12,001
(`perpendicular_expand_with_wrap`), 15,001 (`flex_expand_*`) and 30,001
(`pixels_with_min_expand_constraint`) nodes. arewelayoutyet runs Taffy from a pinned Git
revision, not 0.15.1.

`percentage_and_ratio` is not ported: the scene's style subset has no `aspect-ratio`.

## Caveats

- **Engines slower than the guard are not timed.** Yoga's cold layout of some scenes takes tens
  of seconds (`perpendicular_expand_with_wrap` at 100k: about 36 s); such cells show only the
  gate's single layout, not a criterion estimate. Raise `HOOKE_BENCH_SLOW_SECS` to sample them
  anyway, at the cost of a run that may take hours.

- **No instruction counts.** iai-callgrind needs Valgrind, which does not run on macOS on Apple
  Silicon. All timings are wall-clock.
- **The counting allocator is installed in the bench binary**, so every allocation pays two
  relaxed atomic increments. That cost falls on engines in proportion to how much they allocate.
- **The text model is not text shaping.** It measures how often and under what constraints an
  engine asks for a leaf's size, not what shaping costs. The text table reports measure callbacks
  per leaf for that reason.
- **The random generator avoids known Taffy divergences** (from `tests/taffy_oracle.rs`), and its
  leaves are single words: with wrapping leaves, hooke and Taffy split the shrinking of nested
  content-sized columns differently. The deep workloads use a 65536px viewport so that no
  auto-sized row overflows, where Taffy sizes a content-sized row by its items' flex base sizes
  instead of `fit-content`.
- **Cold for hooke means caches cleared on a warm tree** (as for Taffy), not a freshly built
  tree; the scratch is warm too.
- **Gate tolerance is absolute.** On very long rows f32 accumulation alone can exceed 0.01
  (`flex_expand_equal_weights` at 100k: 100,000 items of 0.1px); the gate then excludes the
  engine for that size and the report says so.

## Baseline findings (2026-11-09, Apple M1 Pro)

The full report of the first run is `notes/benchmarks/2026-11-09-stage2-full.md` (notes are not committed). In short:

- **Equal output.** All four engines agree on 38 of 45 workload sizes. The gate found one hooke
  bug before the run (the `space-between` fallback on an overflowing reversed line, fixed in
  `59858e5` after checking Chromium), one hooke precision issue (100,000 equal `flex: 1 1 0` items
  come out 0.10002 wide instead of 0.1: f32 error accumulating in the sequential distribution), two
  Yoga semantics (flex base sizes clamped by min/max before distribution; a dropped `flex-start`
  cross margin in wrapping containers) and one Taffy flex-base-size error on wrapping text in
  unstretched items.
- **Cold layout:** hooke is 1.10x Taffy's time (geometric mean over 44 benchmarks), 0.21x on
  deep auto-sized nesting, up to 2.45x on flat wide trees; Cephal is 2x to 5x faster than hooke on
  flat trees and 35x slower on a 10k-level chain; Yoga is quadratic on wrapping templates.
- **One text edit:** hooke is 1.2x to 1.6x slower than Taffy and up to 170x slower than Cephal,
  because its edit costs about 5 ns per node of the whole tree (ancestors re-run over all
  children) plus the path copy of wide child lists. This is the first thing to optimise.
- **Resize and candidates:** hooke is 3.8x faster than Taffy on a root resize, a forked
  candidate costs a fifth of Taffy's deep clone, and eight candidates on eight threads cost 1.6x
  one.
- **Memory:** 460 to 850 bytes per node in hooke against Taffy's 960 to 1060; no engine
  allocates on a warm unchanged pass.
