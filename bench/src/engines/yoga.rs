//! Adapter for Yoga (C++) through the `yoga` crate 0.5.
//!
//! Every node is created with one `Config` whose point scale factor is set to 0, which turns
//! Yoga's pixel-grid rounding off; the crate does not wrap `YGConfigSetPointScaleFactor`, so it
//! is declared here and called on the config's raw handle. Every scene property is set
//! explicitly because Yoga's defaults are not CSS's (column direction, `flex-shrink: 0`,
//! `align-content: flex-start`).
//!
//! Yoga's root always fills a definite owner size, while the suite's root is `fit-content` in a
//! definite width, content-sized in height, and resolves percentages against the viewport. The
//! adapter reproduces that with a hidden wrapper: a column flex container sized to the viewport
//! (or `auto` on a max-content axis) with `align-items: flex-start`, holding the scene root with
//! its flex factors and `align-self` neutralised.
//!
//! Text leaves own a [`TextCtx`] as their Yoga context; the measure function reads the string
//! and bumps a shared counter. [`set_text`](Engine::set_text) rewrites the string in place and
//! calls `mark_dirty`, which Yoga propagates to the root.

use std::cell::Cell;
use std::ffi::c_void;
use std::rc::Rc;

use yoga::{
    Align, BoxSizing, Config, Context, Direction, Display, Edge, FlexDirection, Justify, MeasureMode, Node, NodeRef,
    PositionType, Size, StyleUnit, Wrap,
};

use crate::engine::Engine;
use crate::measure::{TextAvail, measure_text};
use crate::scene::{self, Avail, Dim, Frame, Lp, NodeId, Scene, Viewport};

unsafe extern "C" {
    fn YGConfigSetPointScaleFactor(config: *mut c_void, pixels_in_point: f32);
    fn YGConfigSetUseWebDefaults(config: *mut c_void, enabled: bool);
    fn YGConfigFree(config: *mut c_void);
}

/// The raw `YGConfigRef` inside a [`Config`], which is `#[repr(C)]` around that one pointer.
fn config_ref(config: &Config) -> *mut c_void {
    // SAFETY: `Config` is `#[repr(C)] struct { inner_config: YGConfigRef }`.
    unsafe { *(config as *const Config as *const *mut c_void) }
}

/// Frees the Yoga config once every node using it is gone (declared last in [`YogaEngine`]).
struct ConfigGuard(*mut c_void);

impl Drop for ConfigGuard {
    fn drop(&mut self) {
        // SAFETY: the config came from `YGConfigNew` and the nodes referring to it were dropped
        // before this field.
        unsafe { YGConfigFree(self.0) }
    }
}

/// A text leaf's Yoga context.
struct TextCtx {
    text: String,
    calls: Rc<Cell<u64>>,
}

extern "C" fn measure(
    node: NodeRef,
    width: f32,
    width_mode: MeasureMode,
    height: f32,
    height_mode: MeasureMode,
) -> Size {
    let ctx = yoga::get_node_ref_context(&node)
        .and_then(|c| c.downcast_ref::<TextCtx>())
        .expect("measured node carries a TextCtx");
    ctx.calls.set(ctx.calls.get() + 1);
    let axis = |v: f32, mode: MeasureMode| match mode {
        MeasureMode::Exactly => (Some(v), TextAvail::Definite(v)),
        MeasureMode::AtMost => (None, TextAvail::Definite(v)),
        MeasureMode::Undefined => (None, TextAvail::MaxContent),
    };
    let (kw, aw) = axis(width, width_mode);
    let (kh, ah) = axis(height, height_mode);
    let [width, height] = measure_text(&ctx.text, [kw, kh], [aw, ah]);
    Size { width, height }
}

pub struct YogaEngine {
    /// The hidden viewport container; dropped first so its children are already detached.
    wrapper: Node,
    /// Yoga node for each scene node.
    nodes: Vec<Node>,
    parents: Vec<usize>,
    /// Whether the node or an ancestor is `display: none`.
    hidden: Vec<bool>,
    viewport: Viewport,
    calls: Rc<Cell<u64>>,
    _config: Config,
    _free: ConfigGuard,
}

fn unit(d: Dim) -> StyleUnit {
    match d {
        Dim::Auto => StyleUnit::Auto,
        Dim::Px(v) => StyleUnit::Point(v.into()),
        Dim::Percent(f) => StyleUnit::Percent((f * 100.0).into()),
    }
}

/// `auto` as "no value", for min/max sizes and insets.
fn unit_or_undefined(d: Dim) -> StyleUnit {
    match d {
        Dim::Auto => StyleUnit::UndefinedValue,
        d => unit(d),
    }
}

fn lp(p: Lp) -> StyleUnit {
    match p {
        Lp::Px(v) => StyleUnit::Point(v.into()),
        Lp::Percent(f) => StyleUnit::Percent((f * 100.0).into()),
    }
}

fn align(a: scene::Align) -> Align {
    match a {
        scene::Align::Stretch => Align::Stretch,
        scene::Align::FlexStart => Align::FlexStart,
        scene::Align::FlexEnd => Align::FlexEnd,
        scene::Align::Center => Align::Center,
        scene::Align::Baseline => Align::Baseline,
    }
}

fn edges<T: Copy>(e: scene::Edges<T>) -> [(Edge, T); 4] {
    [
        (Edge::Left, e.left),
        (Edge::Right, e.right),
        (Edge::Top, e.top),
        (Edge::Bottom, e.bottom),
    ]
}

/// Sets every scene property on `node`.
fn apply(node: &mut Node, s: &scene::Style) {
    node.set_display(match s.display {
        scene::Display::Flex => Display::Flex,
        scene::Display::None => Display::None,
    });
    node.set_box_sizing(BoxSizing::BorderBox);
    node.set_position_type(match s.position {
        scene::Position::Relative => PositionType::Relative,
        scene::Position::Absolute => PositionType::Absolute,
    });
    node.set_flex_direction(match s.direction {
        scene::FlexDirection::Row => FlexDirection::Row,
        scene::FlexDirection::RowReverse => FlexDirection::RowReverse,
        scene::FlexDirection::Column => FlexDirection::Column,
        scene::FlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
    });
    node.set_flex_wrap(match s.wrap {
        scene::FlexWrap::NoWrap => Wrap::NoWrap,
        scene::FlexWrap::Wrap => Wrap::Wrap,
        scene::FlexWrap::WrapReverse => Wrap::WrapReverse,
    });
    node.set_justify_content(match s.justify {
        scene::Justify::FlexStart => Justify::FlexStart,
        scene::Justify::FlexEnd => Justify::FlexEnd,
        scene::Justify::Center => Justify::Center,
        scene::Justify::SpaceBetween => Justify::SpaceBetween,
        scene::Justify::SpaceAround => Justify::SpaceAround,
        scene::Justify::SpaceEvenly => Justify::SpaceEvenly,
    });
    node.set_align_items(align(s.align_items));
    node.set_align_self(s.align_self.map_or(Align::Auto, align));
    node.set_align_content(match s.align_content {
        scene::AlignContent::Stretch => Align::Stretch,
        scene::AlignContent::FlexStart => Align::FlexStart,
        scene::AlignContent::FlexEnd => Align::FlexEnd,
        scene::AlignContent::Center => Align::Center,
        scene::AlignContent::SpaceBetween => Align::SpaceBetween,
        scene::AlignContent::SpaceAround => Align::SpaceAround,
        scene::AlignContent::SpaceEvenly => Align::SpaceEvenly,
    });
    node.set_flex_grow(s.grow);
    node.set_flex_shrink(s.shrink);
    node.set_flex_basis(unit(s.basis));
    node.set_width(unit(s.width));
    node.set_height(unit(s.height));
    // Yoga has no automatic minimum size: `auto` is no minimum.
    node.set_min_width(unit_or_undefined(s.min_width));
    node.set_min_height(unit_or_undefined(s.min_height));
    node.set_max_width(unit_or_undefined(s.max_width));
    node.set_max_height(unit_or_undefined(s.max_height));
    for (edge, m) in edges(s.margin) {
        node.set_margin(edge, unit(m));
    }
    for (edge, p) in edges(s.padding) {
        node.set_padding(edge, lp(p));
    }
    for (edge, b) in edges(s.border) {
        node.set_border(edge, b);
    }
    node.set_row_gap(StyleUnit::Point(s.row_gap.into()));
    node.set_column_gap(StyleUnit::Point(s.column_gap.into()));
    for (edge, i) in edges(s.inset) {
        node.set_position(edge, unit_or_undefined(i));
    }
}

fn axis_size(a: Avail) -> (StyleUnit, f32) {
    match a {
        Avail::Definite(v) => (StyleUnit::Point(v.into()), v),
        Avail::MaxContent => (StyleUnit::Auto, f32::NAN),
    }
}

impl YogaEngine {
    fn size_wrapper(&mut self) {
        self.wrapper.set_width(axis_size(self.viewport.width).0);
        self.wrapper.set_height(axis_size(self.viewport.height).0);
    }
}

impl Engine for YogaEngine {
    const NAME: &'static str = "yoga";

    fn build(scene: &Scene) -> Self {
        let mut config = Config::new();
        let raw = config_ref(&config);
        // SAFETY: `raw` is a live config; both setters only write config fields.
        unsafe {
            YGConfigSetUseWebDefaults(raw, true);
            YGConfigSetPointScaleFactor(raw, 0.0);
        }
        let calls = Rc::new(Cell::new(0));
        let n = scene.len();
        let mut parents = vec![usize::MAX; n];
        for (i, node) in scene.nodes.iter().enumerate() {
            for &c in &node.children {
                parents[c] = i;
            }
        }
        let mut hidden = vec![false; n];
        for i in 0..n {
            let p = parents[i];
            hidden[i] = scene.nodes[i].style.display == scene::Display::None || (p != usize::MAX && hidden[p]);
        }
        let mut nodes: Vec<Node> = scene
            .nodes
            .iter()
            .map(|sn| {
                let mut node = Node::new_with_config(&mut config);
                apply(&mut node, &sn.style);
                if let Some(text) = &sn.text {
                    node.set_context(Some(Context::new(TextCtx {
                        text: text.clone(),
                        calls: calls.clone(),
                    })));
                    node.set_measure_func(Some(measure));
                }
                node
            })
            .collect();
        for (i, sn) in scene.nodes.iter().enumerate() {
            for (k, &c) in sn.children.iter().enumerate() {
                // Children come after their parent in pre-order.
                let (head, tail) = nodes.split_at_mut(c);
                head[i].insert_child(&mut tail[0], k);
            }
        }
        // The scene root is not a flex item of anything; neutralise what the wrapper would apply.
        let root = &mut nodes[0];
        root.set_flex_grow(0.0);
        root.set_flex_shrink(0.0);
        root.set_flex_basis(StyleUnit::Auto);
        root.set_align_self(Align::Auto);
        let mut wrapper = Node::new_with_config(&mut config);
        wrapper.set_flex_direction(FlexDirection::Column);
        wrapper.set_align_items(Align::FlexStart);
        wrapper.set_justify_content(Justify::FlexStart);
        wrapper.set_box_sizing(BoxSizing::BorderBox);
        wrapper.insert_child(&mut nodes[0], 0);
        let mut engine = YogaEngine {
            wrapper,
            nodes,
            parents,
            hidden,
            viewport: scene.viewport,
            calls,
            _config: config,
            _free: ConfigGuard(raw),
        };
        engine.size_wrapper();
        engine
    }

    fn layout(&mut self) {
        let w = axis_size(self.viewport.width).1;
        let h = axis_size(self.viewport.height).1;
        self.wrapper.calculate_layout(w, h, Direction::LTR);
    }

    fn frames(&self, out: &mut Vec<Frame>) {
        out.clear();
        out.reserve(self.nodes.len());
        for (i, node) in self.nodes.iter().enumerate() {
            if self.hidden[i] {
                out.push(Frame::default());
                continue;
            }
            let l = node.get_layout();
            let p = self.parents[i];
            let (x, y) = if p == usize::MAX {
                (0.0, 0.0)
            } else {
                (out[p].x + l.left(), out[p].y + l.top())
            };
            out.push(Frame {
                x,
                y,
                width: l.width(),
                height: l.height(),
            });
        }
    }

    fn set_text(&mut self, node: NodeId, text: &str) {
        let n = &mut self.nodes[node];
        let ctx = n
            .get_context_mut()
            .and_then(|c| c.downcast_mut::<TextCtx>())
            .expect("set_text on a node without text");
        ctx.text.clear();
        ctx.text.push_str(text);
        n.mark_dirty();
    }

    fn resize(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        self.size_wrapper();
    }

    fn measure_calls(&self) -> u64 {
        self.calls.get()
    }

    fn reset_measure_calls(&mut self) {
        self.calls.set(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::cephal::reference::*;

    fn frames_of(e: &YogaEngine) -> Vec<Frame> {
        let mut out = Vec::new();
        e.frames(&mut out);
        out
    }

    #[test]
    fn yoga_matches_taffy() {
        for scene in all() {
            let mut e = YogaEngine::build(&scene);
            e.layout();
            assert_frames_eq(&frames_of(&e), &taffy_frames(&scene));
        }
    }

    #[test]
    fn yoga_wrapping_text_is_100_by_20() {
        let mut e = YogaEngine::build(&text_column(WRAP_TEXT));
        e.layout();
        let f = frames_of(&e)[2];
        assert!(
            (f.width - 100.0).abs() <= TOL && (f.height - 20.0).abs() <= TOL,
            "{f:?}"
        );
    }

    #[test]
    fn yoga_does_not_round() {
        // Three equal items in 100px: 33.333 each, which a scale factor of 1 would snap.
        let mut scene = grow_row();
        scene.nodes[0].style.width = Dim::Px(100.0);
        scene.nodes[0].style.padding = scene::Edges::all(Lp::Px(0.0));
        scene.nodes[0].style.column_gap = 0.0;
        for i in 1..=3 {
            scene.nodes[i].style = scene::Style {
                grow: 1.0,
                ..scene::Style::default()
            };
        }
        let mut e = YogaEngine::build(&scene);
        e.layout();
        let f = frames_of(&e);
        assert!((f[2].x - 100.0 / 3.0).abs() <= 1e-3, "{:?}", f[2]);
        assert_frames_eq(&f, &taffy_frames(&scene));
    }

    #[test]
    fn yoga_set_text_matches_fresh_build() {
        let new_text = "HH\u{200B}HHHHHHH\u{200B}HHH\u{200B}HHHHH";
        let mut e = YogaEngine::build(&text_column(WRAP_TEXT));
        e.layout();
        e.reset_measure_calls();
        e.set_text(2, new_text);
        e.layout();
        assert!(e.measure_calls() > 0, "the edited leaf must be measured again");
        let mut fresh = YogaEngine::build(&text_column(new_text));
        fresh.layout();
        assert_frames_eq(&frames_of(&e), &frames_of(&fresh));
        assert_frames_eq(&frames_of(&e), &taffy_frames(&text_column(new_text)));
    }

    #[test]
    fn yoga_counts_measure_calls() {
        let mut e = YogaEngine::build(&text_column(WRAP_TEXT));
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
    fn yoga_resize_and_fit_content_root() {
        let mut scene = text_column(WRAP_TEXT);
        scene.nodes[0].style.width = Dim::Auto;
        scene.nodes[1].style.width = Dim::Auto;
        let mut e = YogaEngine::build(&scene);
        e.layout();
        scene.viewport = Viewport {
            width: Avail::Definite(90.0),
            height: Avail::MaxContent,
        };
        e.resize(scene.viewport);
        e.layout();
        let mut fresh = YogaEngine::build(&scene);
        fresh.layout();
        assert_frames_eq(&frames_of(&e), &frames_of(&fresh));
        // The auto-width root is fit-content: min(max-content 120, available 90).
        let f = frames_of(&e);
        assert!((f[0].width - 90.0).abs() <= TOL, "{:?}", f[0]);
        // Under a wide viewport it shrinks to its max-content width.
        scene.viewport = Viewport {
            width: Avail::Definite(800.0),
            height: Avail::Definite(600.0),
        };
        e.resize(scene.viewport);
        e.layout();
        let f = frames_of(&e);
        assert!(
            (f[0].width - 120.0).abs() <= TOL && (f[0].height - 40.0).abs() <= TOL,
            "{:?}",
            f[0]
        );
    }
}
