# hooke

A new FlexBox layout library in Rust, focused on concurrent rendering.

`hooke` is a flexbox-only layout engine for persistent, copy-on-write UI trees.

`hooke` lays out a tree that **you** own: a tree of immutable, reference-counted nodes that
a React-Native-like framework forks per update, lays out on whichever thread holds the
candidate, commits by compare-and-swap and diffs by pointer equality. The engine never
allocates a node and never owns the tree. It reads and writes your nodes through one trait,
`LayoutNode`, and dictates the type of exactly two fields in them: the child list,
`Children`, and the per-node layout state, `LayoutState`.

The crate is `no_std` with `alloc`, has no dependencies and forbids `unsafe`.

## The four rules

1. **Layout results live in the node.** A committed revision is self-contained, and a
   pointer-equal node in two revisions has the same size and the same descendants.
2. **Write permission is reference-count uniqueness.** A pass writes a node only through
   `Handle::make_mut`, which clones the node first if anything else still refers to it.
   Your node's shallow `Clone` is the clone callback; there is no owner pointer.
3. **A child's offset belongs to its parent's list**, not to the child, so a sibling that
   merely moves is never cloned.
4. **Dirtiness follows the borrow path.** Reaching a node mutably means calling
   `Children::make_mut` on every ancestor, and each call marks that level. A node's style
   is compared by a key on every visit, and a measured leaf carries a content id, so a
   forgotten invalidation cannot happen.

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

**Phase 1.** The storage layer and the host contract are complete: copy-on-write child lists,
per-node layout state, the invalidation rules, the in-node and shared measurement caches and
the concurrency contract.

The container algorithm is the complete CSS Flexible Box Layout Level 1 algorithm: `order`,
`display: none` and `display: contents`, both `box-sizing` values, `direction: rtl` and the
`*-reverse` directions, wrapping with `wrap-reverse` and `align-content`, auto margins, the
§4.5 automatic minimum size, `aspect-ratio` with min/max transfer, first-baseline alignment
and container baselines, the `min-content`/`max-content`/`fit-content`/`stretch` keywords,
scroll containers with a scrollbar gutter, relative insets, and absolutely positioned children
against the §4.1 static-position rectangle of their parent. Chrome is the arbiter:
`tests/chrome_fixtures.rs` lays out 2500 Chrome-recorded fixtures (Taffy v0.15.1's generated
corpus plus cephal's divergence cases) and every frame agrees within 0.01; the list of
expected failures in `tests/fixtures/expected_failures.txt` is empty. `tests/taffy_oracle.rs`
is a second oracle on random trees, narrowed to the cases where Taffy agrees with Chrome.

Out of scope for now: vertical writing modes, `visibility: collapse`, last-baseline alignment,
`calc()`, `position: sticky` and `fixed`, fragmentation, `flex-wrap: balance`, and absolutely
positioned children whose containing block is not their parent.

## Features

| Feature | Default | Effect |
|---|---|---|
| `std` | on | `std::error::Error` impls and `SharedMeasureCache` |
| `sync` | on | `Handle` is `Arc` and nodes are `Send + Sync`; off means `Rc` |
| `slots-2` | off | Two persistent measurement slots per node instead of four |
| `f64` | off | `Scalar` for `f64` |

Without `sync` the crate builds for single-core targets without atomic compare-and-swap.

## Running

```sh
cargo test                                            # unit, Chrome fixture, persistence and oracle tests
cargo test --test chrome_fixtures -- --nocapture      # the 2500 Chrome fixtures and their pass rate
cargo test --test persistence                         # the copy-on-write and concurrency contract
cargo run --example shadow_tree                       # the host embedding, step by step
cargo build --no-default-features --features sync     # no_std, Arc handles
cargo build --no-default-features                     # no_std, Rc handles
```

## License

MIT OR Apache-2.0.
