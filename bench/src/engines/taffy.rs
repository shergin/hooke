//! Adapter for Taffy 0.15.1.
//!
//! A `TaffyTree<String>` with rounding disabled; text leaves carry their text as the node
//! context and are measured through `taffy::compute_leaf_layout`, so a leaf's own size, min and
//! max sizes, padding and border apply as Taffy applies them, with the shared Ahem model inside.

use crate::engine::Engine;
use crate::measure::{TextAvail, measure_text};
use crate::scene::{self, Avail, Dim, Frame, Lp, NodeId, Scene, Viewport};
use taffy::prelude::{AvailableSpace, TaffyTree};
use taffy::style as t;

fn dim(d: Dim) -> t::Dimension {
    match d {
        Dim::Auto => t::Dimension::auto(),
        Dim::Px(v) => t::Dimension::length(v),
        Dim::Percent(p) => t::Dimension::percent(p),
    }
}

fn lpa(d: Dim) -> t::LengthPercentageAuto {
    match d {
        Dim::Auto => t::LengthPercentageAuto::auto(),
        Dim::Px(v) => t::LengthPercentageAuto::length(v),
        Dim::Percent(p) => t::LengthPercentageAuto::percent(p),
    }
}

fn lp(l: Lp) -> t::LengthPercentage {
    match l {
        Lp::Px(v) => t::LengthPercentage::length(v),
        Lp::Percent(p) => t::LengthPercentage::percent(p),
    }
}

fn align(a: scene::Align) -> t::AlignItems {
    match a {
        scene::Align::Stretch => t::AlignItems::STRETCH,
        scene::Align::FlexStart => t::AlignItems::FLEX_START,
        scene::Align::FlexEnd => t::AlignItems::FLEX_END,
        scene::Align::Center => t::AlignItems::CENTER,
        scene::Align::Baseline => t::AlignItems::BASELINE,
    }
}

fn rect<T, U>(e: scene::Edges<T>, f: impl Fn(T) -> U) -> taffy::geometry::Rect<U> {
    taffy::geometry::Rect {
        left: f(e.left),
        right: f(e.right),
        top: f(e.top),
        bottom: f(e.bottom),
    }
}

/// Maps the scene's style onto Taffy's, with `box-sizing: border-box`.
pub fn to_taffy_style(s: &scene::Style) -> t::Style {
    t::Style {
        display: match s.display {
            scene::Display::Flex => t::Display::Flex,
            scene::Display::None => t::Display::None,
        },
        box_sizing: t::BoxSizing::BorderBox,
        position: match s.position {
            scene::Position::Relative => t::Position::Relative,
            scene::Position::Absolute => t::Position::Absolute,
        },
        flex_direction: match s.direction {
            scene::FlexDirection::Row => t::FlexDirection::Row,
            scene::FlexDirection::RowReverse => t::FlexDirection::RowReverse,
            scene::FlexDirection::Column => t::FlexDirection::Column,
            scene::FlexDirection::ColumnReverse => t::FlexDirection::ColumnReverse,
        },
        flex_wrap: match s.wrap {
            scene::FlexWrap::NoWrap => t::FlexWrap::NoWrap,
            scene::FlexWrap::Wrap => t::FlexWrap::Wrap,
            scene::FlexWrap::WrapReverse => t::FlexWrap::WrapReverse,
        },
        justify_content: match s.justify {
            scene::Justify::FlexStart => t::JustifyContent::FLEX_START,
            scene::Justify::FlexEnd => t::JustifyContent::FLEX_END,
            scene::Justify::Center => t::JustifyContent::CENTER,
            scene::Justify::SpaceBetween => t::JustifyContent::SPACE_BETWEEN,
            scene::Justify::SpaceAround => t::JustifyContent::SPACE_AROUND,
            scene::Justify::SpaceEvenly => t::JustifyContent::SPACE_EVENLY,
        },
        align_items: align(s.align_items),
        align_self: s.align_self.map(align),
        align_content: match s.align_content {
            scene::AlignContent::Stretch => t::AlignContent::STRETCH,
            scene::AlignContent::FlexStart => t::AlignContent::FLEX_START,
            scene::AlignContent::FlexEnd => t::AlignContent::FLEX_END,
            scene::AlignContent::Center => t::AlignContent::CENTER,
            scene::AlignContent::SpaceBetween => t::AlignContent::SPACE_BETWEEN,
            scene::AlignContent::SpaceAround => t::AlignContent::SPACE_AROUND,
            scene::AlignContent::SpaceEvenly => t::AlignContent::SPACE_EVENLY,
        },
        flex_grow: s.grow,
        flex_shrink: s.shrink,
        flex_basis: dim(s.basis),
        size: taffy::geometry::Size {
            width: dim(s.width),
            height: dim(s.height),
        },
        min_size: taffy::geometry::Size {
            width: lpa(s.min_width),
            height: lpa(s.min_height),
        },
        max_size: taffy::geometry::Size {
            width: lpa(s.max_width),
            height: lpa(s.max_height),
        },
        margin: rect(s.margin, lpa),
        padding: rect(s.padding, lp),
        border: rect(s.border, t::LengthPercentage::length),
        gap: taffy::geometry::Size {
            width: t::LengthPercentage::length(s.column_gap),
            height: t::LengthPercentage::length(s.row_gap),
        },
        inset: rect(s.inset, lpa),
        ..Default::default()
    }
}

fn available(v: Viewport) -> taffy::geometry::Size<AvailableSpace> {
    let a = |a: Avail| match a {
        Avail::Definite(v) => AvailableSpace::Definite(v),
        Avail::MaxContent => AvailableSpace::MaxContent,
    };
    taffy::geometry::Size {
        width: a(v.width),
        height: a(v.height),
    }
}

fn text_avail(a: AvailableSpace) -> TextAvail {
    match a {
        AvailableSpace::Definite(v) => TextAvail::Definite(v),
        AvailableSpace::MinContent => TextAvail::MinContent,
        AvailableSpace::MaxContent => TextAvail::MaxContent,
    }
}

#[derive(Clone)]
pub struct TaffyEngine {
    tree: TaffyTree<String>,
    /// Taffy node of each scene node.
    ids: Vec<taffy::NodeId>,
    /// Whether each scene node or an ancestor is `display: none`.
    hidden: Vec<bool>,
    available: taffy::geometry::Size<AvailableSpace>,
    calls: u64,
}

impl TaffyEngine {
    fn compute(tree: &mut TaffyTree<String>, root: taffy::NodeId, space: taffy::geometry::Size<AvailableSpace>) -> u64 {
        let mut calls = 0u64;
        tree.compute_layout_with_measure(root, space, |input, _id, ctx, style| {
            taffy::compute_leaf_layout(
                input,
                style,
                |_, _| 0.0,
                |known, avail| match ctx.as_deref() {
                    Some(text) => {
                        calls += 1;
                        let [w, h] = measure_text(
                            text,
                            [known.width, known.height],
                            [text_avail(avail.width), text_avail(avail.height)],
                        );
                        taffy::geometry::Size { width: w, height: h }
                    }
                    None => taffy::geometry::Size::ZERO,
                },
            )
        })
        .expect("taffy layout");
        calls
    }

    /// Marks every node dirty, so the next [`layout`](Engine::layout) is a cold pass over the
    /// existing tree. This is how arewelayoutyet times Taffy's cold layout.
    pub fn invalidate_all(&mut self) {
        for &id in &self.ids {
            self.tree.mark_dirty(id).expect("node exists");
        }
    }

    /// Taffy's counterpart of hooke's fork: deep-clones the tree, edits `node`'s text and lays
    /// the clone out, leaving `self` untouched.
    pub fn clone_edit_layout(&self, node: NodeId, text: &str) -> TaffyTree<String> {
        let mut tree = self.tree.clone();
        let id = self.ids[node];
        *tree.get_node_context_mut(id).expect("text leaf") = text.to_string();
        tree.mark_dirty(id).expect("node exists");
        Self::compute(&mut tree, self.ids[0], self.available);
        tree
    }

    /// One [`clone_edit_layout`](Self::clone_edit_layout) per entry of `edits`, the
    /// counterpart of hooke's `candidates`. Sequential: `TaffyTree` 0.15 is neither `Send` nor
    /// `Sync` (its compact lengths hold a raw pointer), so candidates cannot go to other threads.
    pub fn candidates(&self, edits: &[(NodeId, String)]) -> Vec<TaffyTree<String>> {
        edits
            .iter()
            .map(|(node, text)| self.clone_edit_layout(*node, text))
            .collect()
    }

    /// Frames of a tree cloned from this one, in scene node order.
    pub fn frames_of(&self, tree: &TaffyTree<String>, out: &mut Vec<Frame>) {
        out.clear();
        out.resize(self.ids.len(), Frame::default());
        let root_loc = tree.layout(self.ids[0]).expect("root").location;
        // Scene nodes are in pre-order, so a parent's frame is written before its children's.
        // Walk the tree to know each node's parent.
        fn walk(
            tree: &TaffyTree<String>,
            id: taffy::NodeId,
            x: f32,
            y: f32,
            next: &mut usize,
            hidden: &[bool],
            out: &mut [Frame],
        ) {
            let i = *next;
            *next += 1;
            let l = tree.layout(id).expect("node");
            let (ax, ay) = (x + l.location.x, y + l.location.y);
            out[i] = if hidden[i] {
                Frame::default()
            } else {
                Frame {
                    x: ax,
                    y: ay,
                    width: l.size.width,
                    height: l.size.height,
                }
            };
            for c in tree.children(id).expect("node") {
                walk(tree, c, ax, ay, next, hidden, out);
            }
        }
        let mut next = 0;
        walk(
            tree,
            self.ids[0],
            -root_loc.x,
            -root_loc.y,
            &mut next,
            &self.hidden,
            out,
        );
    }
}

impl Engine for TaffyEngine {
    const NAME: &'static str = "taffy";

    fn build(scene: &Scene) -> Self {
        let mut tree: TaffyTree<String> = TaffyTree::with_capacity(scene.len());
        tree.disable_rounding();
        let mut ids = vec![taffy::NodeId::from(0u64); scene.len()];
        // Children before parents: walk the pre-order backwards.
        for i in (0..scene.len()).rev() {
            let n = &scene.nodes[i];
            let style = to_taffy_style(&n.style);
            ids[i] = match &n.text {
                Some(text) => tree.new_leaf_with_context(style, text.clone()).expect("leaf"),
                None => {
                    let kids: Vec<taffy::NodeId> = n.children.iter().map(|&c| ids[c]).collect();
                    tree.new_with_children(style, &kids).expect("container")
                }
            };
        }
        let mut hidden = vec![false; scene.len()];
        for (i, n) in scene.nodes.iter().enumerate() {
            let h = hidden[i] || n.style.display == scene::Display::None;
            hidden[i] = h;
            for &c in &n.children {
                hidden[c] = h;
            }
        }
        TaffyEngine {
            tree,
            ids,
            hidden,
            available: available(scene.viewport),
            calls: 0,
        }
    }

    fn layout(&mut self) {
        self.calls += Self::compute(&mut self.tree, self.ids[0], self.available);
    }

    fn frames(&self, out: &mut Vec<Frame>) {
        self.frames_of(&self.tree, out);
    }

    fn set_text(&mut self, node: NodeId, text: &str) {
        let id = self.ids[node];
        let ctx = self.tree.get_node_context_mut(id).expect("text leaf");
        ctx.clear();
        ctx.push_str(text);
        self.tree.mark_dirty(id).expect("node exists");
    }

    fn resize(&mut self, viewport: Viewport) {
        self.available = available(viewport);
    }

    fn measure_calls(&self) -> u64 {
        self.calls
    }

    fn reset_measure_calls(&mut self) {
        self.calls = 0;
    }
}
