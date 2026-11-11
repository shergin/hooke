//! Differential testing against Taffy 0.15: the same random tree is laid out by both engines
//! and every frame is compared. Taffy is a dev-dependency only.
//!
//! hooke's flex algorithm is arbitrated by Chrome (`tests/chrome_fixtures.rs`), not by Taffy.
//! Taffy is a second oracle, useful only where it agrees with Chrome, so the random generator
//! stays inside that region: single line, no `order`, no baselines, `position: relative`, px
//! margins, padding, borders and gaps, and text leaves whose measure function is shared by both
//! engines. Each place where Taffy 0.15 departs from Chrome is kept out of the generator, with
//! the reason next to the code that does it:
//!
//! - percentage widths only inside a container of definite width ([`Parent::definite_width`]);
//! - no content-sized row container ([`random_tree`]);
//! - no `max-width` on a content-sized container ([`random_tree`]);
//! - a `border-box` container's min size floored at its padding and border ([`random_tree`]);
//! - an item with a `max-width` in a row is not shrinkable ([`item_style`]);
//! - no `max-width` on an item of a row container whose width is derived from its content,
//!   such as its content-based flex base size (css-flexbox-1 §9.9.3, css-sizing-3 §5.1;
//!   [`Parent::intrinsic_row`], [`item_style`]);
//! - no `justify-content: space-between` on a reversed container (css-flexbox-1 §8.2;
//!   [`container_style`]);
//! - text never measures narrower than its min-content width ([`measure_text_leaf`]);
//! - items have `min-size: 0`, so the automatic minimum size (§4.5) is not compared
//!   ([`item_style`]).
//!
//! Set `ORACLE_SEED=<n>` to lay out only that seed and print both trees.

mod support;

use hooke::{AlignItems, BoxSizing, Edges, FlexDirection, Handle, JustifyContent, Length, Position, Size, Style};
use support::*;
use taffy::prelude::*;

fn to_taffy_length(l: Length<f32>) -> taffy::style::Dimension {
    match l {
        Length::Px(v) => taffy::style::Dimension::length(v),
        Length::Percent(p) => taffy::style::Dimension::percent(p),
        _ => taffy::style::Dimension::auto(),
    }
}
fn to_taffy_lpa(l: Length<f32>) -> taffy::style::LengthPercentageAuto {
    match l {
        Length::Px(v) => taffy::style::LengthPercentageAuto::length(v),
        Length::Percent(p) => taffy::style::LengthPercentageAuto::percent(p),
        _ => taffy::style::LengthPercentageAuto::auto(),
    }
}
fn to_taffy_lp(l: Length<f32>) -> taffy::style::LengthPercentage {
    match l {
        Length::Px(v) => taffy::style::LengthPercentage::length(v),
        Length::Percent(p) => taffy::style::LengthPercentage::percent(p),
        _ => taffy::style::LengthPercentage::length(0.0),
    }
}
fn edges_lpa(e: Edges<Length<f32>>) -> taffy::geometry::Rect<taffy::style::LengthPercentageAuto> {
    taffy::geometry::Rect {
        left: to_taffy_lpa(e.left),
        right: to_taffy_lpa(e.right),
        top: to_taffy_lpa(e.top),
        bottom: to_taffy_lpa(e.bottom),
    }
}
fn edges_lp(e: Edges<Length<f32>>) -> taffy::geometry::Rect<taffy::style::LengthPercentage> {
    taffy::geometry::Rect {
        left: to_taffy_lp(e.left),
        right: to_taffy_lp(e.right),
        top: to_taffy_lp(e.top),
        bottom: to_taffy_lp(e.bottom),
    }
}

fn to_taffy_style(s: &Style) -> taffy::style::Style {
    use taffy::style as t;
    let border = s.border();
    t::Style {
        display: t::Display::Flex,
        box_sizing: match s.box_sizing() {
            BoxSizing::ContentBox => t::BoxSizing::ContentBox,
            BoxSizing::BorderBox => t::BoxSizing::BorderBox,
        },
        position: match s.position() {
            Position::Absolute => t::Position::Absolute,
            Position::Relative => t::Position::Relative,
            Position::Static => t::Position::Static,
            other => panic!("unmapped hooke position {other:?}"),
        },
        flex_direction: match s.flex_direction() {
            FlexDirection::Row => t::FlexDirection::Row,
            FlexDirection::RowReverse => t::FlexDirection::RowReverse,
            FlexDirection::Column => t::FlexDirection::Column,
            FlexDirection::ColumnReverse => t::FlexDirection::ColumnReverse,
        },
        flex_wrap: t::FlexWrap::NoWrap,
        flex_grow: s.flex_grow(),
        flex_shrink: s.flex_shrink(),
        flex_basis: to_taffy_length(s.flex_basis()),
        size: taffy::geometry::Size {
            width: to_taffy_length(s.width()),
            height: to_taffy_length(s.height()),
        },
        min_size: taffy::geometry::Size {
            width: to_taffy_lpa(s.min_width()),
            height: to_taffy_lpa(s.min_height()),
        },
        max_size: taffy::geometry::Size {
            width: to_taffy_lpa(s.max_width()),
            height: to_taffy_lpa(s.max_height()),
        },
        margin: edges_lpa(s.margin()),
        padding: edges_lp(s.padding()),
        border: taffy::geometry::Rect {
            left: t::LengthPercentage::length(border.left),
            right: t::LengthPercentage::length(border.right),
            top: t::LengthPercentage::length(border.top),
            bottom: t::LengthPercentage::length(border.bottom),
        },
        gap: taffy::geometry::Size {
            width: to_taffy_lp(s.column_gap()),
            height: to_taffy_lp(s.row_gap()),
        },
        justify_content: match s.justify_content() {
            JustifyContent::Normal => t::JustifyContent::NORMAL,
            JustifyContent::FlexStart | JustifyContent::Start | JustifyContent::Stretch => {
                t::JustifyContent::FLEX_START
            }
            JustifyContent::FlexEnd | JustifyContent::End => t::JustifyContent::FLEX_END,
            JustifyContent::Center => t::JustifyContent::CENTER,
            JustifyContent::SpaceBetween => t::JustifyContent::SPACE_BETWEEN,
            JustifyContent::SpaceAround => t::JustifyContent::SPACE_AROUND,
            JustifyContent::SpaceEvenly => t::JustifyContent::SPACE_EVENLY,
            other => panic!("unmapped hooke justify-content {other:?}"),
        },
        align_items: match s.align_items() {
            AlignItems::Normal => t::AlignItems::NORMAL,
            AlignItems::Stretch => t::AlignItems::STRETCH,
            AlignItems::FlexStart | AlignItems::Start | AlignItems::SelfStart => t::AlignItems::FLEX_START,
            AlignItems::FlexEnd | AlignItems::End | AlignItems::SelfEnd => t::AlignItems::FLEX_END,
            AlignItems::Center => t::AlignItems::CENTER,
            AlignItems::Baseline => t::AlignItems::BASELINE,
            other => panic!("unmapped hooke align-items {other:?}"),
        },
        align_self: s.align_self().map(|a| match a {
            AlignItems::Normal => t::AlignSelf::NORMAL,
            AlignItems::Stretch => t::AlignSelf::STRETCH,
            AlignItems::FlexStart | AlignItems::Start | AlignItems::SelfStart => t::AlignSelf::FLEX_START,
            AlignItems::FlexEnd | AlignItems::End | AlignItems::SelfEnd => t::AlignSelf::FLEX_END,
            AlignItems::Center => t::AlignSelf::CENTER,
            AlignItems::Baseline => t::AlignSelf::BASELINE,
            other => panic!("unmapped hooke align-self {other:?}"),
        }),
        ..Default::default()
    }
}

/// Mirrors a hooke tree into a `TaffyTree`, keeping the text intrinsic sizes as node contexts.
fn mirror(node: &Handle<Node>, tree: &mut TaffyTree<Size<f32>>) -> NodeId {
    let style = to_taffy_style(&node.props.style);
    match node.kind {
        Kind::Text { intrinsic } => tree.new_leaf_with_context(style, intrinsic).unwrap(),
        _ => {
            let kids: Vec<NodeId> = node.children.iter().map(|s| mirror(&s.node, tree)).collect();
            tree.new_with_children(style, &kids).unwrap()
        }
    }
}

/// The oracle's text model, used by both engines: max-content is the intrinsic width,
/// min-content is `min(intrinsic, 10)` (the longest word), and a definite available width
/// wraps the text between the two.
///
/// Unlike `support::measure_text`, a definite available width never squeezes the text below
/// its min-content width, as real text cannot be narrower than its longest word. With the
/// squeezing model the engines disagree whenever an unstretched auto-width item meets less
/// space than its min-content size: hooke sizes it `fit-content`, floored at its min-content
/// size (css-sizing-3 §5.2, css-flexbox-1 §9.4 step 7), while Taffy measures its contents at the
/// available width, which that model answers below their min-content size.
fn measure_text_leaf(intrinsic: Size<f32>, input: hooke::MeasureInput<f32>) -> hooke::Measured<f32> {
    let min_content = intrinsic.width.min(10.0);
    let width = match input.known.width.get() {
        Some(w) => w,
        None => match input.available.width {
            hooke::Available::Definite(a) => intrinsic.width.min(a).max(min_content),
            hooke::Available::MinContent => min_content,
            hooke::Available::MaxContent => intrinsic.width,
        },
    };
    let height = input.known.height.get().unwrap_or(intrinsic.height);
    hooke::Measured {
        size: Size::new(width, height),
        first_baseline: hooke::Opt::some(height * 0.8),
        last_baseline: hooke::Opt::some(height * 0.8),
    }
}

/// hooke's measure function for the oracle: text leaves through [`measure_text_leaf`].
struct TextMeasure;
impl hooke::Measure<Node> for TextMeasure {
    fn measure(
        &mut self,
        node: &mut Node,
        input: hooke::MeasureInput<f32>,
        _cx: &mut hooke::LayoutCx<'_, f32>,
    ) -> hooke::Measured<f32> {
        match node.kind {
            Kind::Text { intrinsic } => measure_text_leaf(intrinsic, input),
            _ => unreachable!("the oracle builds only views and text leaves"),
        }
    }
}

fn taffy_frames(tree: &TaffyTree<Size<f32>>, node: NodeId, origin: [f32; 2], out: &mut Vec<[f32; 4]>) {
    let l = tree.layout(node).unwrap();
    let abs = [origin[0] + l.location.x, origin[1] + l.location.y];
    out.push([abs[0], abs[1], l.size.width, l.size.height]);
    for child in tree.children(node).unwrap() {
        taffy_frames(tree, child, abs, out);
    }
}

fn compare(root: &mut Handle<Node>, seed: u64) {
    if let Err(e) = try_compare(root, seed, true) {
        panic!("{e}");
    }
}

/// Lays `root` out in both engines and compares every frame; `dump` prints both trees on a
/// mismatch.
fn try_compare(root: &mut Handle<Node>, seed: u64, dump: bool) -> Result<(), String> {
    let ours = {
        let mut scratch = hooke::Scratch::new();
        let mut cx = hooke::LayoutCx::new(&mut scratch);
        hooke::layout(
            root,
            hooke::RootConstraints::viewport(800.0, 600.0),
            &mut cx,
            &mut TextMeasure,
        )
        .expect("not cancelled")
    };
    let our_frames: Vec<[f32; 4]> = frames(root).into_iter().map(|(_, f)| f).collect();

    let mut tree: TaffyTree<Size<f32>> = TaffyTree::new();
    tree.disable_rounding();
    let taffy_root = mirror(root, &mut tree);
    tree.compute_layout_with_measure(
        taffy_root,
        taffy::geometry::Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        },
        |input, _id, ctx, style| {
            // Taffy hands the whole leaf to this closure, so go through `compute_leaf_layout`:
            // the leaf's own size, min/max, padding and border then apply as they do in ours
            // (returning the measured size directly ignored e.g. a leaf root's max-width).
            let intrinsic = *ctx.expect("leaves carry their intrinsic size");
            taffy::compute_leaf_layout(
                input,
                style,
                |_, _| 0.0,
                |known, available| {
                    let known = Size::new(known.width.into(), known.height.into());
                    let available = Size::new(
                        from_taffy_available(available.width),
                        from_taffy_available(available.height),
                    );
                    let m = measure_text_leaf(intrinsic, hooke::MeasureInput { known, available });
                    taffy::geometry::Size {
                        width: m.size.width,
                        height: m.size.height,
                    }
                },
            )
        },
    )
    .unwrap();
    // The root's own margin places it inside the viewport in Taffy; our root has no parent to
    // hold an offset, so compare frames relative to the root's border box.
    let root_location = tree.layout(taffy_root).unwrap().location;
    let mut theirs = Vec::new();
    taffy_frames(&tree, taffy_root, [-root_location.x, -root_location.y], &mut theirs);
    let root_layout = tree.layout(taffy_root).unwrap();
    if !((ours.width - root_layout.size.width).abs() < 0.01 && (ours.height - root_layout.size.height).abs() < 0.01) {
        if dump {
            dump_tree(root, 0);
            eprintln!("taffy frames: {theirs:?}");
        }
        return Err(format!(
            "seed {seed}: root size ours {ours:?} taffy {:?}",
            root_layout.size
        ));
    }
    assert_eq!(our_frames.len(), theirs.len());
    for (i, (a, b)) in our_frames.iter().zip(theirs.iter()).enumerate() {
        for k in 0..4 {
            if (a[k] - b[k]).abs() >= 0.01 {
                if dump {
                    dump_tree(root, 0);
                    eprintln!("taffy frames: {theirs:?}");
                }
                return Err(format!("seed {seed}: node #{i} differs: ours {a:?} taffy {b:?}"));
            }
        }
    }
    Ok(())
}

fn from_taffy_available(a: AvailableSpace) -> hooke::Available<f32> {
    match a {
        AvailableSpace::Definite(v) => hooke::Available::Definite(v),
        AvailableSpace::MinContent => hooke::Available::MinContent,
        AvailableSpace::MaxContent => hooke::Available::MaxContent,
    }
}

/// What a child needs to know about its parent; the root has none.
#[derive(Clone, Copy)]
struct Parent {
    column: bool,
    align_items: AlignItems,
    /// The parent's width is definite: a px width, its main size in a row, or a stretched cross
    /// size in a column of definite width. Not a width derived from its own content.
    ///
    /// Percentage widths are generated only below such a parent. Under a content-sized
    /// container Taffy treats an item's percentage width as `auto`; hooke, like Chrome, treats
    /// it as `auto` only while computing the container's intrinsic contribution and then
    /// resolves it against the container's used width (css-sizing-3 §5.2.1, cyclic
    /// percentages; cephal's `TAFFY_DIVERGENCES.md`, see `tests/fixtures/README.md`). The root
    /// resolves its own percentage against the viewport in both engines.
    definite_width: bool,
    /// The parent is a row container whose width is, at least while an ancestor computes its
    /// intrinsic size, derived from its items' max-content contributions: it is an item of a
    /// row with an auto width, or with an auto or percentage width (a cyclic percentage
    /// behaves as `auto` there, css-sizing-3 §5.2.1) inside such an intrinsically sized
    /// container, a column included. No item of it gets a `max-width` ([`item_style`]).
    intrinsic_row: bool,
    /// The parent's width is intrinsic in the sense of [`Parent::intrinsic_row`], whatever its
    /// direction.
    intrinsic_width: bool,
}

fn item_style(rng: &mut Rng, parent: Option<Parent>) -> Style {
    // `min-size: 0` keeps the automatic minimum size (css-flexbox-1 §4.5) out of the
    // comparison; the Chrome fixtures cover it.
    let mut s = Style::web()
        .with_position(Position::Relative)
        .with_min_size(Length::zero(), Length::zero())
        .with_flex_grow(rng.pick(&[0.0, 0.0, 1.0, 2.0]))
        .with_flex_shrink(rng.pick(&[0.0, 1.0, 1.0, 3.0]))
        .with_margin(Edges {
            left: Length::Px(rng.below(5) as f32),
            right: Length::Px(rng.below(5) as f32),
            top: Length::Px(rng.below(5) as f32),
            bottom: Length::Px(rng.below(5) as f32),
        });
    if rng.chance(30) {
        s = s.with_align_self(Some(rng.pick(&[
            AlignItems::FlexStart,
            AlignItems::Center,
            AlignItems::FlexEnd,
            AlignItems::Stretch,
        ])));
    }
    // An item of a row container whose width is derived from its content (its content-based
    // flex base size, or its intrinsic size inside an ancestor's; see
    // [`Parent::intrinsic_row`]) contributes its max-content contribution to that width (css-flexbox-1 §9.9.3): its max-content size,
    // bounded by its flex base size where it cannot grow or shrink, then clamped by its min and
    // max main sizes, with the margins added outside every clamp (css-sizing-3 §5.1: a
    // contribution is the margin box of the clamped size). hooke follows that literally.
    // Taffy's intrinsic branch (`determine_container_main_size` under a min- or max-content
    // constraint) clamps the margin box instead, and for a leaf that cannot shrink lets the
    // flex base size beat the `max-width`. No `max-width` there; margins are kept.
    if rng.chance(25) && !parent.is_some_and(|p| p.intrinsic_row) {
        s = s.with_max_size(Length::Px(30.0 + rng.below(100) as f32), Length::None);
        // In a row, `max-width` is the item's max main size. When the item's content-based
        // flex base size exceeds it, Taffy clamps the flex base size by it and shrinks from the
        // clamped size; hooke, like Chrome, ignores min and max main sizes when determining the
        // flex base size (css-flexbox-1 §9.2 step 3) and shrinks from the unclamped size, so
        // the item's scaled shrink factor and the line's free space differ (§9.7 steps 3-4).
        // An item that cannot shrink is frozen at its hypothetical main size in both engines.
        if parent.is_some_and(|p| !p.column) {
            s = s.with_flex_shrink(0.0);
        }
    }
    s
}

fn container_style(rng: &mut Rng, base: Style, percent_ok: bool) -> Style {
    let mut s = base
        .with_flex_direction(rng.pick(&[
            FlexDirection::Row,
            FlexDirection::Column,
            FlexDirection::RowReverse,
            FlexDirection::ColumnReverse,
        ]))
        .with_justify_content(rng.pick(&[
            JustifyContent::FlexStart,
            JustifyContent::FlexEnd,
            JustifyContent::Center,
            JustifyContent::SpaceBetween,
            JustifyContent::SpaceAround,
            JustifyContent::SpaceEvenly,
        ]))
        .with_align_items(rng.pick(&[
            AlignItems::Stretch,
            AlignItems::FlexStart,
            AlignItems::Center,
            AlignItems::FlexEnd,
        ]))
        .with_padding(Edges::all(Length::Px(rng.below(8) as f32)))
        .with_border(Edges::all(rng.below(3) as f32))
        .with_gap(Length::Px(rng.below(6) as f32), Length::Px(rng.below(6) as f32))
        .with_box_sizing(rng.pick(&[BoxSizing::ContentBox, BoxSizing::BorderBox]));
    // `space-between` with negative free space falls back to `flex-start`, not to a safe start
    // (css-flexbox-1 §8.2, "identical to flex-start"). In a reversed container `flex-start` is
    // the end edge, so an overflowing line overflows past its main-start edge. hooke does
    // this, and so does Chromium (checked on a 255px `row-reverse` with items 60, 171 and 10:
    // x = 189, 18, 7). Taffy clamps the line to the start edge instead (192, 21, 10). This is
    // one of cephal's documented divergences (`chrome_truth/flex_space_between_overflow_reversed`
    // in its corpus, not converted because it uses `display: block`). Whether a line overflows
    // is not known while generating, so a reversed container never gets `space-between`;
    // `space-around` and `space-evenly` fall back to a safe `center` in both engines.
    if matches!(
        s.flex_direction(),
        FlexDirection::RowReverse | FlexDirection::ColumnReverse
    ) && s.justify_content() == JustifyContent::SpaceBetween
    {
        s = s.with_justify_content(JustifyContent::SpaceAround);
    }
    if rng.chance(40) {
        s = s.with_size(Length::Px(60.0 + rng.below(300) as f32), Length::Auto);
    }
    if rng.chance(30) {
        s = s.with_size(s.width(), Length::Px(30.0 + rng.below(200) as f32));
    }
    if rng.chance(20) && percent_ok {
        s = s.with_size(Length::Percent(0.25 + rng.below(50) as f32 / 100.0), s.height());
    }
    s
}

fn random_tree(rng: &mut Rng, depth: u32, tag: &mut u32, parent: Option<Parent>) -> Handle<Node> {
    *tag += 1;
    let t = *tag;
    if depth == 0 || rng.chance(30) {
        return text(
            t,
            item_style(rng, parent),
            10.0 + rng.below(90) as f32,
            8.0 + rng.below(30) as f32,
        );
    }
    let base = item_style(rng, parent);
    let mut style = container_style(rng, base, parent.is_none_or(|p| p.definite_width));
    let row = matches!(style.flex_direction(), FlexDirection::Row | FlexDirection::RowReverse);
    let stretched =
        parent.is_some_and(|p| p.column && style.align_self().unwrap_or(p.align_items) == AlignItems::Stretch);
    // An auto width that the parent decides: the main size in a row, or a stretch to a column
    // of definite width. Any other auto width (the root, an unstretched item in a column, a
    // stretch to a content-sized column) is content-sized: `fit-content` against the available
    // space.
    let width_from_parent = parent.is_some_and(|p| !p.column || (stretched && p.definite_width));
    if style.width().is_auto() && !width_from_parent {
        // A content-sized row container. hooke, like Chrome, sizes it `fit-content`:
        // `min(max-content, max(min-content, available))`, its max-content size being the sum
        // of its items' max-content contributions clamped by their min and max widths
        // (css-flexbox-1 §9.9.1, §9.2 step 3.E "treating auto as fit-content", §9.4 step 7).
        // Taffy lays it out at the sum of its items' flex base sizes: wider than the available
        // space where hooke shrinks the items, and ignoring the items' max widths (cephal's
        // `TAFFY_DIVERGENCES.md`, "shrink-to-fit and intrinsic widths of row containers"; fixture
        // `chrome_truth/root_flex_width_max_content`). Give it a width.
        if row {
            style = style.with_size(Length::Px(60.0 + rng.below(300) as f32), style.height());
        } else {
            // A content-sized column container with a `max-width`. hooke, like Chrome, resolves
            // the container's width first, `max-width` included, and lays its items out
            // against it, so an unstretched item wider than that wraps to fit (fixture
            // `flex/wrap_row_max_width_indefinite_width`); Taffy lays the items out against the
            // space available to the container, unclamped.
            style = style.with_max_size(Length::None, style.max_height());
        }
    }
    if style.box_sizing() == BoxSizing::BorderBox {
        // §9.7 step 4.d with a border box: hooke, like Chrome, floors an item's flexed content
        // box at zero, so its border box never goes below its padding and border, and counts
        // that as a min violation for the other items to absorb (css-flexbox-1 §9.7 step 4,
        // "the target main size ... floored at zero" of the content box). Taffy floors the
        // border box at zero and lets the item overflow its target. An explicit min size of
        // the padding and border is a real min violation in both engines; with `content-box`
        // the zero min size already means that.
        let pb = Length::Px(2.0 * (px(style.padding().left) + style.border().left));
        style = style.with_min_size(pb, pb);
    }
    let intrinsic_width = parent.is_some_and(|p| {
        (!p.column || p.intrinsic_width)
            && (style.width().is_auto() || (matches!(style.width(), Length::Percent(_)) && p.intrinsic_width))
    });
    let me = Parent {
        column: !row,
        align_items: style.align_items(),
        definite_width: matches!(style.width(), Length::Px(_)) || (style.width().is_auto() && width_from_parent),
        intrinsic_row: row && intrinsic_width,
        intrinsic_width,
    };
    let n = 1 + rng.below(4) as usize;
    let kids: Vec<_> = (0..n).map(|_| random_tree(rng, depth - 1, tag, Some(me))).collect();
    view(t, style, kids)
}

fn px(l: Length<f32>) -> f32 {
    match l {
        Length::Px(v) => v,
        _ => 0.0,
    }
}

#[test]
fn matches_taffy_on_hand_written_cases() {
    // Row with grow, padding and gap.
    let a = text(
        1,
        Style::web()
            .with_min_size(Length::zero(), Length::zero())
            .with_flex_grow(1.0),
        20.0,
        10.0,
    );
    let b = text(
        2,
        Style::web()
            .with_min_size(Length::zero(), Length::zero())
            .with_flex_grow(2.0),
        20.0,
        30.0,
    );
    let mut root = view(
        0,
        Style::web()
            .with_size(Length::Px(300.0), Length::Px(100.0))
            .with_padding(Edges::all(Length::Px(10.0)))
            .with_gap(Length::zero(), Length::Px(5.0)),
        vec![a, b],
    );
    compare(&mut root, 0);

    // Column, centered, shrinking.
    let a = text(
        1,
        Style::web().with_min_size(Length::zero(), Length::zero()),
        50.0,
        80.0,
    );
    let b = text(
        2,
        Style::web()
            .with_min_size(Length::zero(), Length::zero())
            .with_flex_shrink(2.0),
        50.0,
        80.0,
    );
    let mut root = view(
        0,
        Style::web()
            .with_flex_direction(FlexDirection::Column)
            .with_align_items(AlignItems::Center)
            .with_size(Length::Px(200.0), Length::Px(100.0)),
        vec![a, b],
    );
    compare(&mut root, 0);

    // The root is content-sized, so its width is its single item's max-content contribution:
    // the item's max-content width clamped by its `max-width`, plus its margins
    // (css-sizing-3 §5.1; css-flexbox-1 §9.9.3 clamps the size, never the margins).
    // Chrome: item 46 wide at x = 3, root 52.
    let leaf = text(
        3,
        Style::web().with_min_size(Length::zero(), Length::zero()),
        87.0,
        10.0,
    );
    let item = view(
        2,
        Style::web()
            .with_min_size(Length::zero(), Length::zero())
            .with_max_size(Length::Px(46.0), Length::None)
            .with_margin(Edges::all(Length::Px(3.0))),
        vec![leaf],
    );
    let mut root = view(1, Style::web(), vec![item]);
    compare(&mut root, 0);

    // As above with an item that cannot shrink: its flex base size (87) bounds its
    // contribution from below, and its `max-width` then clamps it (css-flexbox-1 §9.9.3,
    // "clamped by its flex base size as a minimum (if it is not shrinkable), and then further
    // clamped by its min/max main size"). Chrome: root 46.
    let leaf = text(
        3,
        Style::web().with_min_size(Length::zero(), Length::zero()),
        87.0,
        10.0,
    );
    let item = view(
        2,
        Style::web()
            .with_min_size(Length::zero(), Length::zero())
            .with_flex_shrink(0.0)
            .with_max_size(Length::Px(46.0), Length::None),
        vec![leaf],
    );
    let mut root = view(1, Style::web(), vec![item]);
    compare(&mut root, 0);
}

#[test]
fn matches_taffy_on_random_trees() {
    let only: Option<u64> = std::env::var("ORACLE_SEED").ok().and_then(|v| v.parse().ok());
    let mut failures = Vec::new();
    for seed in 1..=SEEDS {
        if only.is_some_and(|o| o != seed) {
            continue;
        }
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut tag = 0;
        let mut root = random_tree(&mut rng, 3, &mut tag, None);
        if let Err(e) = try_compare(&mut root, seed, only.is_some()) {
            failures.push(e);
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {SEEDS} seeds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

const SEEDS: u64 = 1000;
