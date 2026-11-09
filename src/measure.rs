//! Sizing leaves: the measurement callback and the optional shared cache.
//!
//! Containers are sized by the algorithm; everything else (text, images, inputs) is a
//! [`NodeKind::Measured`](crate::NodeKind::Measured) leaf sized by the host's [`Measure`]
//! implementation. The engine calls it only on a cache miss, with content-box constraints, and
//! expects a content-box size and baselines back. Padding, border, min and max sizes and
//! box-sizing are the engine's business.

use crate::geometry::Size;
use crate::pass::LayoutCx;
use crate::scalar::{Opt, Scalar};
use crate::state::ConstraintKey;
use crate::tree::LayoutNode;

/// Available space on one axis.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Available<S> {
    /// A definite amount.
    Definite(S),
    /// Indefinite; size as narrow as possible without overflowing (every break taken).
    MinContent,
    /// Indefinite; size as wide as the content wants (no breaks taken).
    MaxContent,
}

impl<S: Scalar> Available<S> {
    /// The definite amount, if any.
    #[inline]
    pub fn definite(self) -> Opt<S> {
        match self {
            Available::Definite(v) => Opt::some(v),
            _ => Opt::NONE,
        }
    }
    /// Shrinks a definite amount by `by`, never below zero; keywords pass through.
    #[inline]
    pub fn shrink(self, by: S) -> Self {
        match self {
            Available::Definite(v) => Available::Definite((v - by).max(S::ZERO)),
            other => other,
        }
    }
    /// The bits this value contributes to a [`ConstraintKey`].
    #[inline]
    pub(crate) fn key_bits(self) -> u32 {
        match self {
            Available::Definite(v) => v.key_bits(),
            Available::MinContent => ConstraintKey::MIN_CONTENT,
            Available::MaxContent => ConstraintKey::MAX_CONTENT,
        }
    }
}

/// What the engine knows when it asks for a leaf's size, in content-box terms.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct MeasureInput<S: Scalar> {
    /// Sizes already decided by the parent or the style, which the answer must respect.
    pub known: Size<Opt<S>>,
    /// Space available on each axis for the undecided dimensions.
    pub available: Size<Available<S>>,
}

/// A leaf's answer, in content-box terms.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct Measured<S: Scalar> {
    /// The content-box size. Known dimensions should be echoed back.
    pub size: Size<S>,
    /// First baseline below the content-box top, if the content has one.
    pub first_baseline: Opt<S>,
    /// Last baseline below the content-box top, if the content has one.
    pub last_baseline: Opt<S>,
}

impl<S: Scalar> Measured<S> {
    /// A size with no baselines.
    #[inline]
    pub fn size(width: S, height: S) -> Self {
        Measured { size: Size::new(width, height), first_baseline: Opt::NONE, last_baseline: Opt::NONE }
    }
}

/// Sizes measured leaves. Implemented by the host, or by any closure of the same shape.
///
/// The node is passed mutably so a leaf with inline attachments (a paragraph with embedded
/// views) can lay them out re-entrantly through [`layout`](crate::layout) with the same `cx`
/// and the same `self`:
///
/// ```ignore
/// fn measure(&mut self, node: &mut N, input: MeasureInput<f32>, cx: &mut LayoutCx<'_, f32>) -> Measured<f32> {
///     let (slots, _) = node.parts_mut().children.cow();           // engine-side CoW, no mark
///     for slot in slots.iter_mut() {
///         let size = hooke::layout(&mut slot.node, constraints, cx, self)?;
///         /* feed `size` to the shaper as an inline box, then write `slot.offset` */
///     }
///     /* shape and return */
/// }
/// ```
pub trait Measure<N: LayoutNode> {
    /// Measures `node` under `input`. Called only when no cache can answer.
    fn measure(
        &mut self,
        node: &mut N,
        input: MeasureInput<N::Scalar>,
        cx: &mut LayoutCx<'_, N::Scalar>,
    ) -> Measured<N::Scalar>;
}

impl<N, F> Measure<N> for F
where
    N: LayoutNode,
    F: FnMut(&mut N, MeasureInput<N::Scalar>, &mut LayoutCx<'_, N::Scalar>) -> Measured<N::Scalar>,
{
    #[inline]
    fn measure(&mut self, node: &mut N, input: MeasureInput<N::Scalar>, cx: &mut LayoutCx<'_, N::Scalar>) -> Measured<N::Scalar> {
        self(node, input, cx)
    }
}

/// A measurement cache shared across revisions and threads, keyed by content identity.
///
/// Entries never go stale, because a content id names immutable content; they can only be
/// evicted. The host bumps `epoch` for global changes (font scale, locale). Measurements are
/// pure functions of `(content, key, epoch)`, so concurrent inserts of equal keys write equal
/// values and need no coordination beyond the table's own. A candidate that loses a commit
/// race leaves its text shaping here for the retry to find.
pub trait MeasureCache<S: Scalar>: Sync {
    /// A cached answer, if present.
    fn get(&self, content: u64, key: &ConstraintKey, epoch: u32) -> Option<Measured<S>>;
    /// Stores an answer.
    fn insert(&self, content: u64, key: ConstraintKey, epoch: u32, value: Measured<S>);
}

/// A [`MeasureCache`] that never hits. For microcontrollers and tests.
#[derive(Copy, Clone, Debug, Default)]
pub struct NoCache;

impl<S: Scalar> MeasureCache<S> for NoCache {
    #[inline]
    fn get(&self, _: u64, _: &ConstraintKey, _: u32) -> Option<Measured<S>> {
        None
    }
    #[inline]
    fn insert(&self, _: u64, _: ConstraintKey, _: u32, _: Measured<S>) {}
}

/// A bounded, mutex-protected [`MeasureCache`] for `std` hosts.
///
/// Good enough to prove the contract; a sharded table is the production version.
#[cfg(feature = "std")]
pub struct SharedMeasureCache<S: Scalar> {
    map: std::sync::Mutex<std::collections::HashMap<(u64, ConstraintKey, u32), Measured<S>>>,
    capacity: usize,
    hits: core::sync::atomic::AtomicU64,
}

#[cfg(feature = "std")]
impl<S: Scalar> SharedMeasureCache<S> {
    /// A cache that clears itself when it exceeds `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        SharedMeasureCache {
            map: std::sync::Mutex::new(std::collections::HashMap::with_capacity(capacity.min(1 << 16))),
            capacity,
            hits: core::sync::atomic::AtomicU64::new(0),
        }
    }
    /// Entries currently held.
    pub fn len(&self) -> usize {
        self.map.lock().map(|m| m.len()).unwrap_or(0)
    }
    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Hits served since creation.
    pub fn hits(&self) -> u64 {
        self.hits.load(core::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(feature = "std")]
impl<S: Scalar> MeasureCache<S> for SharedMeasureCache<S> {
    fn get(&self, content: u64, key: &ConstraintKey, epoch: u32) -> Option<Measured<S>> {
        let r = self.map.lock().ok()?.get(&(content, *key, epoch)).copied();
        if r.is_some() {
            self.hits.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        r
    }
    fn insert(&self, content: u64, key: ConstraintKey, epoch: u32, value: Measured<S>) {
        if let Ok(mut m) = self.map.lock() {
            if m.len() >= self.capacity {
                m.clear();
            }
            m.insert((content, key, epoch), value);
        }
    }
}

#[cfg(feature = "std")]
impl<S: Scalar> core::fmt::Debug for SharedMeasureCache<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SharedMeasureCache").field("len", &self.len()).field("hits", &self.hits()).finish()
    }
}
