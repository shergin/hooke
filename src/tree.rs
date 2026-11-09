//! The host contract: how the engine reads and writes your nodes.
//!
//! A host implements [`LayoutNode`] for its node type with two field projections and,
//! optionally, a cheap style key. The engine never sees the node's other fields and never
//! allocates one.

use crate::children::Children;
use crate::scalar::Scalar;
use crate::state::LayoutState;
use crate::style::Style;

/// Implemented by the host's node type.
///
/// `Clone` must be shallow: bump handles, copy inline state, never deep-copy children. The
/// engine invokes it, through [`Handle::make_mut`](crate::Handle::make_mut), only for nodes a
/// pass writes. It is the clone callback.
///
/// ```
/// # use hooke::{Children, LayoutNode, LayoutState, Parts, PartsMut, Style};
/// # use std::sync::Arc;
/// # struct Props { style: Style, version: u64 }
/// #[derive(Clone)]
/// struct Node { props: Arc<Props>, children: Children<Node>, layout: LayoutState }
///
/// impl LayoutNode for Node {
///     type Scalar = f32;
///     fn parts(&self) -> Parts<'_, Self> {
///         Parts { style: &self.props.style, state: &self.layout, children: &self.children }
///     }
///     fn parts_mut(&mut self) -> PartsMut<'_, Self> {
///         PartsMut { style: &self.props.style, state: &mut self.layout, children: &mut self.children }
///     }
///     fn style_key(&self) -> u64 { self.props.version }
/// }
/// ```
pub trait LayoutNode: Clone + Sized + 'static {
    /// The scalar every size is measured in. `f32` unless you have a reason.
    type Scalar: Scalar;

    /// Shared access to the style, the layout state and the children, all at once.
    fn parts(&self) -> Parts<'_, Self>;

    /// Mutable access to the layout state and the children, with the style alongside.
    ///
    /// Both mutable fields come back together because the algorithm writes a container's own
    /// size while recursing into its children. The style stays shared: styles change by
    /// replacing props, never in place.
    fn parts_mut(&mut self) -> PartsMut<'_, Self>;

    /// A value that changes whenever any layout-affecting style changes.
    ///
    /// The engine compares it on every visit and every probe, so a style change is detected
    /// because it happened, not because the host remembered to report it. The default hashes
    /// the style; a host whose props are immutable and versioned should return the version.
    #[inline]
    fn style_key(&self) -> u64 {
        self.parts().style.fingerprint()
    }
}

/// Shared projections of a node's layout-relevant fields.
pub struct Parts<'a, N: LayoutNode> {
    /// The node's style.
    pub style: &'a Style<N::Scalar>,
    /// The node's layout state.
    pub state: &'a LayoutState<N::Scalar>,
    /// The node's children.
    pub children: &'a Children<N>,
}

/// Mutable projections of a node's layout-relevant fields.
pub struct PartsMut<'a, N: LayoutNode> {
    /// The node's style. Never mutable.
    pub style: &'a Style<N::Scalar>,
    /// The node's layout state.
    pub state: &'a mut LayoutState<N::Scalar>,
    /// The node's children.
    pub children: &'a mut Children<N>,
}
