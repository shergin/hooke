//! Workload generators: seeded (xorshift), deterministic scenes at three sizes.
//!
//! Every scene's root has a definite size that does not come from any engine's rule for an
//! auto-sized root: it is `100%` of a definite viewport on both axes (a percentage of the
//! viewport rather than a px length, so the resize workload can change it through
//! [`Engine::resize`](crate::engine::Engine::resize)).
//!
//! - **arewelayoutyet** ([`AWLY`]): the PanGUI templates that arewelayoutyet times (copied there
//!   from Taffy's `benches/pangui`, MIT), ported with their flexbox translation from Taffy's
//!   `benches/benches/pangui.rs` and scaled by their repeat counts. Each template root sits in a
//!   column wrapper of the size arewelayoutyet lays it out into (its own px size, else 1000),
//!   which is how the harness treats it ("the child of a column"). `percentage_and_ratio` is
//!   left out: the scene style has no `aspect-ratio`.
//! - **deep**: [`deep_chain`], a chain of auto-sized containers alternating row and column
//!   with one text leaf at the bottom, and [`deep_branch`], auto-sized containers with fan-out
//!   2 to 3 and text leaves.
//! - **text** ([`text_columns`]): ten fixed-fraction columns of paragraphs of 3 to 20 words.
//! - **random** ([`random_cards`]): a wrapping row of random flex subtrees ("cards") with
//!   one-word text leaves, generated inside the region where Taffy agrees with Chrome
//!   (`tests/taffy_oracle.rs` lists the exclusions this generator keeps).

use crate::scene::{
    Align, AlignContent, Avail, Dim, Edges, FlexDirection, FlexWrap, Justify, Lp, Node, NodeId, Scene, Style, Viewport,
};

/// The xorshift64 generator of hooke's test support.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % n as u64) as u32
    }
    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + self.below(hi - lo + 1)
    }
    pub fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }
    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u32) as usize]
    }
}

/// `words` words of `min..=max` `H`s joined by `U+200B`.
pub fn words(rng: &mut Rng, words: u32, min: u32, max: u32) -> String {
    let mut s = String::new();
    for i in 0..words {
        if i > 0 {
            s.push('\u{200B}');
        }
        for _ in 0..rng.range(min, max) {
            s.push('H');
        }
    }
    s
}

/// An edit of one text leaf: `before` is its text in the scene, `after` the replacement.
#[derive(Clone, Debug)]
pub struct Edit {
    pub node: NodeId,
    pub before: String,
    pub after: String,
}

/// One workload at one size.
#[derive(Clone, Debug)]
pub struct Workload {
    pub name: &'static str,
    pub size: &'static str,
    pub scene: Scene,
    /// A text leaf about halfway through pre-order, if the scene has text.
    pub edit: Option<Edit>,
    /// The viewport 10% wider.
    pub resized: Viewport,
}

/// The three sizes, by label and target node count.
pub const SIZES: [(&str, usize); 3] = [("1k", 1_000), ("10k", 10_000), ("100k", 100_000)];

/// The deep chain's sizes: depth beyond 10k is not sensible for recursive engines.
pub const CHAIN_SIZES: [(&str, usize); 3] = [("100", 100), ("1k", 1_000), ("10k", 10_000)];

/// The arewelayoutyet templates that the scene style can express.
pub const AWLY: [&str; 11] = [
    "expand_with_max_constraint",
    "expand_with_min_constraint",
    "fit_nesting",
    "flex_expand_equal_weights",
    "flex_expand_weights",
    "nested_vertical_stack",
    "padding_and_margin",
    "perpendicular_expand_with_wrap",
    "pixels_with_min_expand_constraint",
    "wide_no_wrap_simple",
    "wide_wrapping",
];

/// The sizes of a workload.
pub fn sizes(workload: &str) -> &'static [(&'static str, usize)] {
    if workload == "deep_chain" { &CHAIN_SIZES } else { &SIZES }
}

/// One benchmark group: the family, the criterion group name, and the workload it runs.
#[derive(Copy, Clone, Debug)]
pub struct Group {
    pub family: &'static str,
    pub group: &'static str,
    pub workload: &'static str,
}

/// Every benchmark group, in report order.
pub fn groups() -> Vec<Group> {
    const COLD: [&str; 12] = [
        "cold_expand_with_max_constraint",
        "cold_expand_with_min_constraint",
        "cold_fit_nesting",
        "cold_flex_expand_equal_weights",
        "cold_flex_expand_weights",
        "cold_nested_vertical_stack",
        "cold_padding_and_margin",
        "cold_perpendicular_expand_with_wrap",
        "cold_pixels_with_min_expand_constraint",
        "cold_wide_no_wrap_simple",
        "cold_wide_wrapping",
        "cold_random",
    ];
    let mut g: Vec<Group> = AWLY.iter().zip(COLD.iter()).map(|(w, c)| Group { family: "cold", group: c, workload: w }).collect();
    g.push(Group { family: "cold", group: COLD[11], workload: "random" });
    g.push(Group { family: "deep", group: "deep_chain", workload: "deep_chain" });
    g.push(Group { family: "deep", group: "deep_branch", workload: "deep_branch" });
    g.push(Group { family: "text", group: "text_columns", workload: "text" });
    g.push(Group { family: "edit", group: "edit_random", workload: "random" });
    g.push(Group { family: "edit", group: "edit_text", workload: "text" });
    g.push(Group { family: "resize", group: "resize_random", workload: "random" });
    g.push(Group { family: "resize", group: "resize_text", workload: "text" });
    g.push(Group { family: "candidates", group: "candidates_text", workload: "text" });
    g
}

/// Every distinct workload name.
pub fn workload_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = AWLY.to_vec();
    v.extend(["random", "deep_chain", "deep_branch", "text"]);
    v
}

/// Builds workload `name` at size `size` (a label from [`sizes`]).
pub fn make(name: &'static str, size: &'static str) -> Workload {
    let (label, target) = *sizes(name).iter().find(|(l, _)| *l == size).unwrap_or_else(|| panic!("no size {size} for {name}"));
    let scene = match name {
        "random" => random_cards(target, 1),
        "text" => text_columns(target, 2),
        "deep_chain" => deep_chain(target),
        "deep_branch" => deep_branch(target, 3),
        awly => awly_scene(awly, target),
    };
    finish(name, label, scene)
}

fn finish(name: &'static str, size: &'static str, scene: Scene) -> Workload {
    let leaves: Vec<NodeId> = scene.text_leaves().collect();
    let edit = (!leaves.is_empty()).then(|| {
        let node = leaves.iter().copied().find(|&i| i >= scene.len() / 2).unwrap_or(leaves[leaves.len() - 1]);
        let before = scene.nodes[node].text.clone().unwrap_or_default();
        // Two more words where the text wraps, four more characters where it is one word.
        let after = if before.contains('\u{200B}') { format!("{before}\u{200B}HHHHHHH\u{200B}HHH") } else { format!("{before}HHHH") };
        Edit { node, before, after }
    });
    let widen = |a: Avail| match a {
        Avail::Definite(v) => Avail::Definite(v * 1.1),
        other => other,
    };
    let resized = Viewport { width: widen(scene.viewport.width), height: scene.viewport.height };
    Workload { name, size, scene, edit, resized }
}

/// `n` text leaves spread evenly through pre-order, for the candidates workload.
pub fn spread_leaves(scene: &Scene, n: usize) -> Vec<NodeId> {
    let leaves: Vec<NodeId> = scene.text_leaves().collect();
    (0..n).map(|i| leaves[(i + 1) * leaves.len() / (n + 1)]).collect()
}

// ---------------------------------------------------------------------------------------------
// building
// ---------------------------------------------------------------------------------------------

struct Builder {
    nodes: Vec<Node>,
}

impl Builder {
    fn new() -> Self {
        Builder { nodes: Vec::new() }
    }
    fn container(&mut self, style: Style, parent: Option<NodeId>) -> NodeId {
        self.push(Node { style, children: Vec::new(), text: None }, parent)
    }
    fn text(&mut self, style: Style, text: String, parent: Option<NodeId>) -> NodeId {
        self.push(Node { style, children: Vec::new(), text: Some(text) }, parent)
    }
    fn push(&mut self, node: Node, parent: Option<NodeId>) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(node);
        if let Some(p) = parent {
            self.nodes[p].children.push(id);
        }
        id
    }
    fn finish(self, width: f32, height: f32) -> Scene {
        Scene { nodes: self.nodes, viewport: Viewport { width: Avail::Definite(width), height: Avail::Definite(height) } }
    }
}

/// The root of every scene: `100%` of the viewport on both axes.
fn root_style() -> Style {
    Style { width: Dim::Percent(1.0), height: Dim::Percent(1.0), ..Style::default() }
}

// ---------------------------------------------------------------------------------------------
// arewelayoutyet (PanGUI templates)
// ---------------------------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq, Debug)]
enum Sz {
    Px(f32),
    Fit,
    Expand(f32),
}

/// A PanGUI template node.
#[derive(Clone, Debug)]
struct P {
    w: Option<Sz>,
    h: Option<Sz>,
    min_w: Option<Sz>,
    max_w: Option<Sz>,
    gap: f32,
    padding: f32,
    margin: f32,
    wrap: bool,
    horizontal: bool,
    children: Vec<P>,
}

fn p(w: Option<Sz>, h: Option<Sz>) -> P {
    P { w, h, min_w: None, max_w: None, gap: 0.0, padding: 0.0, margin: 0.0, wrap: false, horizontal: false, children: Vec::new() }
}

impl P {
    fn horizontal(mut self) -> Self {
        self.horizontal = true;
        self
    }
    fn vertical(mut self) -> Self {
        self.horizontal = false;
        self
    }
    fn wrap(mut self) -> Self {
        self.wrap = true;
        self
    }
    fn padding(mut self, v: f32) -> Self {
        self.padding = v;
        self
    }
    fn margin(mut self, v: f32) -> Self {
        self.margin = v;
        self
    }
    fn gap(mut self, v: f32) -> Self {
        self.gap = v;
        self
    }
    fn min_w(mut self, s: Sz) -> Self {
        self.min_w = Some(s);
        self
    }
    fn max_w(mut self, s: Sz) -> Self {
        self.max_w = Some(s);
        self
    }
    fn with(mut self, children: Vec<P>) -> Self {
        self.children = children;
        self
    }
}

fn px(v: f32) -> Option<Sz> {
    Some(Sz::Px(v))
}
const FIT: Option<Sz> = Some(Sz::Fit);
fn expand(w: f32) -> Option<Sz> {
    Some(Sz::Expand(w))
}

fn repeat(n: usize, items: &[P]) -> Vec<P> {
    let mut v = Vec::with_capacity(n * items.len());
    for _ in 0..n {
        v.extend(items.iter().cloned());
    }
    v
}

/// Taffy's translation of a PanGUI node (`benches/benches/pangui.rs`, `style`): an `Expand`
/// main size is `flex: <weight> 1 0` with a zero minimum, an `Expand` minimum main size is
/// `flex: <weight> 0 auto`, everything else `flex: 0 0 auto`; a cross-axis `Expand` is the
/// default stretch; `justify-content` and `align-content` start.
fn pangui_style(n: &P, parent_is_row: bool) -> Style {
    let dim = |s: Option<Sz>| match s {
        Some(Sz::Px(v)) => Dim::Px(v),
        _ => Dim::Auto,
    };
    let (main, main_min) = if parent_is_row { (n.w, n.min_w) } else { (n.h, None) };
    let expands = matches!(main, Some(Sz::Expand(_)));
    let (grow, shrink, basis) = match (main, main_min) {
        (Some(Sz::Expand(w)), _) => (w, 1.0, Dim::Px(0.0)),
        (_, Some(Sz::Expand(w))) => (w, 0.0, Dim::Auto),
        _ => (0.0, 0.0, Dim::Auto),
    };
    let mut min_width = if parent_is_row && matches!(n.min_w, Some(Sz::Expand(_))) { Dim::Auto } else { dim(n.min_w) };
    let mut min_height = Dim::Auto;
    if expands {
        if parent_is_row && n.min_w.is_none() {
            min_width = Dim::Px(0.0);
        }
        if !parent_is_row {
            min_height = Dim::Px(0.0);
        }
    }
    Style {
        direction: if n.horizontal { FlexDirection::Row } else { FlexDirection::Column },
        wrap: if n.wrap { FlexWrap::Wrap } else { FlexWrap::NoWrap },
        width: dim(n.w),
        height: dim(n.h),
        min_width,
        min_height,
        max_width: dim(n.max_w),
        row_gap: n.gap,
        column_gap: n.gap,
        padding: Edges::all(Lp::Px(n.padding)),
        margin: Edges::all(Dim::Px(n.margin)),
        grow,
        shrink,
        basis,
        align_items: Align::Stretch,
        justify: Justify::FlexStart,
        align_content: AlignContent::FlexStart,
        ..Style::default()
    }
}

fn emit(b: &mut Builder, n: &P, parent: NodeId, parent_is_row: bool) {
    let id = b.container(pangui_style(n, parent_is_row), Some(parent));
    for c in &n.children {
        emit(b, c, id, n.horizontal);
    }
}

/// The repeat count of a template with `per` nodes per repeat for about `target` nodes. The
/// root and the wrapper are not subtracted, so the 1k, 10k and 100k counts of a template are
/// arewelayoutyet's own where its native repeat is one of them.
fn reps(target: usize, per: usize) -> usize {
    ((target + per / 2) / per).max(1)
}

fn template(name: &str, target: usize) -> P {
    let leaf10 = p(px(10.0), px(10.0));
    match name {
        "wide_no_wrap_simple" => p(px(100.0), px(100.0)).horizontal().with(repeat(reps(target, 1), &[leaf10])),
        "wide_wrapping" => p(px(100.0), None).wrap().horizontal().with(repeat(reps(target, 1), &[leaf10])),
        "nested_vertical_stack" => {
            p(px(200.0), FIT).vertical().padding(10.0).gap(5.0).with(repeat(reps(target, 1), &[p(expand(1.0), px(1.0))]))
        }
        "padding_and_margin" => p(px(1000.0), FIT)
            .vertical()
            .padding(20.0)
            .gap(10.0)
            .with(repeat(reps(target, 1), &[p(expand(1.0), px(20.0)).margin(2.0)])),
        "flex_expand_equal_weights" => {
            p(px(10000.0), px(100.0)).horizontal().with(repeat(reps(target, 1), &[p(expand(1.0), expand(1.0))]))
        }
        "flex_expand_weights" => {
            let group = p(px(100.0), px(100.0))
                .horizontal()
                .with(vec![p(expand(1.0), expand(1.0)), p(expand(2.0), expand(1.0))]);
            p(px(10000.0), px(100.0)).horizontal().with(repeat(reps(target, 3), &[group]))
        }
        "expand_with_max_constraint" | "expand_with_min_constraint" => {
            let second = p(expand(1.0), px(10.0));
            let second = if name == "expand_with_max_constraint" { second.max_w(Sz::Px(40.0)) } else { second.min_w(Sz::Px(60.0)) };
            let group = p(px(100.0), FIT).horizontal().with(vec![p(expand(1.0), px(10.0)), second]);
            p(px(100.0), FIT).horizontal().with(repeat(reps(target, 3), &[group]))
        }
        "pixels_with_min_expand_constraint" => {
            let item = p(px(10.0), px(10.0)).min_w(Sz::Expand(1.0));
            let group = p(px(100.0), FIT).horizontal().with(vec![item.clone(), item]);
            p(px(100.0), FIT).horizontal().with(repeat(reps(target, 3), &[group]))
        }
        "perpendicular_expand_with_wrap" => {
            let mut set = Vec::new();
            for s in [10.0, 20.0, 30.0, 40.0, 50.0, 60.0] {
                set.push(p(px(s), px(s)));
                set.push(p(px(1.0), expand(1.0)));
            }
            p(px(100.0), FIT).wrap().horizontal().with(repeat(reps(target, 12), &set))
        }
        "fit_nesting" => {
            // 1 + outer * (1 + 10 * (1 + 10 * (1 + leaves))); arewelayoutyet's is (10, 100).
            let (outer, leaves) = match target {
                t if t <= 2_000 => (1, 9),
                t if t <= 20_000 => (1, 100),
                _ => (10, 100),
            };
            let row = p(expand(1.0), FIT).horizontal().with(repeat(leaves, &[p(expand(1.0), px(10.0))]));
            let col = p(expand(1.0), FIT).vertical().with(repeat(10, &[row]));
            let mid = p(expand(1.0), FIT).horizontal().with(repeat(10, &[col]));
            p(px(1000.0), FIT).vertical().padding(5.0).with(repeat(outer, &[mid]))
        }
        other => panic!("unknown arewelayoutyet template {other}"),
    }
}

/// An arewelayoutyet template scaled to about `target` nodes, in its column wrapper.
pub fn awly_scene(name: &str, target: usize) -> Scene {
    let t = template(name, target);
    let size = |s: Option<Sz>| match s {
        Some(Sz::Px(v)) => v,
        _ => 1000.0,
    };
    let (w, h) = (size(t.w), size(t.h));
    let mut b = Builder::new();
    let root = b.container(Style { direction: FlexDirection::Column, align_items: Align::FlexStart, ..root_style() }, None);
    emit(&mut b, &t, root, false);
    b.finish(w, h)
}

// ---------------------------------------------------------------------------------------------
// deep
// ---------------------------------------------------------------------------------------------

/// The deep scenes' viewport, wide enough that no auto-sized row overflows it at any size. A
/// content-sized row container that overflows its available space is where Taffy departs
/// from Chrome (it lays the row out at the sum of its items' flex base sizes, hooke at
/// `fit-content`; cephal's `TAFFY_DIVERGENCES.md`, fixture `chrome_truth/root_flex_width_max_content`),
/// and these workloads time nesting, not that divergence.
pub const DEEP_VIEWPORT: f32 = 65536.0;

/// A chain of `depth` auto-sized containers alternating row and column, each `align-items:
/// flex-start` with 1px padding, and one text leaf at the bottom.
pub fn deep_chain(depth: usize) -> Scene {
    let mut b = Builder::new();
    let mut parent = b.container(Style { direction: FlexDirection::Column, align_items: Align::FlexStart, ..root_style() }, None);
    let depth = depth.saturating_sub(2).max(1);
    for i in 0..depth {
        let style = Style {
            direction: if i.is_multiple_of(2) { FlexDirection::Row } else { FlexDirection::Column },
            align_items: Align::FlexStart,
            padding: Edges::all(Lp::Px(1.0)),
            ..Style::default()
        };
        parent = b.container(style, Some(parent));
    }
    b.text(Style::default(), "HHHH\u{200B}HHH\u{200B}HHHHH".to_string(), Some(parent));
    b.finish(DEEP_VIEWPORT, DEEP_VIEWPORT)
}

/// About `target` nodes of auto-sized containers with fan-out 2 to 3, alternating row and
/// column by depth, grown breadth first; the unexpanded positions are text leaves of 1 to 3
/// words.
pub fn deep_branch(target: usize, seed: u64) -> Scene {
    let mut rng = Rng::new(seed);
    // Shape first: children per position, grown in FIFO order.
    let mut kids: Vec<Vec<usize>> = vec![Vec::new()];
    let mut next = 0;
    let budget = target.saturating_sub(1).max(2);
    while kids.len() < budget {
        let fan = rng.range(2, 3) as usize;
        for _ in 0..fan {
            let id = kids.len();
            kids.push(Vec::new());
            kids[next].push(id);
        }
        next += 1;
    }
    let mut b = Builder::new();
    let root = b.container(Style { direction: FlexDirection::Column, align_items: Align::FlexStart, ..root_style() }, None);
    fn walk(b: &mut Builder, kids: &[Vec<usize>], at: usize, depth: usize, parent: NodeId, rng: &mut Rng) {
        if kids[at].is_empty() {
            let n = rng.range(1, 3);
            let t = words(rng, n, 1, 5);
            b.text(Style::default(), t, Some(parent));
            return;
        }
        let style = Style {
            direction: if depth.is_multiple_of(2) { FlexDirection::Row } else { FlexDirection::Column },
            align_items: Align::FlexStart,
            ..Style::default()
        };
        let id = b.container(style, Some(parent));
        for &k in &kids[at] {
            walk(b, kids, k, depth + 1, id, rng);
        }
    }
    walk(&mut b, &kids, 0, 0, root, &mut rng);
    b.finish(DEEP_VIEWPORT, DEEP_VIEWPORT)
}

// ---------------------------------------------------------------------------------------------
// text
// ---------------------------------------------------------------------------------------------

/// Ten columns, each `10%` of a 2000px root (so 200px, wrapping text at 19 characters inside
/// their padding), of paragraphs of 3 to 20 words of 1 to 8 characters.
pub fn text_columns(target: usize, seed: u64) -> Scene {
    const COLUMNS: usize = 10;
    let mut rng = Rng::new(seed);
    let mut b = Builder::new();
    let root = b.container(Style { direction: FlexDirection::Row, ..root_style() }, None);
    let per = (target.saturating_sub(1 + COLUMNS) / COLUMNS).max(1);
    for _ in 0..COLUMNS {
        let col = b.container(
            Style {
                direction: FlexDirection::Column,
                width: Dim::Percent(0.1),
                padding: Edges::all(Lp::Px(5.0)),
                row_gap: 4.0,
                ..Style::default()
            },
            Some(root),
        );
        for _ in 0..per {
            let n = rng.range(3, 20);
            let t = words(&mut rng, n, 1, 8);
            b.text(Style { shrink: 0.0, ..Style::default() }, t, Some(col));
        }
    }
    b.finish(2000.0, 1000.0)
}

// ---------------------------------------------------------------------------------------------
// random
// ---------------------------------------------------------------------------------------------

/// What a child needs to know about its parent; mirrors `Parent` in `tests/taffy_oracle.rs`.
#[derive(Clone, Copy)]
struct Parent {
    column: bool,
    align_items: Align,
    definite_width: bool,
    intrinsic_row: bool,
    intrinsic_width: bool,
}

fn item_style(rng: &mut Rng, parent: Parent) -> Style {
    let mut s = Style {
        min_width: Dim::Px(0.0),
        min_height: Dim::Px(0.0),
        grow: rng.pick(&[0.0, 0.0, 1.0, 2.0]),
        shrink: rng.pick(&[0.0, 1.0, 1.0, 3.0]),
        margin: Edges {
            left: Dim::Px(rng.below(5) as f32),
            right: Dim::Px(rng.below(5) as f32),
            top: Dim::Px(rng.below(5) as f32),
            bottom: Dim::Px(rng.below(5) as f32),
        },
        ..Style::default()
    };
    if rng.chance(30) {
        s.align_self = Some(rng.pick(&[Align::FlexStart, Align::Center, Align::FlexEnd, Align::Stretch]));
    }
    // No `max-width` under a row whose width is derived from its content, and an item with a
    // `max-width` in a row cannot shrink (the oracle's reasons).
    if rng.chance(25) && !parent.intrinsic_row {
        s.max_width = Dim::Px(30.0 + rng.below(100) as f32);
        if !parent.column {
            s.shrink = 0.0;
        }
    }
    s
}

fn container_style(rng: &mut Rng, base: Style, percent_ok: bool) -> Style {
    let mut s = Style {
        direction: rng.pick(&[FlexDirection::Row, FlexDirection::Column, FlexDirection::RowReverse, FlexDirection::ColumnReverse]),
        justify: rng.pick(&[
            Justify::FlexStart,
            Justify::FlexEnd,
            Justify::Center,
            Justify::SpaceBetween,
            Justify::SpaceAround,
            Justify::SpaceEvenly,
        ]),
        align_items: rng.pick(&[Align::Stretch, Align::FlexStart, Align::Center, Align::FlexEnd]),
        padding: Edges::all(Lp::Px(rng.below(8) as f32)),
        border: Edges::all(rng.below(3) as f32),
        row_gap: rng.below(6) as f32,
        column_gap: rng.below(6) as f32,
        ..base
    };
    if rng.chance(40) {
        s.width = Dim::Px(60.0 + rng.below(300) as f32);
    }
    if rng.chance(30) {
        s.height = Dim::Px(30.0 + rng.below(200) as f32);
    }
    if rng.chance(20) && percent_ok {
        s.width = Dim::Percent(0.25 + rng.below(50) as f32 / 100.0);
    }
    s
}

fn random_subtree(b: &mut Builder, rng: &mut Rng, depth: u32, parent_id: NodeId, parent: Parent, card: bool) {
    if !card && (depth == 0 || rng.chance(30)) {
        // One word: no break opportunity, so a leaf's height does not depend on its width.
        // With wrapping leaves here hooke and Taffy split the shrinking of nested
        // content-sized columns differently (see the report's gate section).
        let t = words(rng, 1, 1, 12);
        b.text(item_style(rng, parent), t, Some(parent_id));
        return;
    }
    let base = item_style(rng, parent);
    let mut style = container_style(rng, base, parent.definite_width);
    if card {
        // A card in the wrapping root row: a px width it keeps.
        style.width = Dim::Px(100.0 + rng.below(300) as f32);
        style.shrink = 0.0;
        style.grow = 0.0;
        style.max_width = Dim::Auto;
    }
    let row = matches!(style.direction, FlexDirection::Row | FlexDirection::RowReverse);
    let stretched = parent.column && style.align_self.unwrap_or(parent.align_items) == Align::Stretch;
    let width_from_parent = !parent.column || (stretched && parent.definite_width);
    if style.width == Dim::Auto && !width_from_parent {
        if row {
            // No content-sized row container (Taffy sizes it by its flex base sizes).
            style.width = Dim::Px(60.0 + rng.below(300) as f32);
        } else {
            // No `max-width` on a content-sized column container.
            style.max_width = Dim::Auto;
        }
    }
    // Border-box: floor the min size at padding plus border (the oracle's §9.7 step 4 reason).
    let pb = match style.padding.left {
        Lp::Px(v) => 2.0 * (v + style.border.left),
        Lp::Percent(_) => 2.0 * style.border.left,
    };
    style.min_width = Dim::Px(pb);
    style.min_height = Dim::Px(pb);
    let intrinsic_width = (!parent.column || parent.intrinsic_width)
        && (style.width == Dim::Auto || (matches!(style.width, Dim::Percent(_)) && parent.intrinsic_width));
    let me = Parent {
        column: !row,
        align_items: style.align_items,
        definite_width: matches!(style.width, Dim::Px(_)) || (style.width == Dim::Auto && width_from_parent),
        intrinsic_row: row && intrinsic_width,
        intrinsic_width,
    };
    let id = b.container(style, Some(parent_id));
    let n = 1 + rng.below(4);
    for _ in 0..n {
        random_subtree(b, rng, depth.saturating_sub(1), id, me, false);
    }
}

/// A wrapping row (`100%` of a 1000 by 1000 viewport, lines packed to the start) of random
/// "cards" until about `target` nodes: each card a container of px width with random flex
/// subtrees of depth up to 4 below it and one-word text leaves of 1 to 12 characters.
pub fn random_cards(target: usize, seed: u64) -> Scene {
    let mut rng = Rng::new(seed);
    let mut b = Builder::new();
    let root = b.container(
        Style {
            direction: FlexDirection::Row,
            wrap: FlexWrap::Wrap,
            align_items: Align::FlexStart,
            align_content: AlignContent::FlexStart,
            ..root_style()
        },
        None,
    );
    let me = Parent { column: false, align_items: Align::FlexStart, definite_width: true, intrinsic_row: false, intrinsic_width: false };
    while b.nodes.len() < target {
        random_subtree(&mut b, &mut rng, 4, root, me, true);
    }
    b.finish(1000.0, 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preorder_ok(s: &Scene) -> bool {
        // Each node's children are consecutive subtrees right after it.
        fn walk(s: &Scene, i: NodeId, next: &mut usize) -> bool {
            if i != *next {
                return false;
            }
            *next += 1;
            s.nodes[i].children.iter().all(|&c| walk(s, c, next))
        }
        let mut next = 0;
        walk(s, 0, &mut next) && next == s.len()
    }

    #[test]
    fn workloads_are_preorder_deterministic_and_near_size() {
        for name in workload_names() {
            for &(label, target) in sizes(name).iter().take(2) {
                let a = make(name, label);
                let b = make(name, label);
                assert_eq!(a.scene, b.scene, "{name} {label} deterministic");
                assert!(preorder_ok(&a.scene), "{name} {label} pre-order");
                let n = a.scene.len() as f64;
                assert!(n > target as f64 * 0.7 && n < target as f64 * 1.5, "{name} {label}: {n} nodes for {target}");
            }
        }
    }

    #[test]
    fn awly_native_sizes() {
        // arewelayoutyet's own node counts, plus our wrapper.
        assert_eq!(awly_scene("fit_nesting", 100_000).len(), 101_111 + 1);
        assert_eq!(awly_scene("flex_expand_weights", 15_000).len(), 15_001 + 1);
        assert_eq!(awly_scene("nested_vertical_stack", 10_000).len(), 10_001 + 1);
        assert_eq!(awly_scene("wide_no_wrap_simple", 100_000).len(), 100_001 + 1);
    }
}
