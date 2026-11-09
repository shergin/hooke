//! The child list: one shared allocation of handles plus offsets, copy-on-write by uniqueness.
//!
//! [`Children`] is the only write path a host has into a subtree, which is what lets it mark
//! dirtiness mechanically: every [`Children::make_mut`] on the way to an edit marks that level,
//! and the engine clears the mark after laying the container out. It also carries each child's
//! offset, so a sibling that merely moves is never cloned, and its allocation identity is
//! meaningful: the same allocation means the same children with the same sizes and offsets.
//!
//! It dereferences to `[Slot<N>]`, so reading is ordinary slice code: `children.len()`,
//! `children.iter()`, `&children[i]`. Writing is explicit.

use crate::geometry::Point;
use crate::scalar::Scalar;
use crate::tree::LayoutNode;
use crate::Handle;
use core::fmt;
use core::ops::Deref;

/// One child: its handle and its parent-relative offset.
///
/// The offset is written by the engine. The host reads it to mount the child and may treat
/// `node` like any other handle.
pub struct Slot<N: LayoutNode> {
    /// The child node.
    pub node: Handle<N>,
    /// The child's border-box origin relative to this parent's border box.
    pub offset: Point<N::Scalar>,
}

impl<N: LayoutNode> Slot<N> {
    /// Mutable access to the child, cloning it first if anything else still refers to it.
    #[inline]
    pub fn node_mut(&mut self) -> &mut N {
        Handle::make_mut(&mut self.node)
    }
}

impl<N: LayoutNode> Clone for Slot<N> {
    #[inline]
    fn clone(&self) -> Self {
        Slot { node: self.node.clone(), offset: self.offset }
    }
}

impl<N: LayoutNode> fmt::Debug for Slot<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Slot").field("offset", &self.offset).field("size", &self.node.parts().state.size()).finish()
    }
}

/// A node's children: a shared, contiguous list of [`Slot`]s.
pub struct Children<N: LayoutNode> {
    list: Option<Handle<[Slot<N>]>>,
    touched: bool,
}

impl<N: LayoutNode> Children<N> {
    /// No children, and no allocation.
    #[inline]
    pub const fn empty() -> Self {
        Children { list: None, touched: false }
    }

    /// Builds a list from child handles in one allocation when the iterator knows its length.
    ///
    /// Equivalent to `nodes.into_iter().collect()`.
    #[inline]
    pub fn from_nodes<I>(nodes: I) -> Self
    where
        I: IntoIterator<Item = Handle<N>>,
    {
        nodes.into_iter().collect()
    }

    /// The slots. Also available through `Deref`.
    #[inline]
    pub fn slots(&self) -> &[Slot<N>] {
        self.list.as_deref().unwrap_or(&[])
    }

    /// True when both lists are the same allocation, which implies the same children with the
    /// same sizes and offsets. Two empty lists are equal.
    #[inline]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        match (&self.list, &other.list) {
            (Some(a), Some(b)) => Handle::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    /// True if the host has written through this list since the engine last laid the parent out.
    #[inline]
    pub fn is_touched(&self) -> bool {
        self.touched
    }

    /// The host's write path. Clones the list if it is shared, and marks this level as changed
    /// so the parent's cached layout is not reused. Reaching a grandchild means calling this on
    /// every level in between, which is exactly the set of nodes whose layout may change.
    #[inline]
    pub fn make_mut(&mut self) -> &mut [Slot<N>] {
        self.touched = true;
        self.cow().0
    }

    /// Like [`make_mut`](Self::make_mut) but without marking the level. For edits the host
    /// promises cannot affect layout (colors, handlers, text that keeps its content id). A
    /// broken promise yields stale layout silently.
    #[inline]
    pub fn make_mut_untracked(&mut self) -> &mut [Slot<N>] {
        self.cow().0
    }

    /// A copy of the list with the child at `index` replaced.
    #[must_use]
    pub fn with_replaced(&self, index: usize, node: Handle<N>) -> Self {
        let mut c = self.clone();
        c.make_mut()[index].node = node;
        c
    }

    /// A copy of the list with `node` inserted at `index`.
    #[must_use]
    pub fn with_inserted(&self, index: usize, node: Handle<N>) -> Self {
        let slots = self.slots();
        let before = slots[..index].iter().cloned();
        let after = slots[index..].iter().cloned();
        let new = core::iter::once(Slot { node, offset: Point::ZERO });
        Self::from_slots(before.chain(new).chain(after))
    }

    /// A copy of the list with the child at `index` removed.
    #[must_use]
    pub fn with_removed(&self, index: usize) -> Self {
        let slots = self.slots();
        let it = slots.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, s)| s.clone());
        Self::from_slots(it)
    }

    fn from_slots<I: Iterator<Item = Slot<N>>>(it: I) -> Self {
        let list: Handle<[Slot<N>]> = it.collect();
        if list.is_empty() {
            Self::empty()
        } else {
            Children { list: Some(list), touched: true }
        }
    }

    /// Engine-only copy-on-write that does not mark. Returns whether the list was cloned.
    #[inline]
    pub(crate) fn cow(&mut self) -> (&mut [Slot<N>], bool) {
        match &mut self.list {
            Some(l) => {
                let cloned = Handle::get_mut(l).is_none();
                (Handle::make_mut(l), cloned)
            }
            None => (&mut [], false),
        }
    }

    #[inline]
    pub(crate) fn clear_touched(&mut self) {
        self.touched = false;
    }
}

impl<N: LayoutNode> Deref for Children<N> {
    type Target = [Slot<N>];
    #[inline]
    fn deref(&self) -> &[Slot<N>] {
        self.slots()
    }
}

impl<N: LayoutNode> Default for Children<N> {
    #[inline]
    fn default() -> Self {
        Self::empty()
    }
}

impl<N: LayoutNode> Clone for Children<N> {
    #[inline]
    fn clone(&self) -> Self {
        Children { list: self.list.clone(), touched: self.touched }
    }
}

impl<N: LayoutNode> FromIterator<Handle<N>> for Children<N> {
    /// One allocation when the iterator reports an exact size; `Arc<[T]>` and `Rc<[T]>`
    /// specialise on that internally.
    fn from_iter<I: IntoIterator<Item = Handle<N>>>(iter: I) -> Self {
        let list: Handle<[Slot<N>]> =
            iter.into_iter().map(|node| Slot { node, offset: Point::<N::Scalar>::ZERO }).collect();
        if list.is_empty() {
            Self::empty()
        } else {
            Children { list: Some(list), touched: true }
        }
    }
}

impl<'a, N: LayoutNode> IntoIterator for &'a Children<N> {
    type Item = &'a Slot<N>;
    type IntoIter = core::slice::Iter<'a, Slot<N>>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.slots().iter()
    }
}

impl<N: LayoutNode> fmt::Debug for Children<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Children").field("len", &self.len()).field("touched", &self.touched).finish()
    }
}

// Silence the unused-import lint for `Scalar` on configurations where it is only used in bounds.
#[allow(dead_code)]
fn _assert_scalar<S: Scalar>() {}
