//! Adapter for hooke.
//!
//! The host node is a minimal Fabric-like shadow node: immutable, versioned props behind a
//! handle, the text of a measured leaf, the child list and the layout state. `Clone` is shallow,
//! as the host contract requires.
//!
//! The adapter behaves like a persistent host. It keeps the committed revision (`root`) and,
//! during an edit, the revision before it, so [`Engine::set_text`] really path-copies from the
//! root to the leaf instead of mutating unique nodes in place; [`Engine::layout`] lays out the
//! candidate and then commits it, dropping the previous revision. One [`Scratch`] is reused
//! across passes.

use crate::engine::Engine;
use crate::measure::{TextAvail, measure_text};
use crate::scene::{self, Avail, Dim, Frame, Lp, NodeId, Scene, Viewport};
use hooke::{
    Available, Children, Handle, LayoutCx, LayoutNode, LayoutState, Measure, MeasureInput, Measured, Opt, Parts,
    PartsMut, RootConstraints, Scratch, SharedMeasureCache, Size, Style,
};
use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A fresh, non-zero id for props versions and text content ids.
fn fresh_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Immutable, versioned props.
pub struct Props {
    pub style: Style,
    pub version: u64,
}

/// The host node. `Clone` is shallow: handle bumps plus the inline layout state.
#[derive(Clone)]
pub struct HNode {
    pub props: Handle<Props>,
    /// The text of a measured leaf; `None` for containers.
    pub text: Option<Arc<str>>,
    pub children: Children<HNode>,
    pub layout: LayoutState,
}

impl LayoutNode for HNode {
    type Scalar = f32;
    fn parts(&self) -> Parts<'_, Self> {
        Parts {
            style: &self.props.style,
            state: &self.layout,
            children: &self.children,
        }
    }
    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut {
            style: &self.props.style,
            state: &mut self.layout,
            children: &mut self.children,
        }
    }
    fn style_key(&self) -> u64 {
        self.props.version
    }
}

/// The measurer: the shared Ahem model, counting calls.
struct Ahem<'a> {
    calls: &'a Cell<u64>,
}

fn text_avail(a: Available<f32>) -> TextAvail {
    match a {
        Available::Definite(v) => TextAvail::Definite(v),
        Available::MinContent => TextAvail::MinContent,
        Available::MaxContent => TextAvail::MaxContent,
    }
}

fn measure_node(node: &HNode, input: MeasureInput<f32>) -> Measured<f32> {
    let text = node.text.as_deref().unwrap_or("");
    let [w, h] = measure_text(
        text,
        [input.known.width.get(), input.known.height.get()],
        [text_avail(input.available.width), text_avail(input.available.height)],
    );
    Measured::size(w, h)
}

impl Measure<HNode> for Ahem<'_> {
    fn measure(&mut self, node: &mut HNode, input: MeasureInput<f32>, _cx: &mut LayoutCx<'_, f32>) -> Measured<f32> {
        self.calls.set(self.calls.get() + 1);
        measure_node(node, input)
    }
}

/// A thread-safe measurer for the candidates workload.
struct AhemAtomic<'a> {
    calls: &'a AtomicU64,
}

impl Measure<HNode> for AhemAtomic<'_> {
    fn measure(&mut self, node: &mut HNode, input: MeasureInput<f32>, _cx: &mut LayoutCx<'_, f32>) -> Measured<f32> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        measure_node(node, input)
    }
}

fn len(d: Dim) -> hooke::Length<f32> {
    match d {
        Dim::Auto => hooke::Length::Auto,
        Dim::Px(v) => hooke::Length::Px(v),
        Dim::Percent(p) => hooke::Length::Percent(p),
    }
}

fn max_len(d: Dim) -> hooke::Length<f32> {
    match d {
        Dim::Auto => hooke::Length::None,
        other => len(other),
    }
}

fn lp(l: Lp) -> hooke::Length<f32> {
    match l {
        Lp::Px(v) => hooke::Length::Px(v),
        Lp::Percent(p) => hooke::Length::Percent(p),
    }
}

fn align(a: scene::Align) -> hooke::AlignItems {
    match a {
        scene::Align::Stretch => hooke::AlignItems::Stretch,
        scene::Align::FlexStart => hooke::AlignItems::FlexStart,
        scene::Align::FlexEnd => hooke::AlignItems::FlexEnd,
        scene::Align::Center => hooke::AlignItems::Center,
        scene::Align::Baseline => hooke::AlignItems::Baseline,
    }
}

/// Maps the scene's style onto hooke's, with `box-sizing: border-box` and the scene's
/// `position` set explicitly (`Style::web()` is content-box and static).
pub fn to_hooke_style(s: &scene::Style) -> Style {
    use hooke::{
        AlignContent as AC, Display as D, FlexDirection as FD, FlexWrap as FW, JustifyContent as J, Position as P,
    };
    let e = |e: scene::Edges<Dim>| hooke::Edges {
        left: len(e.left),
        right: len(e.right),
        top: len(e.top),
        bottom: len(e.bottom),
    };
    Style::web()
        .with_box_sizing(hooke::BoxSizing::BorderBox)
        .with_display(match s.display {
            scene::Display::Flex => D::Flex,
            scene::Display::None => D::None,
        })
        .with_position(match s.position {
            scene::Position::Relative => P::Relative,
            scene::Position::Absolute => P::Absolute,
        })
        .with_flex_direction(match s.direction {
            scene::FlexDirection::Row => FD::Row,
            scene::FlexDirection::RowReverse => FD::RowReverse,
            scene::FlexDirection::Column => FD::Column,
            scene::FlexDirection::ColumnReverse => FD::ColumnReverse,
        })
        .with_flex_wrap(match s.wrap {
            scene::FlexWrap::NoWrap => FW::NoWrap,
            scene::FlexWrap::Wrap => FW::Wrap,
            scene::FlexWrap::WrapReverse => FW::WrapReverse,
        })
        .with_justify_content(match s.justify {
            scene::Justify::FlexStart => J::FlexStart,
            scene::Justify::FlexEnd => J::FlexEnd,
            scene::Justify::Center => J::Center,
            scene::Justify::SpaceBetween => J::SpaceBetween,
            scene::Justify::SpaceAround => J::SpaceAround,
            scene::Justify::SpaceEvenly => J::SpaceEvenly,
        })
        .with_align_items(align(s.align_items))
        .with_align_self(s.align_self.map(align))
        .with_align_content(match s.align_content {
            scene::AlignContent::Stretch => AC::Stretch,
            scene::AlignContent::FlexStart => AC::FlexStart,
            scene::AlignContent::FlexEnd => AC::FlexEnd,
            scene::AlignContent::Center => AC::Center,
            scene::AlignContent::SpaceBetween => AC::SpaceBetween,
            scene::AlignContent::SpaceAround => AC::SpaceAround,
            scene::AlignContent::SpaceEvenly => AC::SpaceEvenly,
        })
        .with_flex_grow(s.grow)
        .with_flex_shrink(s.shrink)
        .with_flex_basis(len(s.basis))
        .with_size(len(s.width), len(s.height))
        .with_min_size(len(s.min_width), len(s.min_height))
        .with_max_size(max_len(s.max_width), max_len(s.max_height))
        .with_margin(e(s.margin))
        .with_padding(hooke::Edges {
            left: lp(s.padding.left),
            right: lp(s.padding.right),
            top: lp(s.padding.top),
            bottom: lp(s.padding.bottom),
        })
        .with_border(hooke::Edges {
            left: s.border.left,
            right: s.border.right,
            top: s.border.top,
            bottom: s.border.bottom,
        })
        .with_gap(hooke::Length::Px(s.row_gap), hooke::Length::Px(s.column_gap))
        .with_inset(e(s.inset))
}

fn root_constraints(v: Viewport) -> RootConstraints<f32> {
    let a = |a: Avail| match a {
        Avail::Definite(v) => Available::Definite(v),
        Avail::MaxContent => Available::MaxContent,
    };
    RootConstraints {
        available: Size::new(a(v.width), a(v.height)),
        known: Size::new(Opt::NONE, Opt::NONE),
        direction: hooke::Direction::Ltr,
    }
}

fn build_node(scene: &Scene, i: NodeId) -> Handle<HNode> {
    let n = &scene.nodes[i];
    let props = Handle::new(Props {
        style: to_hooke_style(&n.style),
        version: fresh_id(),
    });
    match &n.text {
        Some(t) => Handle::new(HNode {
            props,
            text: Some(Arc::from(t.as_str())),
            children: Children::empty(),
            layout: LayoutState::measured(fresh_id()),
        }),
        None => Handle::new(HNode {
            props,
            text: None,
            children: Children::from_nodes(n.children.iter().map(|&c| build_node(scene, c)).collect::<Vec<_>>()),
            layout: LayoutState::container(),
        }),
    }
}

pub struct HookeEngine {
    /// The committed revision (or the candidate between `set_text` and `layout`).
    root: Handle<HNode>,
    /// The revision an edit forked from, alive until the candidate is laid out and committed,
    /// as a host's mounted tree would be. Keeping it is what makes `set_text` path-copy.
    previous: Option<Handle<HNode>>,
    constraints: RootConstraints<f32>,
    scratch: RefCell<Scratch<f32>>,
    calls: Cell<u64>,
    /// Parent of each scene node (`u32::MAX` for the root) and its index among its siblings.
    parent: Vec<u32>,
    index_in_parent: Vec<u32>,
    /// Scratch path buffer.
    path: Vec<u32>,
}

impl HookeEngine {
    /// The committed root.
    pub fn root(&self) -> &Handle<HNode> {
        &self.root
    }

    fn path_to(parent: &[u32], index_in_parent: &[u32], node: NodeId, path: &mut Vec<u32>) {
        path.clear();
        let mut n = node as u32;
        while parent[n as usize] != u32::MAX {
            path.push(index_in_parent[n as usize]);
            n = parent[n as usize];
        }
        path.reverse();
    }

    /// Path-copies from `root` to the leaf at `path` (`Handle::make_mut` on the root,
    /// `Children::make_mut` plus `Handle::make_mut` on every level) and replaces its text.
    fn edit(root: &mut Handle<HNode>, path: &[u32], text: &str) {
        let mut node: &mut HNode = Handle::make_mut(root);
        for &i in path {
            node = node.children.make_mut()[i as usize].node_mut();
        }
        node.text = Some(Arc::from(text));
        node.layout.set_content_id(fresh_id());
    }

    /// Marks every node as needing layout without changing the tree, so the next
    /// [`layout`](Engine::layout) is a cold pass over warm storage (the scratch is kept). This
    /// is hooke's counterpart of marking every Taffy node dirty, which is how the cold
    /// workloads repeat a cold layout without rebuilding. Nodes are unique here, so nothing is
    /// cloned.
    pub fn invalidate_all(&mut self) {
        self.previous = None;
        fn walk(node: &mut Handle<HNode>) {
            let n = Handle::make_mut(node);
            n.layout.invalidate();
            for slot in n.children.make_mut() {
                walk(&mut slot.node);
            }
        }
        walk(&mut self.root);
    }

    /// Forks the committed revision, path-copies an edit of `node`'s text and lays the fork
    /// out, leaving the committed revision untouched. Returns the laid-out fork.
    pub fn fork_edit_layout(&self, node: NodeId, text: &str) -> Handle<HNode> {
        let mut fork = self.root.clone();
        let mut path = Vec::new();
        Self::path_to(&self.parent, &self.index_in_parent, node, &mut path);
        Self::edit(&mut fork, &path, text);
        let mut scratch = self.scratch.borrow_mut();
        let mut cx = LayoutCx::new(&mut scratch);
        hooke::layout(&mut fork, self.constraints, &mut cx, &mut Ahem { calls: &self.calls }).expect("not cancelled");
        fork
    }

    /// Forks one candidate per entry of `edits` from the committed revision, edits a
    /// different leaf in each and lays them out on `edits.len()` scoped threads, each with its
    /// own scratch; with `shared_cache`, all of them share one [`SharedMeasureCache`] through
    /// [`LayoutCx::with_cache`]. Returns the laid-out candidates.
    pub fn candidates(&self, edits: &[(NodeId, String)], shared_cache: bool) -> Vec<Handle<HNode>> {
        let cache: SharedMeasureCache<f32> = SharedMeasureCache::with_capacity(4096);
        let cache = shared_cache.then_some(&cache);
        let calls = AtomicU64::new(0);
        let (root, parent, index, constraints) = (&self.root, &self.parent, &self.index_in_parent, self.constraints);
        let calls_ref = &calls;
        let out = std::thread::scope(|s| {
            let handles: Vec<_> = edits
                .iter()
                .map(|(node, text)| {
                    s.spawn(move || {
                        let mut fork = root.clone();
                        let mut path = Vec::new();
                        Self::path_to(parent, index, *node, &mut path);
                        Self::edit(&mut fork, &path, text);
                        let mut scratch = Scratch::new();
                        let mut cx = LayoutCx::new(&mut scratch);
                        if let Some(c) = cache {
                            cx = cx.with_cache(c);
                        }
                        hooke::layout(&mut fork, constraints, &mut cx, &mut AhemAtomic { calls: calls_ref })
                            .expect("not cancelled");
                        fork
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("candidate thread panicked"))
                .collect()
        });
        self.calls.set(self.calls.get() + calls.load(Ordering::Relaxed));
        out
    }

    /// Frames of an arbitrary revision of this tree (a fork), in scene node order.
    pub fn frames_of(root: &Handle<HNode>, out: &mut Vec<Frame>) {
        out.clear();
        fn walk(node: &Handle<HNode>, x: f32, y: f32, hidden: bool, out: &mut Vec<Frame>) {
            let hidden = hidden || node.props.style.display() == hooke::Display::None;
            let s = node.layout.size();
            out.push(if hidden {
                Frame::default()
            } else {
                Frame {
                    x,
                    y,
                    width: s.width,
                    height: s.height,
                }
            });
            for slot in &node.children {
                walk(&slot.node, x + slot.offset.x, y + slot.offset.y, hidden, out);
            }
        }
        walk(root, 0.0, 0.0, false, out);
    }
}

impl Engine for HookeEngine {
    const NAME: &'static str = "hooke";

    fn build(scene: &Scene) -> Self {
        let mut parent = vec![u32::MAX; scene.len()];
        let mut index_in_parent = vec![0u32; scene.len()];
        for (i, n) in scene.nodes.iter().enumerate() {
            for (k, &c) in n.children.iter().enumerate() {
                parent[c] = i as u32;
                index_in_parent[c] = k as u32;
            }
        }
        HookeEngine {
            root: build_node(scene, scene.root()),
            previous: None,
            constraints: root_constraints(scene.viewport),
            scratch: RefCell::new(Scratch::new()),
            calls: Cell::new(0),
            parent,
            index_in_parent,
            path: Vec::new(),
        }
    }

    fn layout(&mut self) {
        let scratch = self.scratch.get_mut();
        let mut cx = LayoutCx::new(scratch);
        hooke::layout(
            &mut self.root,
            self.constraints,
            &mut cx,
            &mut Ahem { calls: &self.calls },
        )
        .expect("not cancelled");
        // Commit: the candidate is now the revision; the one it forked from is released.
        self.previous = None;
    }

    fn frames(&self, out: &mut Vec<Frame>) {
        Self::frames_of(&self.root, out);
    }

    fn set_text(&mut self, node: NodeId, text: &str) {
        // A persistent host keeps the mounted revision while it builds the next one.
        if self.previous.is_none() {
            self.previous = Some(self.root.clone());
        }
        let mut path = std::mem::take(&mut self.path);
        Self::path_to(&self.parent, &self.index_in_parent, node, &mut path);
        Self::edit(&mut self.root, &path, text);
        self.path = path;
    }

    fn resize(&mut self, viewport: Viewport) {
        self.constraints = root_constraints(viewport);
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
    use crate::engines::taffy::TaffyEngine;
    use crate::gate::{Verdict, compare};
    use crate::workloads;

    #[test]
    fn fork_and_candidates_match_an_in_place_edit_and_taffys_clone() {
        let w = workloads::make("text", "1k");
        let edit = w.edit.as_ref().unwrap();
        let mut h = HookeEngine::build(&w.scene);
        h.layout();
        let mut before = Vec::new();
        h.frames(&mut before);

        let fork = h.fork_edit_layout(edit.node, &edit.after);
        let mut forked = Vec::new();
        HookeEngine::frames_of(&fork, &mut forked);
        let mut committed = Vec::new();
        h.frames(&mut committed);
        assert_eq!(
            compare(&before, &committed),
            Verdict::Agrees,
            "the committed revision is intact"
        );

        let mut t = TaffyEngine::build(&w.scene);
        t.layout();
        let cloned = t.clone_edit_layout(edit.node, &edit.after);
        let mut theirs = Vec::new();
        t.frames_of(&cloned, &mut theirs);
        assert_eq!(compare(&forked, &theirs), Verdict::Agrees, "fork vs Taffy clone");

        let mut in_place = HookeEngine::build(&w.scene);
        in_place.layout();
        in_place.set_text(edit.node, &edit.after);
        in_place.layout();
        let mut edited = Vec::new();
        in_place.frames(&mut edited);
        assert_eq!(compare(&forked, &edited), Verdict::Agrees, "fork vs in-place edit");

        let edits: Vec<(NodeId, String)> = vec![(edit.node, edit.after.clone()); 3];
        for cache in [false, true] {
            for c in h.candidates(&edits, cache) {
                let mut f = Vec::new();
                HookeEngine::frames_of(&c, &mut f);
                assert_eq!(compare(&forked, &f), Verdict::Agrees, "candidate, cache {cache}");
            }
        }
    }

    #[test]
    fn invalidate_all_relays_out_to_the_same_frames() {
        let w = workloads::make("random", "1k");
        let mut h = HookeEngine::build(&w.scene);
        h.layout();
        let mut a = Vec::new();
        h.frames(&mut a);
        h.reset_measure_calls();
        h.invalidate_all();
        h.layout();
        assert!(h.measure_calls() > 0, "an invalidated pass measures again");
        let mut b = Vec::new();
        h.frames(&mut b);
        assert_eq!(compare(&a, &b), Verdict::Agrees);
    }
}
