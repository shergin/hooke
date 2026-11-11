//! The per-node layout state that a host embeds in its node.
//!
//! [`LayoutState`] is 192 bytes for `f32` with the default four measurement slots, 128 with two. It holds the
//! node's border-box size, the memo of its last final layout, a few persistent answers to
//! measurement questions, the content id of a measured leaf, and the last style key the engine
//! saw. The node's *offset* is not here: it belongs to the parent's [`Slot`](crate::Slot).
//!
//! The engine writes this through `&mut` it obtains by copy-on-write. Hosts read sizes and
//! flags, and call the two invalidation methods when measured content changes.

use crate::geometry::Size;
use crate::scalar::{Opt, Scalar};
use core::fmt;

/// Number of persistent measurement slots per node: four by default, two with `slots-2`.
///
/// One parent pass can ask a leaf up to three distinct questions (a content-based flex base
/// size, a hypothetical cross size, and the final layout), and the first two change when the
/// parent's own cross size goes from indefinite to definite. Four slots hold one steady state
/// without eviction; two are enough for leaves whose size never depends on the parent.
#[cfg(feature = "slots-2")]
pub const DEFAULT_SLOTS: usize = 2;
/// Number of persistent measurement slots per node: four by default, two with `slots-2`.
#[cfg(not(feature = "slots-2"))]
pub const DEFAULT_SLOTS: usize = 4;

/// How a node participates in layout.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NodeKind {
    /// A flex container: its size comes from its children and its style.
    Container,
    /// A leaf sized by the host's [`Measure`](crate::Measure) callback: text, images, inputs.
    Measured,
    /// A node that generates no box; its children are laid out by the nearest box ancestor.
    Contents,
}

/// The constraints a node was sized under, as an opaque, hashable key.
///
/// Per axis it records the size the parent imposed if any, otherwise the available space
/// (definite, min-content or max-content); plus the containing block's size on both axes, which
/// is the percentage basis, and a few flags including the inline direction. Two
/// questions with equal keys have equal answers for the same node content, which is what makes
/// both the in-node slots and a [`MeasureCache`](crate::MeasureCache) sound.
#[repr(C)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ConstraintKey {
    axis: [u32; 2],
    parent: [u32; 2],
    flags: u32,
}

impl ConstraintKey {
    pub(crate) const MIN_CONTENT: u32 = 0xFF80_0001;
    pub(crate) const MAX_CONTENT: u32 = 0xFF80_0002;
    pub(crate) const KNOWN_W: u32 = 1 << 0;
    pub(crate) const KNOWN_H: u32 = 1 << 1;
    pub(crate) const RTL: u32 = 1 << 2;
    /// The question only asks for a size; children are not positioned.
    pub(crate) const COMPUTE_SIZE: u32 = 1 << 3;
    /// The imposed width (height) is not definite: it came from flexing in a container whose
    /// own main size is not definite, so it is not a percentage basis for the node's children.
    pub(crate) const INDEFINITE_W: u32 = 1 << 4;
    pub(crate) const INDEFINITE_H: u32 = 1 << 5;
    /// Content-size question: the node's own size, min and max styles are ignored (a measured
    /// leaf answers its content size).
    pub(crate) const CONTENT_SIZE: u32 = 1 << 6;
    const VALID: u32 = 1 << 31;

    #[inline]
    pub(crate) fn new(axis: [u32; 2], parent: [u32; 2], flags: u32) -> Self {
        ConstraintKey {
            axis,
            parent,
            flags: flags & !Self::VALID,
        }
    }
    #[inline]
    fn valid(self) -> Self {
        ConstraintKey {
            flags: self.flags | Self::VALID,
            ..self
        }
    }
    /// The key with the containing-block size removed, for nodes whose answer cannot depend on it.
    #[inline]
    pub(crate) fn without_parent(self) -> Self {
        ConstraintKey { parent: [0; 2], ..self }
    }
    /// One of the key's five words, for hashing: the two axes, the two parent words, the flags.
    #[inline]
    pub(crate) fn word(&self, i: usize) -> u32 {
        match i {
            0 => self.axis[0],
            1 => self.axis[1],
            2 => self.parent[0],
            3 => self.parent[1],
            _ => self.flags,
        }
    }
    /// The key with `flag` set.
    #[inline]
    pub(crate) fn with_flag(self, flag: u32) -> Self {
        ConstraintKey {
            flags: self.flags | (flag & !Self::VALID),
            ..self
        }
    }
    /// The same constraints asked as a final layout rather than a size query.
    #[inline]
    pub(crate) fn as_final(self) -> Self {
        ConstraintKey {
            flags: self.flags & !Self::COMPUTE_SIZE,
            ..self
        }
    }
    #[inline]
    fn known(&self, axis: usize) -> bool {
        self.flags & if axis == 0 { Self::KNOWN_W } else { Self::KNOWN_H } != 0
    }
}

/// One axis of a constraint, decoded from its key bits.
#[derive(Copy, Clone, PartialEq)]
enum AxisConstraint<S> {
    Known(S),
    Definite(S),
    MinContent,
    MaxContent,
}

impl<S: Scalar> AxisConstraint<S> {
    fn decode(key: &ConstraintKey, axis: usize) -> Option<Self> {
        let bits = key.axis[axis];
        if key.known(axis) {
            return S::from_key_bits(bits).map(AxisConstraint::Known);
        }
        match bits {
            ConstraintKey::MIN_CONTENT => Some(AxisConstraint::MinContent),
            ConstraintKey::MAX_CONTENT => Some(AxisConstraint::MaxContent),
            _ => S::from_key_bits(bits).map(AxisConstraint::Definite),
        }
    }
    /// Yoga's `canUseCachedMeasurement` rules for one axis: the question `new` can be answered
    /// by an entry asked `old` that produced `result`.
    fn compatible(new: Self, old: Self, result: S) -> bool {
        use AxisConstraint::*;
        match (new, old) {
            (a, b) if a == b => true,
            // The new question imposes exactly the size the old answer produced.
            (Known(v), _) => v.approx_eq(result),
            // The old answer was unconstrained and still fits the new definite space.
            (Definite(avail), MaxContent) => result <= avail + S::EPSILON,
            // The new definite space is tighter than the old one and the old answer still fits.
            (Definite(avail), Definite(old_avail)) => old_avail > avail && result <= avail + S::EPSILON,
            _ => false,
        }
    }
}

/// One cached answer.
#[repr(C)]
#[derive(Copy, Clone)]
struct Entry<S: Scalar> {
    key: ConstraintKey,
    size: Size<S>,
    baseline: Opt<S>,
}

impl<S: Scalar> Entry<S> {
    const EMPTY: Self = Entry {
        key: ConstraintKey {
            axis: [0; 2],
            parent: [0; 2],
            flags: 0,
        },
        size: Size::ZERO,
        baseline: Opt::NONE,
    };
}

const F_KIND: u32 = 0b11;
const F_NEW_LAYOUT: u32 = 1 << 2;

/// The engine's per-node state. Embed one in every node; see the [module docs](self).
#[repr(C)]
#[derive(Copy, Clone)]
pub struct LayoutState<S: Scalar = f32, const K: usize = DEFAULT_SLOTS> {
    size: Size<S>,
    memo: Entry<S>,
    slots: [Entry<S>; K],
    content: u64,
    style_key: u64,
    flags: u32,
    clock: u8,
    used: u8,
    _pad: [u8; 2],
}

const _: () = assert!(core::mem::size_of::<LayoutState<f32, 2>>() == 128);
const _: () = assert!(core::mem::size_of::<LayoutState<f32, 4>>() == 192);

impl<S: Scalar, const K: usize> LayoutState<S, K> {
    const fn with_kind(kind: NodeKind) -> Self {
        LayoutState {
            size: Size::ZERO,
            memo: Entry::EMPTY,
            slots: [Entry::EMPTY; K],
            content: 0,
            style_key: 0,
            flags: kind as u32,
            clock: 0,
            used: 0,
            _pad: [0; 2],
        }
    }

    /// State for a flex container.
    pub const fn container() -> Self {
        Self::with_kind(NodeKind::Container)
    }
    /// State for a leaf sized by the host's measurer. `content_id` names the measured
    /// *content* (the text, the image) and keys the shared cache; it must be non-zero to be
    /// cached across revisions.
    pub const fn measured(content_id: u64) -> Self {
        let mut s = Self::with_kind(NodeKind::Measured);
        s.content = content_id;
        s
    }
    /// State for a node that generates no box.
    pub const fn contents() -> Self {
        Self::with_kind(NodeKind::Contents)
    }

    /// How this node participates in layout.
    #[inline]
    pub fn kind(&self) -> NodeKind {
        match self.flags & F_KIND {
            0 => NodeKind::Container,
            1 => NodeKind::Measured,
            _ => NodeKind::Contents,
        }
    }
    /// The border-box size written by the last pass. The offset lives in the parent's slot.
    #[inline]
    pub fn size(&self) -> Size<S> {
        self.size
    }
    /// True if the last pass that wrote this node changed its size. Cleared by the host after
    /// it has reacted (for example by firing an `onLayout` event).
    #[inline]
    pub fn has_new_layout(&self) -> bool {
        self.flags & F_NEW_LAYOUT != 0
    }
    /// Clears [`has_new_layout`](Self::has_new_layout).
    #[inline]
    pub fn clear_new_layout(&mut self) {
        self.flags &= !F_NEW_LAYOUT;
    }
    /// The content id of a measured leaf; zero for containers.
    #[inline]
    pub fn content_id(&self) -> u64 {
        self.content
    }
    /// Host: the measured content changed. Clears this node's cached answers and makes the
    /// new content the key of future shared-cache lookups.
    pub fn set_content_id(&mut self, id: u64) {
        self.content = id;
        self.clear_caches();
    }
    /// Host: drop every cached answer. Rarely needed, because style changes are detected
    /// through [`LayoutNode::style_key`](crate::LayoutNode::style_key) and structural changes
    /// through [`Children::make_mut`](crate::Children::make_mut).
    pub fn invalidate(&mut self) {
        self.clear_caches();
    }

    // ---- engine side ----

    #[inline]
    pub(crate) fn style_key(&self) -> u64 {
        self.style_key
    }
    #[inline]
    pub(crate) fn set_style_key(&mut self, k: u64) {
        self.style_key = k;
    }
    pub(crate) fn clear_caches(&mut self) {
        self.memo.key.flags = 0;
        for e in self.slots.iter_mut() {
            e.key.flags = 0;
        }
        self.used = 0;
        self.clock = 0;
    }
    /// The memoised final layout for `key`, if any.
    #[inline]
    pub(crate) fn memo(&self, key: &ConstraintKey) -> Option<(Size<S>, Opt<S>)> {
        if self.memo.key.flags & ConstraintKey::VALID != 0 && self.memo.key == key.valid() {
            Some((self.memo.size, self.memo.baseline))
        } else {
            None
        }
    }
    /// A persistent measurement answer for `key`, if any.
    #[inline]
    pub(crate) fn slot(&self, key: &ConstraintKey) -> Option<(Size<S>, Opt<S>)> {
        let k = key.valid();
        self.slots[..self.used as usize]
            .iter()
            .find(|e| e.key == k)
            .map(|e| (e.size, e.baseline))
    }
    /// Like [`slot`](Self::slot) but also accepts entries that answer `key` under Yoga's
    /// relaxed rules. Only sound for measured leaves, whose answers depend on nothing but their
    /// own constraints.
    pub(crate) fn slot_relaxed(&self, key: &ConstraintKey) -> Option<(Size<S>, Opt<S>)> {
        if let Some(exact) = self.slot(key) {
            return Some(exact);
        }
        let k = key.valid();
        let new = [AxisConstraint::<S>::decode(&k, 0)?, AxisConstraint::<S>::decode(&k, 1)?];
        for e in self.slots[..self.used as usize].iter() {
            if e.key.parent != k.parent
                || (e.key.flags ^ k.flags) & !(ConstraintKey::KNOWN_W | ConstraintKey::KNOWN_H) != 0
            {
                continue;
            }
            let (Some(old_w), Some(old_h)) = (
                AxisConstraint::<S>::decode(&e.key, 0),
                AxisConstraint::<S>::decode(&e.key, 1),
            ) else {
                continue;
            };
            if AxisConstraint::compatible(new[0], old_w, e.size.width)
                && AxisConstraint::compatible(new[1], old_h, e.size.height)
            {
                let size = Size::new(
                    if let AxisConstraint::Known(v) = new[0] {
                        v
                    } else {
                        e.size.width
                    },
                    if let AxisConstraint::Known(v) = new[1] {
                        v
                    } else {
                        e.size.height
                    },
                );
                return Some((size, e.baseline));
            }
        }
        None
    }
    pub(crate) fn record_memo(&mut self, key: ConstraintKey, size: Size<S>, baseline: Opt<S>) {
        self.memo = Entry {
            key: key.valid(),
            size,
            baseline,
        };
    }
    /// Records a measurement answer, evicting round-robin once the slots are full.
    pub(crate) fn record_slot(&mut self, key: ConstraintKey, size: Size<S>, baseline: Opt<S>) {
        let e = Entry {
            key: key.valid(),
            size,
            baseline,
        };
        if (self.used as usize) < K {
            self.slots[self.used as usize] = e;
            self.used += 1;
        } else if K > 0 {
            let i = self.clock as usize % K;
            self.slots[i] = e;
            self.clock = self.clock.wrapping_add(1);
        }
    }
    /// Writes the size; returns true and sets `has_new_layout` if it changed.
    pub(crate) fn set_size(&mut self, size: Size<S>) -> bool {
        let changed = self.size.width.key_bits() != size.width.key_bits()
            || self.size.height.key_bits() != size.height.key_bits();
        if changed {
            self.size = size;
            self.flags |= F_NEW_LAYOUT;
        }
        changed
    }
}

impl<S: Scalar, const K: usize> fmt::Debug for LayoutState<S, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LayoutState")
            .field("kind", &self.kind())
            .field("size", &self.size)
            .field("has_new_layout", &self.has_new_layout())
            .field("content_id", &self.content)
            .field("cached_measurements", &self.used)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_fill_then_rotate() {
        let mut s: LayoutState<f32, 2> = LayoutState::measured(1);
        let k = |i: u32| ConstraintKey::new([i, 0], [0, 0], 0);
        s.record_slot(k(1), Size::new(1.0, 1.0), Opt::NONE);
        s.record_slot(k(2), Size::new(2.0, 2.0), Opt::NONE);
        assert_eq!(s.slot(&k(1)).map(|r| r.0.width), Some(1.0));
        s.record_slot(k(3), Size::new(3.0, 3.0), Opt::NONE);
        assert_eq!(s.slot(&k(1)), None, "oldest evicted");
        assert_eq!(s.slot(&k(3)).map(|r| r.0.width), Some(3.0));
        s.set_content_id(2);
        assert_eq!(s.slot(&k(3)), None, "content change clears slots");
    }

    #[test]
    fn memo_requires_exact_key() {
        let mut s: LayoutState = LayoutState::container();
        let k = ConstraintKey::new([5, 6], [7, 8], ConstraintKey::KNOWN_W);
        assert_eq!(s.memo(&k), None);
        s.record_memo(k, Size::new(10.0, 20.0), Opt::NONE);
        assert_eq!(s.memo(&k).map(|r| r.0), Some(Size::new(10.0, 20.0)));
        assert_eq!(s.memo(&ConstraintKey::new([5, 6], [7, 8], 0)), None);
        assert!(s.set_size(Size::new(10.0, 20.0)));
        assert!(s.has_new_layout());
        assert!(!s.set_size(Size::new(10.0, 20.0)));
    }
}
