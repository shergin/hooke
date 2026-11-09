//! A Fabric-like host node for tests, plus helpers shared by the test binaries.
#![allow(dead_code)]

use hooke::{
    Available, Children, Handle, LayoutCx, LayoutNode, LayoutState, Measure, MeasureCache, MeasureInput, Measured,
    Opt, Parts, PartsMut, RootConstraints, Scratch, Size, Stats, Style,
};
use std::sync::atomic::{AtomicU64, Ordering};

pub mod fixtures;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub fn fresh_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

pub struct Props {
    pub style: Style,
    pub version: u64,
}
impl Props {
    pub fn new(style: Style) -> Handle<Props> {
        Handle::new(Props { style, version: fresh_id() })
    }
}

#[derive(Clone, Debug)]
pub enum Kind {
    View,
    /// A text-like leaf with an intrinsic size; `chars` drives the width.
    Text { intrinsic: Size<f32> },
    /// A measured leaf whose attachment children are laid out re-entrantly.
    Paragraph,
    /// A text leaf measured by Taffy's Ahem text model ([`measure_ahem`]), as in the Chrome
    /// fixtures: `text` with zero-width-space break opportunities, `vertical` for a vertical
    /// writing mode (Taffy's `TestNodeContext::ahem_text`).
    AhemText { text: String, vertical: bool },
}

/// The host node. `Clone` is shallow.
#[derive(Clone)]
pub struct Node {
    pub tag: u32,
    pub props: Handle<Props>,
    pub kind: Kind,
    pub children: Children<Node>,
    pub layout: LayoutState,
}

impl LayoutNode for Node {
    type Scalar = f32;
    fn parts(&self) -> Parts<'_, Self> {
        Parts { style: &self.props.style, state: &self.layout, children: &self.children }
    }
    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { style: &self.props.style, state: &mut self.layout, children: &mut self.children }
    }
    fn style_key(&self) -> u64 {
        self.props.version
    }
}

pub fn view(tag: u32, style: Style, children: Vec<Handle<Node>>) -> Handle<Node> {
    Handle::new(Node {
        tag,
        props: Props::new(style),
        kind: Kind::View,
        children: Children::from_nodes(children),
        layout: LayoutState::container(),
    })
}

pub fn text(tag: u32, style: Style, width: f32, height: f32) -> Handle<Node> {
    Handle::new(Node {
        tag,
        props: Props::new(style),
        kind: Kind::Text { intrinsic: Size::new(width, height) },
        children: Children::empty(),
        layout: LayoutState::measured(fresh_id()),
    })
}

pub fn paragraph(tag: u32, style: Style, attachments: Vec<Handle<Node>>) -> Handle<Node> {
    Handle::new(Node {
        tag,
        props: Props::new(style),
        kind: Kind::Paragraph,
        children: Children::from_nodes(attachments),
        layout: LayoutState::measured(fresh_id()),
    })
}

/// Path copy: `make_mut` at every level. The borrow path is the dirty path.
pub fn node_at_mut<'a>(root: &'a mut Handle<Node>, path: &[usize]) -> &'a mut Node {
    let mut node: &mut Node = Handle::make_mut(root);
    for &i in path {
        let next = node.children.make_mut()[i].node_mut();
        node = next;
    }
    node
}

/// The test measurer. Text answers its intrinsic size, shrunk to a definite available width;
/// the same rule is mirrored in the Taffy oracle so both engines see identical leaves.
pub struct Shaper {
    pub calls: u32,
}

pub fn measure_text(intrinsic: Size<f32>, input: MeasureInput<f32>) -> Measured<f32> {
    let width = match input.known.width.get() {
        Some(w) => w,
        None => match input.available.width {
            Available::Definite(a) => intrinsic.width.min(a),
            Available::MinContent => intrinsic.width.min(10.0),
            Available::MaxContent => intrinsic.width,
        },
    };
    let height = match input.known.height.get() {
        Some(h) => h,
        None => intrinsic.height,
    };
    Measured { size: Size::new(width, height), first_baseline: Opt::some(height * 0.8), last_baseline: Opt::some(height * 0.8) }
}

impl Measure<Node> for Shaper {
    fn measure(&mut self, node: &mut Node, input: MeasureInput<f32>, cx: &mut LayoutCx<'_, f32>) -> Measured<f32> {
        self.calls += 1;
        match node.kind.clone() {
            Kind::Text { intrinsic } => measure_text(intrinsic, input),
            Kind::Paragraph => {
                // Attachments are real subtrees: lay them out re-entrantly, then place them inline.
                let mut x = 50.0f32;
                let n = node.children.len();
                for i in 0..n {
                    let c = RootConstraints {
                        available: Size::new(Available::MaxContent, input.available.height),
                        known: Size::new(Opt::NONE, Opt::NONE),
                        direction: hooke::Direction::Ltr,
                    };
                    let size = {
                        let slots = node.children.make_mut_untracked();
                        hooke::layout(&mut slots[i].node, c, cx, self).unwrap_or(Size::ZERO)
                    };
                    node.children.make_mut_untracked()[i].offset = hooke::Point::new(x, 0.0);
                    x += size.width;
                }
                Measured { size: Size::new(x, 12.0), first_baseline: Opt::some(10.0), last_baseline: Opt::some(10.0) }
            }
            Kind::AhemText { text, vertical } => measure_ahem(&text, vertical, input),
            Kind::View => Measured::default(),
        }
    }
}

/// A text leaf measured by [`measure_ahem`].
pub fn ahem_text(tag: u32, style: Style, text: &str, vertical: bool) -> Handle<Node> {
    Handle::new(Node {
        tag,
        props: Props::new(style),
        kind: Kind::AhemText { text: text.to_string(), vertical },
        children: Children::empty(),
        layout: LayoutState::measured(fresh_id()),
    })
}

/// Taffy v0.15.1's test text model (`tests/common/src/lib.rs`: `test_measure_function` with
/// `TestMeasureData::AhemText`), ported line for line.
///
/// Every glyph of the Ahem font is a 10px square at the fixtures' 10px font size and line height
/// 1; `U+200B` (zero-width space) is the only break opportunity. Line lengths are counted in
/// UTF-8 bytes, as Taffy counts them. A dimension the engine already knows is echoed back. The
/// model has no baselines (Taffy's leaf reports none), so none are returned.
pub fn measure_ahem(text: &str, vertical: bool, input: MeasureInput<f32>) -> Measured<f32> {
    const ZWS: char = '\u{200B}';
    const H_WIDTH: f32 = 10.0;
    const H_HEIGHT: f32 = 10.0;

    let known = input.known;
    if let (Some(width), Some(height)) = (known.width.get(), known.height.get()) {
        return Measured::size(width, height);
    }

    let (inline_known, block_known, inline_available) = if vertical {
        (known.height, known.width, input.available.height)
    } else {
        (known.width, known.height, input.available.width)
    };
    let lines: Vec<&str> = text.split(ZWS).collect();
    let computed = if lines.is_empty() {
        Size::new(0.0, 0.0)
    } else {
        let min_line_length: usize = lines.iter().map(|line| line.len()).max().unwrap_or(0);
        let max_line_length: usize = lines.iter().map(|line| line.len()).sum();
        let inline_size = inline_known
            .get()
            .unwrap_or_else(|| match inline_available {
                Available::MinContent => min_line_length as f32 * H_WIDTH,
                Available::MaxContent => max_line_length as f32 * H_WIDTH,
                Available::Definite(inline_size) => inline_size.min(max_line_length as f32 * H_WIDTH),
            })
            .max(min_line_length as f32 * H_WIDTH);
        let block_size = block_known.get().unwrap_or_else(|| {
            let inline_line_length = (inline_size / H_WIDTH).floor() as usize;
            let mut line_count = 1;
            let mut current_line_length = 0;
            for line in &lines {
                if current_line_length + line.len() > inline_line_length {
                    if current_line_length > 0 {
                        line_count += 1
                    };
                    current_line_length = line.len();
                } else {
                    current_line_length += line.len();
                };
            }
            (line_count as f32) * H_HEIGHT
        });
        if vertical { Size::new(block_size, inline_size) } else { Size::new(inline_size, block_size) }
    };

    Measured::size(known.width.get().unwrap_or(computed.width), known.height.get().unwrap_or(computed.height))
}

pub fn run(root: &mut Handle<Node>, viewport: Size<f32>, cache: Option<&dyn MeasureCache<f32>>) -> (Size<f32>, Stats, u32) {
    let mut scratch = Scratch::new();
    let mut cx = LayoutCx::new(&mut scratch);
    if let Some(c) = cache {
        cx = cx.with_cache(c);
    }
    let mut shaper = Shaper { calls: 0 };
    let size = hooke::layout(root, RootConstraints::viewport(viewport.width, viewport.height), &mut cx, &mut shaper)
        .expect("not cancelled");
    (size, cx.stats, shaper.calls)
}

/// Absolute frames of every node, in pre-order, for whole-tree comparisons.
pub fn frames(root: &Handle<Node>) -> Vec<(u32, [f32; 4])> {
    fn walk(node: &Handle<Node>, origin: [f32; 2], out: &mut Vec<(u32, [f32; 4])>) {
        let s = node.layout.size();
        out.push((node.tag, [origin[0], origin[1], s.width, s.height]));
        for slot in &node.children {
            walk(&slot.node, [origin[0] + slot.offset.x, origin[1] + slot.offset.y], out);
        }
    }
    let mut out = Vec::new();
    walk(root, [0.0, 0.0], &mut out);
    out
}

/// A small deterministic PRNG for random trees.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
    pub fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }
    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u32) as usize]
    }
}

fn px(l: hooke::Length<f32>) -> String {
    match l {
        hooke::Length::Px(v) => format!("{v}"),
        hooke::Length::Percent(p) => format!("{}%", p * 100.0),
        other => format!("{other:?}"),
    }
}

/// Prints a tree with the style properties that matter for debugging a mismatch.
pub fn dump_tree(node: &Handle<Node>, depth: usize) {
    let s = &node.props.style;
    let kind = match node.kind {
        Kind::Text { intrinsic } => format!("Text {}x{}", intrinsic.width, intrinsic.height),
        _ => "View".into(),
    };
    let m = s.margin();
    eprintln!(
        "{}#{} {kind} dir={:?} just={:?} ai={:?} as={:?} grow={} shrink={} size=({},{}) max=({},{}) margin=[{} {} {} {}] pad={} border={} gap=({},{}) bs={:?} -> {:?}",
        "  ".repeat(depth), node.tag, s.flex_direction(), s.justify_content(), s.align_items(), s.align_self(), s.flex_grow(), s.flex_shrink(),
        px(s.width()), px(s.height()), px(s.max_width()), px(s.max_height()), px(m.left), px(m.right), px(m.top), px(m.bottom),
        px(s.padding().left), s.border().left, px(s.row_gap()), px(s.column_gap()), s.box_sizing(), node.layout.size()
    );
    for slot in &node.children {
        eprint!("{}  @({}, {}) ", "  ".repeat(depth), slot.offset.x, slot.offset.y);
        dump_tree(&slot.node, depth + 1);
    }
}
