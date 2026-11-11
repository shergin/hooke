# hooke

[![crates.io](https://img.shields.io/crates/v/hooke.svg)](https://crates.io/crates/hooke)
[![docs.rs](https://docs.rs/hooke/badge.svg)](https://docs.rs/hooke)

**Flexbox layout for concurrent rendering.**

`hooke` lays out UI trees made of immutable, shared nodes. Fork a tree by cloning one handle.
Lay out competing versions of it on different threads. Commit one, drop the rest, and diff
revisions by pointer equality.

Flexbox only. Pure Rust, `no_std` with `alloc`, no dependencies, no `unsafe`.

```sh
cargo add hooke
```

## Why it exists

A concurrent renderer never mutates the tree that is on screen. React Native's Fabric is the
model: every update forks the current tree into a candidate, lays the candidate out off the
main thread, and commits it only if nothing else got there first. Several candidates can be in
flight at once, at different priorities, and most of them share most of their nodes.

That asks something unusual of a layout engine: it has to work on a tree whose nodes belong to
several versions at the same time. Engines are not built that way.

- **Taffy** keeps sizes, positions and caches inside the tree it lays out and overwrites them
  on every pass. A second version of a tree is a deep copy, and its tree and styles cannot
  cross threads.
- **Yoga** does support this model, and Fabric runs on it. It is C++, and the sharing rests on
  a convention: a raw owner pointer that every code path has to check, plus a clone callback.

`hooke` makes copy-on-write the storage model itself and lets the borrow checker enforce it.
It was written for a Rust UI framework with a Fabric-style renderer, and it does nothing else.

## What is different

- **You own the tree.** The engine never allocates a node. It reads and writes yours through
  one trait, `LayoutNode`, and asks for two fields: a child list and 192 bytes of layout state.
- **A fork is free; a layout pays only for what changed.** A pass clones a node only when it
  has to write it and someone else still holds it. Everything else stays shared.
- **Every revision is complete.** Results live in the nodes, so a committed tree needs no side
  table, and a subtree that did not change is the same pointer in both revisions.
- **Moving is not changing.** A child's offset is stored in its parent's list, so a sibling
  that only shifts is never cloned.
- **Nothing to invalidate by hand.** There is no `mark_dirty`. Reaching a node mutably marks
  the path to it, styles are compared by key, and measured leaves carry a content id.
- **Threads are ordinary.** Nodes are `Send + Sync` whenever your payload is. Each candidate
  gets its own pass, and passes can share one measurement cache, so text measured for one
  candidate is not measured again for the next.
- **Chrome is the arbiter.** 2500 layouts recorded in Chrome, every frame within 0.01.

| | hooke | Taffy | Yoga |
|---|---|---|---|
| Tree | the host's | the engine's, or the host's through traits | the engine's nodes |
| Results | in your nodes, immutable once committed | in the tree, overwritten | in the node |
| A second version | shares nodes, clones what a pass writes | deep copy | shares nodes, clones through owner checks and a callback |
| Invalidation | automatic | dirty marking up the parent chain | dirty bit through owner pointers |
| Layout modes | flexbox | flexbox, grid, block | flexbox |
| Language | Rust, no `unsafe` | Rust | C++ |

A fork costs a handle clone instead of a tree copy, and a relayout after an edit reuses the
answer of every subtree the edit did not reach. [`bench/`](bench/README.md) measures both
against Taffy, Yoga and Cephal, and checks that the engines produce the same layout before it
times them.

## When not to use it

If you need grid, block or inline layout, use [Taffy](https://crates.io/crates/taffy). If your
UI tree is mutable and retained, an engine that owns the tree is simpler to live with, and
Taffy is again the right choice. `hooke` earns its constraints only when versions of a tree
have to coexist.

## Embedding

```rust
use hooke::{Children, Handle, LayoutNode, LayoutState, Parts, PartsMut, Style};
use std::sync::Arc;

struct Props { style: Style, version: u64 }

#[derive(Clone)]                   // shallow: handle bumps plus the inline state
struct ShadowNode {
    props: Arc<Props>,
    children: Children<ShadowNode>,
    layout: LayoutState,             // 192 bytes: size, memo, four measure slots, ids
}

impl LayoutNode for ShadowNode {
    type Scalar = f32;
    fn parts(&self) -> Parts<'_, Self> {
        Parts { style: &self.props.style, state: &self.layout, children: &self.children }
    }
    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { style: &self.props.style, state: &mut self.layout, children: &mut self.children }
    }
    fn style_key(&self) -> u64 { self.props.version }   // O(1): props are immutable and versioned
}
```

Then call `hooke::layout(&mut root, RootConstraints::viewport(w, h), &mut cx, &mut measurer)`
with a `LayoutCx` over a reusable `Scratch`, and implement `Measure` for leaves such as text.
Sizes end up in each node's `LayoutState`, offsets in each parent's `Children`; `snap` turns
them into device-pixel frames at mount time.

[`examples/shadow_tree.rs`](examples/shadow_tree.rs) is a complete Fabric-like host: a
`ShadowNode` with a family, versioned props and a text leaf; path-copy edits; a `ShadowTree`
with a compare-and-swap commit; two candidates laid out on two threads sharing a
`SharedMeasureCache`, the loser rebased with zero re-measurement; a mount-time diff that skips
pointer-equal subtrees; and cancellation.

## Status

Early: this is 0.1, and the API may change before 1.0. The minimum supported Rust version is 1.88.

Implemented: the complete CSS Flexible Box Layout Level 1 algorithm. That includes `order`,
`display: none` and `display: contents`, both `box-sizing` values, `direction: rtl` and the
`*-reverse` directions, wrapping with `wrap-reverse` and `align-content`, auto margins, the
§4.5 automatic minimum size, `aspect-ratio` with min/max transfer, first-baseline alignment,
the `min-content`/`max-content`/`fit-content`/`stretch` keywords, scroll containers with a
scrollbar gutter, relative insets, and absolutely positioned children of their parent.

Verified: `tests/chrome_fixtures.rs` lays out 2500 Chrome-recorded fixtures (Taffy's generated
corpus plus Cephal's divergence cases) with no expected failures. `tests/taffy_oracle.rs`
compares random trees against Taffy where Taffy agrees with Chrome. `tests/persistence.rs`
holds the copy-on-write and concurrency contract.

Not implemented: vertical writing modes, `visibility: collapse`, last-baseline alignment,
`calc()`, `position: sticky` and `fixed`, fragmentation, `flex-wrap: balance`, and absolutely
positioned children whose containing block is not their parent.

## Features

| Feature | Default | Effect |
|---|---|---|
| `std` | on | `std::error::Error` impls and `SharedMeasureCache` |
| `sync` | on | `Handle` is `Arc` and nodes are `Send + Sync`; off means `Rc` |
| `slots-2` | off | Two persistent measurement slots per node instead of four (see below) |
| `f64` | off | `Scalar` for `f64` |

Without `sync` the crate builds for single-core targets without atomic compare-and-swap.
`slots-2` changes the size of `LayoutState` for every crate in the build, so only the final
application should enable it, never a library.

## Running

The tests and their 2500 fixtures are not part of the published package; run them from a git
checkout.

```sh
cargo test                                            # unit, Chrome fixture, persistence and oracle tests
cargo test --test chrome_fixtures -- --nocapture      # the 2500 Chrome fixtures and their pass rate
cargo test --test persistence                         # the copy-on-write and concurrency contract
cargo run --example shadow_tree                       # the host embedding, step by step
cargo build --no-default-features --features sync     # no_std, Arc handles
cargo build --no-default-features                     # no_std, Rc handles
```

## License

Licensed under either of the [MIT license](LICENSE-MIT) or the
[Apache License, Version 2.0](LICENSE-APACHE), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
