//! A flexbox-only layout engine for persistent, copy-on-write UI trees.
//!
//! `hooke` lays out a tree that **you** own: a tree of immutable, reference-counted
//! nodes that a React-Native-like framework forks per update, lays out on whichever thread
//! holds the candidate, commits by compare-and-swap and diffs by pointer equality. The engine
//! never allocates a node and never owns the tree. It reads and writes your nodes through one
//! trait, [`LayoutNode`], and dictates the type of exactly two fields in them: the child list,
//! [`Children`], and the per-node layout state, [`LayoutState`].
//!
//! # The four rules
//!
//! 1. **Layout results live in the node.** A committed revision is self-contained, and a
//!    pointer-equal node in two revisions has the same size and the same descendants.
//! 2. **Write permission is reference-count uniqueness.** A pass writes a node only through
//!    [`Handle::make_mut`], which clones the node first if anything else still refers to it.
//!    Your node's shallow `Clone` is the clone callback; there is no owner pointer.
//! 3. **A child's offset belongs to its parent's list**, not to the child, so a sibling that
//!    merely moves is never cloned.
//! 4. **Dirtiness follows the borrow path.** Reaching a node mutably means calling
//!    [`Children::make_mut`] on every ancestor, and each call marks that level. A node's style
//!    is compared by a key on every visit, and a measured leaf carries a content id, so a
//!    forgotten invalidation cannot happen.
//!
//! # Embedding
//!
//! ```
//! use hooke::{Children, Handle, LayoutNode, LayoutState, Parts, PartsMut, Style};
//! use std::sync::Arc;
//!
//! struct Props { style: Style, version: u64 }
//!
//! #[derive(Clone)]                   // shallow: handle bumps plus the inline state
//! struct ShadowNode {
//!     props: Arc<Props>,
//!     children: Children<ShadowNode>,
//!     layout: LayoutState,             // 192 bytes: size, memo, four measure slots, ids
//! }
//!
//! impl LayoutNode for ShadowNode {
//!     type Scalar = f32;
//!     fn parts(&self) -> Parts<'_, Self> {
//!         Parts { style: &self.props.style, state: &self.layout, children: &self.children }
//!     }
//!     fn parts_mut(&mut self) -> PartsMut<'_, Self> {
//!         PartsMut { style: &self.props.style, state: &mut self.layout, children: &mut self.children }
//!     }
//!     fn style_key(&self) -> u64 { self.props.version }   // O(1): props are immutable and versioned
//! }
//! # let _ = |n: Handle<ShadowNode>| n;
//! ```
//!
//! See [`layout`] for running a pass, [`Measure`] for sizing leaves such as text, and
//! [`snap()`] for pixel-snapping results at mount time.
//!
//! # Status
//!
//! Phase 0. The storage layer, the host contract, the invalidation rules, the caches and the
//! concurrency contract are the deliverable. The container algorithm is **provisional**: a
//! single-line flexbox without wrapping, baselines, the automatic minimum size or aspect
//! ratio. It exists so the storage layer can be exercised and compared against Taffy: the
//! differential test in `tests/taffy_oracle.rs` lays out random trees with both engines and
//! finds every frame equal within 0.01. The spec-complete algorithm is the next phase.
//!
//! # Features
//!
//! | Feature | Default | Effect |
//! |---|---|---|
//! | `std` | on | `std::error::Error` impls and [`SharedMeasureCache`] |
//! | `sync` | on | [`Handle`] is `Arc` and nodes are `Send + Sync`; off means `Rc` |
//! | `slots-2` | off | Two persistent measurement slots per node instead of four |
//! | `f64` | off | `Scalar` for `f64` |

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

extern crate alloc;

pub mod children;
pub mod geometry;
pub mod measure;
pub mod pass;
pub mod scalar;
pub mod snap;
pub mod state;
pub mod style;
pub mod tree;

pub use children::{Children, Slot};
pub use geometry::{Axis, Edges, Point, Rect, Size};
pub use measure::{Available, Measure, MeasureCache, MeasureInput, Measured, NoCache};
#[cfg(feature = "std")]
pub use measure::SharedMeasureCache;
pub use pass::{layout, Cancelled, LayoutCx, RootConstraints, Scratch, Stats};
pub use scalar::{Opt, Scalar};
pub use snap::snap;
pub use state::{ConstraintKey, LayoutState, NodeKind, DEFAULT_SLOTS};
pub use style::{
    AlignContent, AlignItems, BoxSizing, Direction, Display, FlexDirection, FlexWrap, JustifyContent, Length,
    Overflow, Position, Style,
};
pub use tree::{LayoutNode, Parts, PartsMut};

/// The shared handle every node lives behind: `Arc` with the `sync` feature, `Rc` without.
///
/// The engine never names `Arc` or `Rc` directly. Hosts use `Handle::new`, `Handle::make_mut`
/// and `Handle::ptr_eq` exactly as they would on the concrete type.
#[cfg(feature = "sync")]
pub type Handle<T> = alloc::sync::Arc<T>;

/// The shared handle every node lives behind: `Arc` with the `sync` feature, `Rc` without.
#[cfg(not(feature = "sync"))]
pub type Handle<T> = alloc::rc::Rc<T>;
