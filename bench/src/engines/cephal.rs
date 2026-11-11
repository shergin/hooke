//! Adapter for Cephal 0.2 (flexbox only).
//!
//! Cephal keeps a retained tree with per-node caches and a dirty queue, so the adapter builds the
//! tree once and every later operation is a native edit: [`set_text`](Engine::set_text) rewrites
//! the leaf's context in place and calls `Tree::invalidate`, and [`resize`](Engine::resize) only
//! stores the new available space, which `compute_layout_with_measure` compares with the last
//! one. Pixel snapping is disabled with `Tree::disable_rounding`, and frames are read from
//! `Tree::unrounded_layout` regardless. Text leaves carry their string as the node context; the
//! measure closure routes through [`measure_text`].

use cephal::style::{
    AlignContent, AlignItems, BoxSizing, Dimension, Display, FlexDirection, FlexWrap, LengthPercentage,
    LengthPercentageAuto, Position,
};
use cephal::{AvailableSpace, MeasureInput, MeasureOutput, Rect, Size, Style, Tree};

use crate::engine::Engine;
use crate::measure::{TextAvail, measure_text};
use crate::scene::{self, Avail, Dim, Frame, Lp, NodeId, Scene, Viewport};

pub struct CephalEngine {
    tree: Tree<String>,
    /// Cephal node for each scene node.
    nodes: Vec<cephal::NodeId>,
    /// Scene parent of each node (`usize::MAX` for the root).
    parents: Vec<usize>,
    /// Whether the node or an ancestor is `display: none`.
    hidden: Vec<bool>,
    viewport: Viewport,
    measure_calls: u64,
}

fn dimension(d: Dim) -> Dimension {
    match d {
        Dim::Auto => Dimension::AUTO,
        Dim::Px(v) => Dimension::length(v),
        Dim::Percent(f) => Dimension::percent(f),
    }
}

fn lpa(d: Dim) -> LengthPercentageAuto {
    match d {
        Dim::Auto => LengthPercentageAuto::AUTO,
        Dim::Px(v) => LengthPercentageAuto::length(v),
        Dim::Percent(f) => LengthPercentageAuto::percent(f),
    }
}

fn lp(p: Lp) -> LengthPercentage {
    match p {
        Lp::Px(v) => LengthPercentage::length(v),
        Lp::Percent(f) => LengthPercentage::percent(f),
    }
}

fn rect<T: Copy, U>(e: scene::Edges<T>, f: impl Fn(T) -> U) -> Rect<U> {
    Rect {
        left: f(e.left),
        right: f(e.right),
        top: f(e.top),
        bottom: f(e.bottom),
    }
}

fn align(a: scene::Align) -> AlignItems {
    match a {
        scene::Align::Stretch => AlignItems::STRETCH,
        scene::Align::FlexStart => AlignItems::FLEX_START,
        scene::Align::FlexEnd => AlignItems::FLEX_END,
        scene::Align::Center => AlignItems::CENTER,
        scene::Align::Baseline => AlignItems::BASELINE,
    }
}

fn justify(j: scene::Justify) -> AlignContent {
    match j {
        scene::Justify::FlexStart => AlignContent::FLEX_START,
        scene::Justify::FlexEnd => AlignContent::FLEX_END,
        scene::Justify::Center => AlignContent::CENTER,
        scene::Justify::SpaceBetween => AlignContent::SPACE_BETWEEN,
        scene::Justify::SpaceAround => AlignContent::SPACE_AROUND,
        scene::Justify::SpaceEvenly => AlignContent::SPACE_EVENLY,
    }
}

fn align_content(a: scene::AlignContent) -> AlignContent {
    match a {
        scene::AlignContent::Stretch => AlignContent::STRETCH,
        scene::AlignContent::FlexStart => AlignContent::FLEX_START,
        scene::AlignContent::FlexEnd => AlignContent::FLEX_END,
        scene::AlignContent::Center => AlignContent::CENTER,
        scene::AlignContent::SpaceBetween => AlignContent::SPACE_BETWEEN,
        scene::AlignContent::SpaceAround => AlignContent::SPACE_AROUND,
        scene::AlignContent::SpaceEvenly => AlignContent::SPACE_EVENLY,
    }
}

/// Maps every field of the scene style; the rest stays at `Style::DEFAULT` (which is block
/// display, static position and border-box sizing).
fn style(s: &scene::Style) -> Style {
    let mut out = Style::DEFAULT;
    out.display = match s.display {
        scene::Display::Flex => Display::Flex,
        scene::Display::None => Display::None,
    };
    out.box_sizing = BoxSizing::BorderBox;
    out.position = match s.position {
        scene::Position::Relative => Position::Relative,
        scene::Position::Absolute => Position::Absolute,
    };
    out.flex_direction = match s.direction {
        scene::FlexDirection::Row => FlexDirection::Row,
        scene::FlexDirection::RowReverse => FlexDirection::RowReverse,
        scene::FlexDirection::Column => FlexDirection::Column,
        scene::FlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
    };
    out.flex_wrap = match s.wrap {
        scene::FlexWrap::NoWrap => FlexWrap::NoWrap,
        scene::FlexWrap::Wrap => FlexWrap::Wrap,
        scene::FlexWrap::WrapReverse => FlexWrap::WrapReverse,
    };
    out.justify_content = Some(justify(s.justify));
    out.align_items = Some(align(s.align_items));
    out.align_self = s.align_self.map(align);
    out.align_content = Some(align_content(s.align_content));
    out.flex_grow = s.grow;
    out.flex_shrink = s.shrink;
    out.flex_basis = dimension(s.basis);
    out.size = Size {
        width: dimension(s.width),
        height: dimension(s.height),
    };
    // `auto` minimum is the automatic minimum size; `auto` maximum resolves to none.
    out.min_size = Size {
        width: lpa(s.min_width),
        height: lpa(s.min_height),
    };
    out.max_size = Size {
        width: lpa(s.max_width),
        height: lpa(s.max_height),
    };
    out.margin = rect(s.margin, lpa);
    out.padding = rect(s.padding, lp);
    out.border = rect(s.border, LengthPercentage::length);
    out.gap = Size {
        width: LengthPercentage::length(s.column_gap),
        height: LengthPercentage::length(s.row_gap),
    };
    out.inset = rect(s.inset, lpa);
    out
}

fn available(a: Avail) -> AvailableSpace {
    match a {
        Avail::Definite(v) => AvailableSpace::Definite(v),
        Avail::MaxContent => AvailableSpace::MaxContent,
    }
}

fn text_avail(a: AvailableSpace) -> TextAvail {
    match a {
        AvailableSpace::Definite(v) => TextAvail::Definite(v),
        AvailableSpace::MinContent => TextAvail::MinContent,
        AvailableSpace::MaxContent => TextAvail::MaxContent,
    }
}

impl Engine for CephalEngine {
    const NAME: &'static str = "cephal";

    fn build(scene: &Scene) -> Self {
        let n = scene.len();
        let mut tree: Tree<String> = Tree::with_capacity(n);
        tree.disable_rounding();
        // The text model never reads the available height.
        tree.set_measure_ignores_available_height(true);
        let mut parents = vec![usize::MAX; n];
        let mut hidden = vec![false; n];
        for (i, node) in scene.nodes.iter().enumerate() {
            for &c in &node.children {
                parents[c] = i;
            }
        }
        // Pre-order: parents precede children.
        for i in 0..n {
            let p = parents[i];
            hidden[i] = scene.nodes[i].style.display == scene::Display::None || (p != usize::MAX && hidden[p]);
        }
        let mut nodes: Vec<Option<cephal::NodeId>> = vec![None; n];
        // Reverse pre-order creates every child before its parent.
        for i in (0..n).rev() {
            let node = &scene.nodes[i];
            let s = style(&node.style);
            let id = match &node.text {
                Some(text) => tree.new_leaf_with_context(s, text.clone()),
                None => {
                    let children: Vec<_> = node.children.iter().map(|&c| nodes[c].expect("child built")).collect();
                    tree.new_with_children(s, &children)
                }
            };
            nodes[i] = Some(id);
        }
        CephalEngine {
            tree,
            nodes: nodes.into_iter().map(|n| n.expect("node built")).collect(),
            parents,
            hidden,
            viewport: scene.viewport,
            measure_calls: 0,
        }
    }

    fn layout(&mut self) {
        let root = self.nodes[0];
        let space = Size {
            width: available(self.viewport.width),
            height: available(self.viewport.height),
        };
        let calls = &mut self.measure_calls;
        self.tree
            .compute_layout_with_measure(root, space, |text: &mut String, input: MeasureInput, _: &Style| {
                *calls += 1;
                let known = [input.known_dimensions.width, input.known_dimensions.height];
                let avail = [
                    text_avail(input.available_space.width),
                    text_avail(input.available_space.height),
                ];
                let [width, height] = measure_text(text, known, avail);
                MeasureOutput::from_size(Size { width, height })
            });
    }

    fn frames(&self, out: &mut Vec<Frame>) {
        out.clear();
        out.reserve(self.nodes.len());
        for (i, &id) in self.nodes.iter().enumerate() {
            if self.hidden[i] {
                out.push(Frame::default());
                continue;
            }
            let l = self.tree.unrounded_layout(id);
            let p = self.parents[i];
            // The root is the origin; every other node is offset from its parent's border box.
            let (x, y) = if p == usize::MAX {
                (0.0, 0.0)
            } else {
                (out[p].x + l.location.x, out[p].y + l.location.y)
            };
            out.push(Frame {
                x,
                y,
                width: l.size.width,
                height: l.size.height,
            });
        }
    }

    fn set_text(&mut self, node: NodeId, text: &str) {
        let id = self.nodes[node];
        let ctx = self.tree.context_mut(id).expect("set_text on a node without text");
        ctx.clear();
        ctx.push_str(text);
        self.tree.invalidate(id);
    }

    fn resize(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    fn measure_calls(&self) -> u64 {
        self.measure_calls
    }

    fn reset_measure_calls(&mut self) {
        self.measure_calls = 0;
    }
}

/// Test support shared with the Yoga adapter's tests: small scenes and a Taffy reference
/// mapped by hand.
#[cfg(test)]
pub(crate) mod reference {
    use crate::measure::{TextAvail, measure_text};
    use crate::scene::{self, Avail, Dim, Edges, Frame, Lp, Node, Scene, Style, Viewport};
    use taffy::prelude::*;

    pub const TOL: f32 = 0.01;

    fn dim(d: Dim) -> Dimension {
        match d {
            Dim::Auto => Dimension::auto(),
            Dim::Px(v) => Dimension::length(v),
            Dim::Percent(f) => Dimension::percent(f),
        }
    }
    fn lpa(d: Dim) -> LengthPercentageAuto {
        match d {
            Dim::Auto => LengthPercentageAuto::auto(),
            Dim::Px(v) => LengthPercentageAuto::length(v),
            Dim::Percent(f) => LengthPercentageAuto::percent(f),
        }
    }
    fn lp(p: Lp) -> LengthPercentage {
        match p {
            Lp::Px(v) => LengthPercentage::length(v),
            Lp::Percent(f) => LengthPercentage::percent(f),
        }
    }
    fn al(a: scene::Align) -> AlignItems {
        match a {
            scene::Align::Stretch => AlignItems::STRETCH,
            scene::Align::FlexStart => AlignItems::FLEX_START,
            scene::Align::FlexEnd => AlignItems::FLEX_END,
            scene::Align::Center => AlignItems::CENTER,
            scene::Align::Baseline => AlignItems::BASELINE,
        }
    }

    /// Only the fields the test scenes use are mapped.
    fn style(s: &Style) -> taffy::Style {
        taffy::Style {
            display: match s.display {
                scene::Display::Flex => Display::Flex,
                scene::Display::None => Display::None,
            },
            box_sizing: BoxSizing::BorderBox,
            position: match s.position {
                scene::Position::Relative => Position::Relative,
                scene::Position::Absolute => Position::Absolute,
            },
            flex_direction: match s.direction {
                scene::FlexDirection::Row => FlexDirection::Row,
                scene::FlexDirection::RowReverse => FlexDirection::RowReverse,
                scene::FlexDirection::Column => FlexDirection::Column,
                scene::FlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
            },
            flex_wrap: match s.wrap {
                scene::FlexWrap::NoWrap => FlexWrap::NoWrap,
                scene::FlexWrap::Wrap => FlexWrap::Wrap,
                scene::FlexWrap::WrapReverse => FlexWrap::WrapReverse,
            },
            justify_content: match s.justify {
                scene::Justify::FlexStart => JustifyContent::FLEX_START,
                scene::Justify::FlexEnd => JustifyContent::FLEX_END,
                scene::Justify::Center => JustifyContent::CENTER,
                scene::Justify::SpaceBetween => JustifyContent::SPACE_BETWEEN,
                scene::Justify::SpaceAround => JustifyContent::SPACE_AROUND,
                scene::Justify::SpaceEvenly => JustifyContent::SPACE_EVENLY,
            },
            align_items: al(s.align_items),
            align_self: s.align_self.map(al),
            flex_grow: s.grow,
            flex_shrink: s.shrink,
            flex_basis: dim(s.basis),
            size: taffy::Size {
                width: dim(s.width),
                height: dim(s.height),
            },
            min_size: taffy::Size {
                width: lpa(s.min_width),
                height: lpa(s.min_height),
            },
            max_size: taffy::Size {
                width: lpa(s.max_width),
                height: lpa(s.max_height),
            },
            margin: taffy::Rect {
                left: lpa(s.margin.left),
                right: lpa(s.margin.right),
                top: lpa(s.margin.top),
                bottom: lpa(s.margin.bottom),
            },
            padding: taffy::Rect {
                left: lp(s.padding.left),
                right: lp(s.padding.right),
                top: lp(s.padding.top),
                bottom: lp(s.padding.bottom),
            },
            border: taffy::Rect {
                left: LengthPercentage::length(s.border.left),
                right: LengthPercentage::length(s.border.right),
                top: LengthPercentage::length(s.border.top),
                bottom: LengthPercentage::length(s.border.bottom),
            },
            gap: taffy::Size {
                width: LengthPercentage::length(s.column_gap),
                height: LengthPercentage::length(s.row_gap),
            },
            inset: taffy::Rect {
                left: lpa(s.inset.left),
                right: lpa(s.inset.right),
                top: lpa(s.inset.top),
                bottom: lpa(s.inset.bottom),
            },
            ..Default::default()
        }
    }

    fn avail(a: Avail) -> AvailableSpace {
        match a {
            Avail::Definite(v) => AvailableSpace::Definite(v),
            Avail::MaxContent => AvailableSpace::MaxContent,
        }
    }

    /// Lays `scene` out with Taffy directly, unrounded; root at the origin.
    pub fn taffy_frames(scene: &Scene) -> Vec<Frame> {
        let mut tree: TaffyTree<String> = TaffyTree::new();
        tree.disable_rounding();
        let n = scene.len();
        let mut ids = vec![None; n];
        for i in (0..n).rev() {
            let node = &scene.nodes[i];
            let id = match &node.text {
                Some(t) => tree.new_leaf_with_context(style(&node.style), t.clone()).unwrap(),
                None => {
                    let c: Vec<_> = node.children.iter().map(|&c| ids[c].unwrap()).collect();
                    tree.new_with_children(style(&node.style), &c).unwrap()
                }
            };
            ids[i] = Some(id);
        }
        let root = ids[0].unwrap();
        let space = taffy::Size {
            width: avail(scene.viewport.width),
            height: avail(scene.viewport.height),
        };
        tree.compute_layout_with_measure(root, space, |input, _, ctx, style| {
            let text = ctx.map(|t| t.as_str()).unwrap_or("");
            taffy::compute_leaf_layout(
                input,
                style,
                |_, _| 0.0,
                |known, available| {
                    let a = |s: AvailableSpace| match s {
                        AvailableSpace::Definite(v) => TextAvail::Definite(v),
                        AvailableSpace::MinContent => TextAvail::MinContent,
                        AvailableSpace::MaxContent => TextAvail::MaxContent,
                    };
                    let [width, height] = measure_text(
                        text,
                        [known.width, known.height],
                        [a(available.width), a(available.height)],
                    );
                    taffy::Size { width, height }
                },
            )
        })
        .unwrap();
        let mut parents = vec![usize::MAX; n];
        for (i, node) in scene.nodes.iter().enumerate() {
            for &c in &node.children {
                parents[c] = i;
            }
        }
        let mut out: Vec<Frame> = Vec::with_capacity(n);
        for i in 0..n {
            let l = tree.layout(ids[i].unwrap()).unwrap();
            let p = parents[i];
            let (x, y) = if p == usize::MAX {
                (0.0, 0.0)
            } else {
                (out[p].x + l.location.x, out[p].y + l.location.y)
            };
            out.push(Frame {
                x,
                y,
                width: l.size.width,
                height: l.size.height,
            });
        }
        out
    }

    pub fn assert_frames_eq(got: &[Frame], want: &[Frame]) {
        assert_eq!(got.len(), want.len());
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            let close = (g.x - w.x).abs() <= TOL
                && (g.y - w.y).abs() <= TOL
                && (g.width - w.width).abs() <= TOL
                && (g.height - w.height).abs() <= TOL;
            assert!(close, "node {i}: got {g:?}, want {w:?}");
        }
    }

    fn container(style: Style, children: Vec<usize>) -> Node {
        Node {
            style,
            children,
            text: None,
        }
    }
    fn text(style: Style, t: &str) -> Node {
        Node {
            style,
            children: Vec::new(),
            text: Some(t.to_owned()),
        }
    }
    fn viewport(w: f32, h: f32) -> Viewport {
        Viewport {
            width: Avail::Definite(w),
            height: Avail::Definite(h),
        }
    }

    /// A 300x100 row with 10px padding and three items growing 1, 2 and 1, the middle one with
    /// a 20px basis.
    pub fn grow_row() -> Scene {
        let root = Style {
            width: Dim::Px(300.0),
            height: Dim::Px(100.0),
            padding: Edges::all(Lp::Px(10.0)),
            column_gap: 5.0,
            ..Style::default()
        };
        let item = |grow: f32, basis: Dim| Style {
            grow,
            basis,
            ..Style::default()
        };
        Scene {
            nodes: vec![
                container(root, vec![1, 2, 3]),
                container(item(1.0, Dim::Auto), vec![]),
                container(
                    Style {
                        padding: Edges::all(Lp::Percent(0.1)),
                        ..item(2.0, Dim::Px(20.0))
                    },
                    vec![],
                ),
                container(
                    Style {
                        margin: Edges {
                            left: Dim::Px(4.0),
                            ..Edges::all(Dim::Px(0.0))
                        },
                        ..item(1.0, Dim::Auto)
                    },
                    vec![],
                ),
            ],
            viewport: viewport(800.0, 600.0),
        }
    }

    pub const WRAP_TEXT: &str = "HHHH\u{200B}HHHH\u{200B}HHHH";

    /// A column with a 100px-wide box holding a wrapping text leaf, followed by a sized box.
    pub fn text_column(t: &str) -> Scene {
        let root = Style {
            direction: scene::FlexDirection::Column,
            width: Dim::Px(400.0),
            ..Style::default()
        };
        let boxed = Style {
            width: Dim::Px(100.0),
            align_items: scene::Align::FlexStart,
            ..Style::default()
        };
        Scene {
            nodes: vec![
                container(root, vec![1, 3]),
                container(boxed, vec![2]),
                text(Style::default(), t),
                container(
                    Style {
                        height: Dim::Px(30.0),
                        ..Style::default()
                    },
                    vec![],
                ),
            ],
            viewport: viewport(800.0, 600.0),
        }
    }

    /// A 200x200 box with border, an in-flow child and two absolutely positioned children with
    /// px and percent insets.
    pub fn absolute() -> Scene {
        let root = Style {
            width: Dim::Px(200.0),
            height: Dim::Px(200.0),
            border: Edges::all(2.0),
            padding: Edges::all(Lp::Px(8.0)),
            ..Style::default()
        };
        let abs1 = Style {
            position: scene::Position::Absolute,
            inset: Edges {
                left: Dim::Px(10.0),
                top: Dim::Px(20.0),
                right: Dim::Auto,
                bottom: Dim::Auto,
            },
            width: Dim::Px(50.0),
            height: Dim::Px(40.0),
            ..Style::default()
        };
        let abs2 = Style {
            position: scene::Position::Absolute,
            inset: Edges {
                left: Dim::Percent(0.1),
                right: Dim::Px(30.0),
                top: Dim::Auto,
                bottom: Dim::Percent(0.25),
            },
            height: Dim::Px(10.0),
            ..Style::default()
        };
        Scene {
            nodes: vec![
                container(root, vec![1, 2, 3]),
                container(
                    Style {
                        width: Dim::Px(20.0),
                        ..Style::default()
                    },
                    vec![],
                ),
                container(abs1, vec![]),
                container(abs2, vec![]),
            ],
            viewport: viewport(800.0, 600.0),
        }
    }

    pub fn all() -> Vec<Scene> {
        vec![grow_row(), text_column(WRAP_TEXT), absolute()]
    }
}

#[cfg(test)]
mod tests {
    use super::reference::*;
    use super::*;

    fn frames_of(e: &CephalEngine) -> Vec<Frame> {
        let mut out = Vec::new();
        e.frames(&mut out);
        out
    }

    #[test]
    fn cephal_matches_taffy() {
        for scene in all() {
            let mut e = CephalEngine::build(&scene);
            e.layout();
            assert_frames_eq(&frames_of(&e), &taffy_frames(&scene));
        }
    }

    #[test]
    fn cephal_wrapping_text_is_100_by_20() {
        let scene = text_column(WRAP_TEXT);
        let mut e = CephalEngine::build(&scene);
        e.layout();
        let f = frames_of(&e)[2];
        assert!(
            (f.width - 100.0).abs() <= TOL && (f.height - 20.0).abs() <= TOL,
            "{f:?}"
        );
    }

    #[test]
    fn cephal_set_text_matches_fresh_build() {
        let new_text = "HH\u{200B}HHHHHHH\u{200B}HHH\u{200B}HHHHH";
        let mut e = CephalEngine::build(&text_column(WRAP_TEXT));
        e.layout();
        e.reset_measure_calls();
        e.set_text(2, new_text);
        e.layout();
        assert!(e.measure_calls() > 0, "the edited leaf must be measured again");
        let mut fresh = CephalEngine::build(&text_column(new_text));
        fresh.layout();
        assert_frames_eq(&frames_of(&e), &frames_of(&fresh));
        assert_frames_eq(&frames_of(&e), &taffy_frames(&text_column(new_text)));
    }

    #[test]
    fn cephal_counts_measure_calls() {
        let mut e = CephalEngine::build(&text_column(WRAP_TEXT));
        assert_eq!(e.measure_calls(), 0);
        e.layout();
        assert!(e.measure_calls() > 0);
        e.reset_measure_calls();
        assert_eq!(e.measure_calls(), 0);
        // A clean relayout under the same viewport measures nothing.
        e.layout();
        assert_eq!(e.measure_calls(), 0);
    }

    #[test]
    fn cephal_resize_relays_out() {
        let mut scene = text_column(WRAP_TEXT);
        scene.nodes[0].style.width = Dim::Auto;
        scene.nodes[1].style.width = Dim::Auto;
        let mut e = CephalEngine::build(&scene);
        e.layout();
        scene.viewport = Viewport {
            width: Avail::Definite(90.0),
            height: Avail::MaxContent,
        };
        e.resize(scene.viewport);
        e.layout();
        let mut fresh = CephalEngine::build(&scene);
        fresh.layout();
        assert_frames_eq(&frames_of(&e), &frames_of(&fresh));
        // The auto-width root is fit-content: min(max-content 120, available 90). (Taffy 0.15
        // reports 120 here, so this case is not compared with it.)
        assert!((frames_of(&e)[0].width - 90.0).abs() <= TOL);
    }
}
