//! The neutral tree description every engine adapter builds from.
//!
//! A [`Scene`] is a flat vector of nodes in pre-order, root first, each with a [`Style`] in the
//! subset of flexbox that hooke, Taffy, Yoga and Cephal all support, and either child indices or
//! a text string (a measured leaf under the shared text model in [`crate::measure`]). Percentages
//! are fractions (`0.5` is 50%). Gaps and borders are absolute lengths; everything else may be a
//! percentage where CSS allows it.
//!
//! Frames are compared in scene node order, so an adapter must keep a mapping from scene index
//! to its own node.

/// Index of a node in [`Scene::nodes`].
pub type NodeId = usize;

/// A length that may be `auto`, absolute or a percentage of the containing block.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub enum Dim {
    #[default]
    Auto,
    Px(f32),
    /// A fraction of the containing block's size on the relevant axis.
    Percent(f32),
}

/// A length that is absolute or a percentage (padding).
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Lp {
    Px(f32),
    Percent(f32),
}

impl Default for Lp {
    fn default() -> Self {
        Lp::Px(0.0)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Display {
    #[default]
    Flex,
    None,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Position {
    #[default]
    Relative,
    Absolute,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum FlexDirection {
    #[default]
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Justify {
    #[default]
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Align {
    #[default]
    Stretch,
    FlexStart,
    FlexEnd,
    Center,
    Baseline,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum AlignContent {
    #[default]
    Stretch,
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// Four edges: left, right, top, bottom.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct Edges<T> {
    pub left: T,
    pub right: T,
    pub top: T,
    pub bottom: T,
}

impl<T: Copy> Edges<T> {
    pub const fn all(v: T) -> Self {
        Edges {
            left: v,
            right: v,
            top: v,
            bottom: v,
        }
    }
}

/// The style subset. Every engine is configured with `box-sizing: border-box` and `min-size: auto`
/// semantics as the engine defines them; the gate tells us where that differs.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Style {
    pub display: Display,
    pub position: Position,
    pub direction: FlexDirection,
    pub wrap: FlexWrap,
    pub justify: Justify,
    pub align_items: Align,
    pub align_self: Option<Align>,
    pub align_content: AlignContent,
    pub grow: f32,
    pub shrink: f32,
    pub basis: Dim,
    pub width: Dim,
    pub height: Dim,
    pub min_width: Dim,
    pub min_height: Dim,
    /// `Dim::Auto` means no maximum.
    pub max_width: Dim,
    pub max_height: Dim,
    pub margin: Edges<Dim>,
    pub padding: Edges<Lp>,
    /// Border widths, absolute.
    pub border: Edges<f32>,
    /// `row-gap` and `column-gap`, absolute.
    pub row_gap: f32,
    pub column_gap: f32,
    /// Insets for `position: absolute`; `Dim::Auto` is `auto`.
    pub inset: Edges<Dim>,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            display: Display::Flex,
            position: Position::Relative,
            direction: FlexDirection::Row,
            wrap: FlexWrap::NoWrap,
            justify: Justify::FlexStart,
            align_items: Align::Stretch,
            align_self: None,
            align_content: AlignContent::Stretch,
            grow: 0.0,
            shrink: 1.0,
            basis: Dim::Auto,
            width: Dim::Auto,
            height: Dim::Auto,
            min_width: Dim::Auto,
            min_height: Dim::Auto,
            max_width: Dim::Auto,
            max_height: Dim::Auto,
            margin: Edges::all(Dim::Px(0.0)),
            padding: Edges::all(Lp::Px(0.0)),
            border: Edges::all(0.0),
            row_gap: 0.0,
            column_gap: 0.0,
            inset: Edges::all(Dim::Auto),
        }
    }
}

/// One node: a container (children, possibly none) or a text leaf.
#[derive(Clone, PartialEq, Debug)]
pub struct Node {
    pub style: Style,
    /// Child indices into [`Scene::nodes`]; empty for leaves and empty containers.
    pub children: Vec<NodeId>,
    /// Text content measured under the shared model; a node with text has no children.
    pub text: Option<String>,
}

/// Space available to the root on one axis.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Avail {
    Definite(f32),
    MaxContent,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Viewport {
    pub width: Avail,
    pub height: Avail,
}

/// A complete workload input: the tree and the root's available space. Node 0 is the root and
/// nodes are in pre-order.
#[derive(Clone, PartialEq, Debug)]
pub struct Scene {
    pub nodes: Vec<Node>,
    pub viewport: Viewport,
}

impl Scene {
    pub fn root(&self) -> NodeId {
        0
    }
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    /// Indices of the text leaves, in pre-order.
    pub fn text_leaves(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.text.is_some())
            .map(|(i, _)| i)
    }
}

/// A node's border box, absolute within the root's border box, unrounded.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
