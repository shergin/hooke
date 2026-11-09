//! Running a layout pass: the context, the scratch, the entry point and the per-node protocol.
//!
//! The protocol at every node, in order:
//!
//! 1. **Probe through `&`.** If the style key matches, the child list is not touched and a
//!    cached answer exists for these constraints, return it. Neither the parent's list nor the
//!    node is made unique.
//! 2. **Make unique.** `Handle::make_mut` on the parent's list (once per container per pass),
//!    then on the node. This is the only place a node is cloned, and the host's `Clone` is the
//!    clone callback.
//! 3. **Invalidate.** A changed style key or a touched list clears the node's cached answers.
//! 4. **Compute.** A measured leaf consults its slots, the per-pass table, the shared cache and
//!    finally the measurer. A container runs the algorithm, probing each child before
//!    descending, and writes each child's offset into its own list.
//! 5. **Record.** Memo or slot, size, `has_new_layout`, and clear `touched`.

mod flexbox;
mod table;

use crate::children::Slot;
use crate::geometry::{Axis, Point, Size};
use crate::measure::{Available, Measure, MeasureCache, MeasureInput, Measured};
use crate::scalar::{Opt, Scalar};
use crate::state::{ConstraintKey, NodeKind};
use crate::style::{slot, BoxSizing, Direction, Display, Length};
use crate::tree::LayoutNode;
use crate::Handle;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, Ordering};

/// Per-pass temporaries, reused across passes. After warm-up a pass allocates nothing beyond
/// what copy-on-write demands.
pub struct Scratch<S: Scalar> {
    items: Pool<flexbox::Item<S>>,
    lines: Pool<flexbox::Line<S>>,
    indices: Pool<u32>,
    /// Leaf measurements of this pass, by node address and key.
    leaf_table: table::PassTable<Measured<S>>,
    /// Container size queries of this pass, by node address and key, so a question a node's
    /// slots have evicted is still answered once per pass.
    size_table: table::PassTable<(Size<S>, Opt<S>)>,
}

/// A pool of vectors handed out to nested containers. Every vector in the pool is kept at the
/// largest capacity any of them has needed, because which vector serves which container depends
/// on the order of the recursion: without that, an edit that changes the order can hand a small
/// vector to a large container and allocate.
struct Pool<T> {
    free: Vec<Vec<T>>,
    high: usize,
}

impl<T> Default for Pool<T> {
    fn default() -> Self {
        Pool { free: Vec::new(), high: 0 }
    }
}

impl<T> Pool<T> {
    fn take(&mut self) -> Vec<T> {
        self.free.pop().unwrap_or_default()
    }
    fn give(&mut self, mut v: Vec<T>) {
        v.clear();
        if v.capacity() > self.high {
            self.high = v.capacity();
            for w in self.free.iter_mut() {
                w.reserve_exact(self.high);
            }
        } else {
            v.reserve_exact(self.high);
        }
        self.free.push(v);
    }
}

impl<S: Scalar> Default for Scratch<S> {
    fn default() -> Self {
        Scratch {
            items: Pool::default(),
            lines: Pool::default(),
            indices: Pool::default(),
            leaf_table: table::PassTable::default(),
            size_table: table::PassTable::default(),
        }
    }
}

impl<S: Scalar> Scratch<S> {
    /// An empty scratch.
    pub fn new() -> Self {
        Self::default()
    }
    fn take_items(&mut self) -> Vec<flexbox::Item<S>> {
        self.items.take()
    }
    fn give_items(&mut self, v: Vec<flexbox::Item<S>>) {
        self.items.give(v)
    }
    fn take_lines(&mut self) -> Vec<flexbox::Line<S>> {
        self.lines.take()
    }
    fn give_lines(&mut self, v: Vec<flexbox::Line<S>>) {
        self.lines.give(v)
    }
    fn take_indices(&mut self) -> Vec<u32> {
        self.indices.take()
    }
    fn give_indices(&mut self, v: Vec<u32>) {
        self.indices.give(v)
    }
}

impl<S: Scalar> fmt::Debug for Scratch<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scratch").field("pooled_item_vecs", &self.items.free.len()).finish()
    }
}

/// Counters a pass reports. All are per top-level [`layout`] call.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Nodes considered, including those answered by a probe.
    pub visited: u32,
    /// Answers served from a node's final-layout memo or measurement slots without descending.
    pub memo_hits: u32,
    /// Leaf answers served from a node's persistent measurement slots inside the leaf path.
    pub slot_hits: u32,
    /// Answers served from the per-pass tables.
    pub pass_table_hits: u32,
    /// Leaf answers served from the shared cache.
    pub shared_hits: u32,
    /// Calls into the measurer.
    pub measured: u32,
    /// Nodes cloned because they were shared when the pass needed to write them.
    pub nodes_cloned: u32,
    /// Child lists cloned for the same reason.
    pub lists_cloned: u32,
}

/// Everything a pass needs besides the tree: scratch, caches, cancellation and counters.
pub struct LayoutCx<'a, S: Scalar> {
    scratch: &'a mut Scratch<S>,
    cache: Option<&'a dyn MeasureCache<S>>,
    epoch: u32,
    cancel: Option<&'a AtomicBool>,
    depth: u32,
    /// Counters for the current top-level pass.
    pub stats: Stats,
}

impl<'a, S: Scalar> LayoutCx<'a, S> {
    /// A context over a reusable scratch, with no shared cache and no cancellation.
    pub fn new(scratch: &'a mut Scratch<S>) -> Self {
        LayoutCx { scratch, cache: None, epoch: 0, cancel: None, depth: 0, stats: Stats::default() }
    }
    /// Uses `cache` for measured leaves with a non-zero content id.
    #[must_use]
    pub fn with_cache(mut self, cache: &'a dyn MeasureCache<S>) -> Self {
        self.cache = Some(cache);
        self
    }
    /// The epoch under which shared-cache entries are read and written.
    #[must_use]
    pub fn with_epoch(mut self, epoch: u32) -> Self {
        self.epoch = epoch;
        self
    }
    /// Polls `flag` once per container; a set flag makes [`layout`] return [`Cancelled`].
    #[must_use]
    pub fn with_cancel(mut self, flag: &'a AtomicBool) -> Self {
        self.cancel = Some(flag);
        self
    }
}

impl<S: Scalar> fmt::Debug for LayoutCx<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LayoutCx").field("epoch", &self.epoch).field("stats", &self.stats).finish()
    }
}

/// How the root is constrained.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct RootConstraints<S: Scalar> {
    /// Space available to the root on each axis. An auto-sized root is `fit-content` in a
    /// definite width and content-sized in height; its intrinsic size under the keywords.
    pub available: Size<Available<S>>,
    /// Sizes imposed on the root, overriding its style.
    pub known: Size<Opt<S>>,
    /// Inline direction for nodes whose own `direction` is `inherit`.
    pub direction: Direction,
}

impl<S: Scalar> RootConstraints<S> {
    /// A definite viewport, no imposed size, left-to-right.
    pub fn viewport(width: S, height: S) -> Self {
        RootConstraints {
            available: Size::new(Available::Definite(width), Available::Definite(height)),
            known: Size::new(Opt::NONE, Opt::NONE),
            direction: Direction::Ltr,
        }
    }
    /// Returns the constraints with `direction` set.
    #[must_use]
    pub fn with_direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }
    /// Returns the constraints with imposed sizes.
    #[must_use]
    pub fn with_known(mut self, width: Opt<S>, height: Opt<S>) -> Self {
        self.known = Size::new(width, height);
        self
    }
}

/// The pass was cancelled through [`LayoutCx::with_cancel`]. Whatever was written so far is
/// correct for its constraints; the candidate can be dropped or laid out again.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("layout pass cancelled")
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Cancelled {}

/// Lays out the tree under `root` and returns the root's border-box size.
///
/// The root is a handle so the same uniqueness rule applies to it: a candidate forked from a
/// committed revision shares its root until the pass needs to write it. Results are in the
/// tree afterwards: each node's size in its [`LayoutState`](crate::LayoutState), each child's
/// offset in its parent's [`Children`](crate::Children).
///
/// Re-entrant: a [`Measure`] implementation may call this on an attachment subtree with the
/// same `cx` and the same measurer.
pub fn layout<N: LayoutNode, M: Measure<N>>(
    root: &mut Handle<N>,
    constraints: RootConstraints<N::Scalar>,
    cx: &mut LayoutCx<'_, N::Scalar>,
    measure: &mut M,
) -> Result<Size<N::Scalar>, Cancelled> {
    if cx.depth == 0 {
        cx.scratch.leaf_table.clear();
        cx.scratch.size_table.clear();
        cx.stats = Stats::default();
    }
    cx.depth += 1;
    let direction = match constraints.direction {
        Direction::Inherit => Direction::Ltr,
        d => d,
    };
    let parent_size = constraints.available.map(|a| a.definite());
    // The root's own margins come out of the space available to it, as a parent would do.
    let margin = root.parts().style.resolve_edges(slot::MARGIN, parent_size.width).sums();
    let available = Size::new(constraints.available.width.shrink(margin.width), constraints.available.height.shrink(margin.height));
    let input = Input::new(constraints.known, available, parent_size, direction).finalize();
    let result = Pass { cx, measure, _n: PhantomData }.visit(root, input).map(|(size, _)| size);
    cx.depth -= 1;
    result
}

/// The constraints a node is visited under. Crate-private; [`ConstraintKey`] is its public face.
#[derive(Copy, Clone)]
pub(crate) struct Input<S: Scalar> {
    pub key: ConstraintKey,
    /// Border-box sizes imposed by the parent.
    pub known: Size<Opt<S>>,
    pub available: Size<Available<S>>,
    /// The containing block's content-box size; the percentage basis.
    pub parent_size: Size<Opt<S>>,
    pub direction: Direction,
    /// Per axis: the imposed size is not definite (not a percentage basis for children).
    pub indefinite: Size<bool>,
    /// Asks for the content size: on axes without an imposed size, the node's own size, min
    /// and max properties (and aspect ratio) are ignored. This is how a container asks an item
    /// for its flex base size or its contributions; the item's properties are the container's
    /// business.
    pub content_size: bool,
    /// True for a final layout, which positions children and writes the node's size; false for
    /// a size query, which only answers how big the node would be.
    pub final_layout: bool,
}

impl<S: Scalar> Input<S> {
    pub(crate) fn new(
        known: Size<Opt<S>>,
        available: Size<Available<S>>,
        parent_size: Size<Opt<S>>,
        direction: Direction,
    ) -> Self {
        let mut flags = 0;
        let axis_bits = |axis: Axis, flag: u32, flags: &mut u32| match known.get(axis).get() {
            Some(v) => {
                *flags |= flag;
                v.key_bits()
            }
            None => available.get(axis).key_bits(),
        };
        let w = axis_bits(Axis::Horizontal, ConstraintKey::KNOWN_W, &mut flags);
        let h = axis_bits(Axis::Vertical, ConstraintKey::KNOWN_H, &mut flags);
        if direction == Direction::Rtl {
            flags |= ConstraintKey::RTL;
        }
        flags |= ConstraintKey::COMPUTE_SIZE;
        let key =
            ConstraintKey::new([w, h], [parent_size.width.key_bits(), parent_size.height.key_bits()], flags);
        Input {
            key,
            known,
            available,
            parent_size,
            direction,
            indefinite: Size::new(false, false),
            content_size: false,
            final_layout: false,
        }
    }
    /// Marks imposed sizes as indefinite, per axis. Only axes with an imposed size are marked;
    /// only the keys of containers carry it (see `Pass::node_key`).
    pub(crate) fn with_indefinite(mut self, indefinite: Size<bool>) -> Self {
        self.indefinite = Size::new(
            indefinite.width && self.known.width.is_some(),
            indefinite.height && self.known.height.is_some(),
        );
        self
    }
    /// Asks for the content size. Only the keys of nodes it can affect carry it (see
    /// `Pass::node_key`).
    pub(crate) fn content_size(mut self) -> Self {
        self.content_size = true;
        self
    }
    /// Turns a size query into a final layout.
    #[inline]
    pub(crate) fn finalize(mut self) -> Self {
        self.final_layout = true;
        self.key = self.key.as_final();
        self
    }
    #[inline]
    fn is_final(&self) -> bool {
        self.final_layout
    }
}

/// A node's answer: its border-box size and first baseline.
type Answer<S> = (Size<S>, Opt<S>);

pub(crate) struct Pass<'c, 'a, N: LayoutNode, M> {
    pub(crate) cx: &'c mut LayoutCx<'a, N::Scalar>,
    pub(crate) measure: &'c mut M,
    _n: PhantomData<fn(&N)>,
}

impl<'c, 'a, N: LayoutNode, M: Measure<N>> Pass<'c, 'a, N, M> {
    /// The key a node's answers are stored under. A node whose own style has no percentages (or
    /// `stretch`) cannot depend on its containing block's size: its children resolve against
    /// its own size, and the space available to it is in the key already. So the parent size is
    /// dropped and the same answer serves every parent context.
    fn node_key(node: &N, input: &Input<N::Scalar>) -> ConstraintKey {
        let p = node.parts();
        let measured = p.state.kind() == NodeKind::Measured;
        let mut key = if p.style.uses_percentages() { input.key } else { input.key.without_parent() };
        // Definiteness only changes how a container resolves its children's percentages.
        if !measured {
            if input.indefinite.width {
                key = key.with_flag(ConstraintKey::INDEFINITE_W);
            }
            if input.indefinite.height {
                key = key.with_flag(ConstraintKey::INDEFINITE_H);
            }
        }
        // A content-size question differs only for a node with its own size, min or max styles
        // (or an aspect ratio) on an axis the question leaves open.
        let s = p.style;
        let sized = |l: Length<N::Scalar>| !matches!(l, Length::Auto | Length::None);
        if input.content_size
            && ([s.width(), s.height(), s.min_width(), s.min_height(), s.max_width(), s.max_height()].into_iter().any(sized)
                || s.aspect_ratio().is_some())
        {
            key.with_flag(ConstraintKey::CONTENT_SIZE)
        } else {
            key
        }
    }

    /// Can `node` answer `input` without being visited mutably?
    ///
    /// A final layout (both sizes imposed) is answered only by the final-layout memo, because a
    /// memo hit guarantees the node's stored size is the memoised one. A size query may also be
    /// answered by the measurement slots, relaxed for leaves.
    fn probe(node: &N, input: &Input<N::Scalar>) -> Option<Answer<N::Scalar>> {
        let p = node.parts();
        if p.children.is_touched() || p.state.style_key() != node.style_key() {
            return None;
        }
        let key = Self::node_key(node, input);
        if input.is_final() {
            return p.state.memo(&key);
        }
        // A final layout under the same constraints also answers a size query.
        if let Some(hit) = p.state.memo(&key.as_final()) {
            return Some(hit);
        }
        if p.state.kind() == NodeKind::Measured {
            p.state.slot_relaxed(&key)
        } else {
            p.state.slot(&key)
        }
    }

    pub(crate) fn visit(&mut self, handle: &mut Handle<N>, input: Input<N::Scalar>) -> Result<Answer<N::Scalar>, Cancelled> {
        if let Some(flag) = self.cx.cancel
            && flag.load(Ordering::Relaxed)
        {
            return Err(Cancelled);
        }
        self.cx.stats.visited += 1;

        // 1. probe
        if let Some(hit) = Self::probe(&**handle, &input) {
            self.cx.stats.memo_hits += 1;
            return Ok(hit);
        }
        // A container size query answered earlier in this pass (its slots may have evicted it).
        let is_container = (**handle).parts().state.kind() != NodeKind::Measured;
        if is_container && !input.is_final() {
            let addr = &**handle as *const N as usize;
            if let Some(hit) = self.cx.scratch.size_table.get(addr, &Self::node_key(&**handle, &input)) {
                self.cx.stats.pass_table_hits += 1;
                return Ok(hit);
            }
        }

        // 2. make unique
        if Handle::get_mut(handle).is_none() {
            self.cx.stats.nodes_cloned += 1;
        }
        let node: &mut N = Handle::make_mut(handle);

        // 3. invalidate
        let style_key = node.style_key();
        let (kind, display) = {
            let p = node.parts_mut();
            if p.state.style_key() != style_key {
                p.state.clear_caches();
                p.state.set_style_key(style_key);
            }
            if p.children.is_touched() {
                p.state.clear_caches();
            }
            (p.state.kind(), p.style.display())
        };

        // 4. compute
        let (size, baseline) = if display == Display::None {
            (Size::ZERO, Opt::NONE)
        } else {
            match kind {
                NodeKind::Measured => self.measure_leaf(node, input)?,
                // A `contents` node reached directly (as a root) is laid out as a container.
                NodeKind::Container | NodeKind::Contents => flexbox::layout_container(self, node, input)?,
            }
        };

        // 5. record. Only a final layout writes the node's size; size queries leave it alone,
        // so a memo hit always finds the stored size equal to the memoised one.
        let key = Self::node_key(node, &input);
        if is_container && !input.is_final() {
            self.cx.scratch.size_table.insert(node as *const N as usize, key, (size, baseline));
        }
        let p = node.parts_mut();
        if input.is_final() {
            p.state.record_memo(key, size, baseline);
            p.state.set_size(size);
        } else {
            p.state.record_slot(key, size, baseline);
        }
        p.children.clear_touched();
        Ok((size, baseline))
    }

    /// Visits the descendant of `node` at `path` (a child, or a child of `display: contents`
    /// nodes), probing before making anything on the path unique.
    pub(crate) fn visit_path(
        &mut self,
        node: &mut N,
        path: &[u32],
        input: Input<N::Scalar>,
    ) -> Result<Answer<N::Scalar>, Cancelled> {
        let probed = {
            let mut n: &N = node;
            for &i in path {
                n = &n.parts().children[i as usize].node;
            }
            Self::probe(n, &input)
        };
        if let Some(hit) = probed {
            self.cx.stats.visited += 1;
            self.cx.stats.memo_hits += 1;
            return Ok(hit);
        }
        let slot = self.slot_mut(node, path);
        self.visit(&mut slot.node, input)
    }

    /// Mutable access to the slot at `path`, cloning shared lists and intermediate
    /// (`display: contents`) nodes on the way.
    fn slot_mut<'n>(&mut self, node: &'n mut N, path: &[u32]) -> &'n mut Slot<N> {
        let (list, cloned) = node.parts_mut().children.cow();
        if cloned {
            self.cx.stats.lists_cloned += 1;
        }
        let mut slot: &'n mut Slot<N> = &mut list[path[0] as usize];
        for &i in &path[1..] {
            if Handle::get_mut(&mut slot.node).is_none() {
                self.cx.stats.nodes_cloned += 1;
            }
            let inner: &'n mut N = Handle::make_mut(&mut slot.node);
            let (list, cloned) = inner.parts_mut().children.cow();
            if cloned {
                self.cx.stats.lists_cloned += 1;
            }
            slot = &mut list[i as usize];
        }
        slot
    }

    /// Writes the offset of the descendant at `path` into its parent's list, cloning only if
    /// the value changed. A `display: contents` node on the path keeps a zero offset and size.
    pub(crate) fn set_offset_path(&mut self, node: &mut N, path: &[u32], offset: Point<N::Scalar>) {
        let current = {
            let mut n: &N = node;
            for &i in &path[..path.len() - 1] {
                n = &n.parts().children[i as usize].node;
            }
            let list = n.parts().children;
            let slot = &list[*path.last().unwrap() as usize];
            slot.offset
        };
        if current.x.key_bits() == offset.x.key_bits() && current.y.key_bits() == offset.y.key_bits() {
            return;
        }
        let slot = self.slot_mut(node, path);
        slot.offset = offset;
    }

    /// A measured leaf: style sizes first, then slots, the per-pass table, the shared cache,
    /// and only then the measurer.
    fn measure_leaf(&mut self, node: &mut N, input: Input<N::Scalar>) -> Result<Answer<N::Scalar>, Cancelled> {
        type S<N> = <N as LayoutNode>::Scalar;
        let style = *node.parts().style;
        let direction = match style.direction() {
            Direction::Inherit => input.direction,
            d => d,
        };
        let inset = flexbox::content_inset(&style, input.parent_size.width, direction);
        let inset_sum = inset.sums();
        let pb_sum = flexbox::padding_border(&style, input.parent_size.width).sums();
        let content_box = style.box_sizing() == BoxSizing::ContentBox;
        let to_bb = |axis: Axis, v: S<N>| if content_box { v + pb_sum.get(axis) } else { v };
        let bounds = |axis: Axis| -> (Opt<S<N>>, Opt<S<N>>) {
            if input.content_size {
                return (Opt::NONE, Opt::NONE);
            }
            let (_, min_slot, max_slot) = slot::sizing(axis);
            let parent = input.parent_size.get(axis);
            (style.resolve(min_slot, parent).map(|v| to_bb(axis, v)), style.resolve(max_slot, parent).map(|v| to_bb(axis, v)))
        };

        // Border-box sizes decided before measuring: imposed by the parent, or set in the style
        // (not when the question is for the content size).
        let mut keyword: Size<Option<Available<S<N>>>> = Size::new(None, None);
        let mut decided: Size<Opt<S<N>>> = Size::from_fn(|axis| {
            if input.known.get(axis).is_some() {
                return input.known.get(axis);
            }
            if input.content_size {
                return Opt::NONE;
            }
            let (size_slot, _, _) = slot::sizing(axis);
            let parent = input.parent_size.get(axis);
            let (min, max) = bounds(axis);
            match style.length(size_slot) {
                Length::MinContent => {
                    keyword.set(axis, Some(Available::MinContent));
                    Opt::NONE
                }
                Length::MaxContent => {
                    keyword.set(axis, Some(Available::MaxContent));
                    Opt::NONE
                }
                Length::Stretch => {
                    let margins = style.resolve_edges(slot::MARGIN, input.parent_size.width).sum(axis);
                    parent.map(|p| (p - margins).max(S::<N>::ZERO).clamp_opt(min, max))
                }
                _ => style.resolve(size_slot, parent).map(|v| to_bb(axis, v)).map(|v| v.clamp_opt(min, max)),
            }
        });
        let ratio = if input.content_size { Opt::NONE } else { style.aspect_ratio() };
        if let Some(r) = ratio.get() {
            let transfer = |from: Axis, v: S<N>| {
                let inner = if content_box { (v - pb_sum.get(from)).max(S::<N>::ZERO) } else { v };
                let other = match from {
                    Axis::Horizontal => inner / r,
                    Axis::Vertical => inner * r,
                };
                let (min, max) = bounds(from.cross());
                (if content_box { other + pb_sum.get(from.cross()) } else { other }).clamp_opt(min, max)
            };
            match (decided.width.get(), decided.height.get()) {
                (Some(w), None) => decided.height = Opt::some(transfer(Axis::Horizontal, w)),
                (None, Some(h)) => decided.width = Opt::some(transfer(Axis::Vertical, h)),
                _ => {}
            }
        }

        if let (Some(w), Some(h)) = (decided.width.get(), decided.height.get()) {
            return Ok((Size::new(w.max(pb_sum.width), h.max(pb_sum.height)), Opt::NONE));
        }

        // Cached answers, in order of cost.
        let key = Self::node_key(node, &input);
        let (content_id, slot_hit) = {
            let p = node.parts();
            (p.state.content_id(), p.state.slot_relaxed(&key))
        };
        if let Some(hit) = slot_hit {
            self.cx.stats.slot_hits += 1;
            return Ok(hit);
        }
        let addr = node as *const N as usize;
        let cached: Option<Measured<S<N>>> = if let Some(m) = self.cx.scratch.leaf_table.get(addr, &key) {
            self.cx.stats.pass_table_hits += 1;
            Some(m)
        } else if content_id != 0 {
            match self.cx.cache.and_then(|c| c.get(content_id, &key, self.cx.epoch)) {
                Some(m) => {
                    self.cx.stats.shared_hits += 1;
                    Some(m)
                }
                None => None,
            }
        } else {
            None
        };

        let measured = match cached {
            Some(m) => m,
            None => {
                self.cx.stats.measured += 1;
                let available = Size::from_fn(|axis| match decided.get(axis).get() {
                    Some(v) => Available::Definite((v - inset_sum.get(axis)).max(S::<N>::ZERO)),
                    None => match keyword.get(axis) {
                        Some(k) => k,
                        None => input.available.get(axis).shrink(inset_sum.get(axis)),
                    },
                });
                let measure_input = MeasureInput {
                    known: Size::new(
                        decided.width.map(|v| (v - inset_sum.width).max(S::<N>::ZERO)),
                        decided.height.map(|v| (v - inset_sum.height).max(S::<N>::ZERO)),
                    ),
                    available,
                };
                let m = self.measure.measure(node, measure_input, self.cx);
                self.cx.scratch.leaf_table.insert(addr, key, m);
                if content_id != 0
                    && let Some(c) = self.cx.cache
                {
                    c.insert(content_id, key, self.cx.epoch, m);
                }
                m
            }
        };

        // Content box back to border box, decided axes winning, clamped by the style's bounds.
        let mut size = Size::from_fn(|axis| match decided.get(axis).get() {
            Some(v) => v,
            None => {
                let (min, max) = bounds(axis);
                (measured.size.get(axis) + inset_sum.get(axis)).clamp_opt(min, max)
            }
        });
        // A ratio-dependent height is at least the transferred width.
        if let (Some(r), None) = (ratio.get(), decided.height.get()) {
            let inner_w = if content_box { (size.width - pb_sum.width).max(S::<N>::ZERO) } else { size.width };
            let via = if content_box { inner_w / r + pb_sum.height } else { inner_w / r };
            let (min, max) = bounds(Axis::Vertical);
            size.height = size.height.max(via.clamp_opt(min, max));
        }
        size.width = size.width.max(pb_sum.width);
        size.height = size.height.max(pb_sum.height);
        let baseline = measured.first_baseline.map(|b| b + inset.top);
        Ok((size, baseline)) // recorded by `visit`
    }
}
