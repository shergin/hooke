//! Differential testing against Taffy 0.15: the same random tree is laid out by both engines
//! and every frame is compared. Taffy is a dev-dependency only.
//!
//! The random trees stay inside what the provisional algorithm implements: single line, no
//! `order`, no baselines, `min-size` set to `0` on items so Taffy's automatic minimum does not
//! apply, `position: relative`, and text leaves whose measure function is mirrored exactly.

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
    taffy::geometry::Rect { left: to_taffy_lpa(e.left), right: to_taffy_lpa(e.right), top: to_taffy_lpa(e.top), bottom: to_taffy_lpa(e.bottom) }
}
fn edges_lp(e: Edges<Length<f32>>) -> taffy::geometry::Rect<taffy::style::LengthPercentage> {
    taffy::geometry::Rect { left: to_taffy_lp(e.left), right: to_taffy_lp(e.right), top: to_taffy_lp(e.top), bottom: to_taffy_lp(e.bottom) }
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
        size: taffy::geometry::Size { width: to_taffy_length(s.width()), height: to_taffy_length(s.height()) },
        min_size: taffy::geometry::Size { width: to_taffy_lpa(s.min_width()), height: to_taffy_lpa(s.min_height()) },
        max_size: taffy::geometry::Size { width: to_taffy_lpa(s.max_width()), height: to_taffy_lpa(s.max_height()) },
        margin: edges_lpa(s.margin()),
        padding: edges_lp(s.padding()),
        border: taffy::geometry::Rect {
            left: t::LengthPercentage::length(border.left),
            right: t::LengthPercentage::length(border.right),
            top: t::LengthPercentage::length(border.top),
            bottom: t::LengthPercentage::length(border.bottom),
        },
        gap: taffy::geometry::Size { width: to_taffy_lp(s.column_gap()), height: to_taffy_lp(s.row_gap()) },
        justify_content: match s.justify_content() {
            JustifyContent::Normal => t::JustifyContent::NORMAL,
            JustifyContent::FlexStart | JustifyContent::Start | JustifyContent::Stretch => t::JustifyContent::FLEX_START,
            JustifyContent::FlexEnd | JustifyContent::End => t::JustifyContent::FLEX_END,
            JustifyContent::Center => t::JustifyContent::CENTER,
            JustifyContent::SpaceBetween => t::JustifyContent::SPACE_BETWEEN,
            JustifyContent::SpaceAround => t::JustifyContent::SPACE_AROUND,
            JustifyContent::SpaceEvenly => t::JustifyContent::SPACE_EVENLY,
        },
        align_items: match s.align_items() {
            AlignItems::Normal => t::AlignItems::NORMAL,
            AlignItems::Stretch => t::AlignItems::STRETCH,
            AlignItems::FlexStart | AlignItems::Start | AlignItems::SelfStart => t::AlignItems::FLEX_START,
            AlignItems::FlexEnd | AlignItems::End | AlignItems::SelfEnd => t::AlignItems::FLEX_END,
            AlignItems::Center => t::AlignItems::CENTER,
            AlignItems::Baseline => t::AlignItems::BASELINE,
        },
        align_self: s.align_self().map(|a| match a {
            AlignItems::Normal => t::AlignSelf::NORMAL,
            AlignItems::Stretch => t::AlignSelf::STRETCH,
            AlignItems::FlexStart | AlignItems::Start | AlignItems::SelfStart => t::AlignSelf::FLEX_START,
            AlignItems::FlexEnd | AlignItems::End | AlignItems::SelfEnd => t::AlignSelf::FLEX_END,
            AlignItems::Center => t::AlignSelf::CENTER,
            AlignItems::Baseline => t::AlignSelf::BASELINE,
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

fn taffy_frames(tree: &TaffyTree<Size<f32>>, node: NodeId, origin: [f32; 2], out: &mut Vec<[f32; 4]>) {
    let l = tree.layout(node).unwrap();
    let abs = [origin[0] + l.location.x, origin[1] + l.location.y];
    out.push([abs[0], abs[1], l.size.width, l.size.height]);
    for child in tree.children(node).unwrap() {
        taffy_frames(tree, child, abs, out);
    }
}

fn compare(root: &mut Handle<Node>, seed: u64) {
    let (ours, _, _) = run(root, Size::new(800.0, 600.0), None);
    let our_frames: Vec<[f32; 4]> = frames(root).into_iter().map(|(_, f)| f).collect();

    let mut tree: TaffyTree<Size<f32>> = TaffyTree::new();
    tree.disable_rounding();
    let taffy_root = mirror(root, &mut tree);
    tree.compute_layout_with_measure(
        taffy_root,
        taffy::geometry::Size { width: AvailableSpace::Definite(800.0), height: AvailableSpace::Definite(600.0) },
        |input, _id, ctx, style| {
            // Taffy hands the whole leaf to this closure, so go through `compute_leaf_layout`:
            // the leaf's own size, min/max, padding and border then apply as they do in ours
            // (returning the measured size directly ignored e.g. a leaf root's max-width).
            let intrinsic = *ctx.expect("leaves carry their intrinsic size");
            taffy::compute_leaf_layout(input, style, |_, _| 0.0, |known, available| {
                let known = Size::new(known.width.into(), known.height.into());
                let available = Size::new(from_taffy_available(available.width), from_taffy_available(available.height));
                let m = measure_text(intrinsic, hooke::MeasureInput { known, available });
                taffy::geometry::Size { width: m.size.width, height: m.size.height }
            })
        },
    )
    .unwrap();
    // The root's own margin places it inside the viewport in Taffy; our root has no parent to
    // hold an offset, so compare frames relative to the root's border box.
    let root_location = tree.layout(taffy_root).unwrap().location;
    let mut theirs = Vec::new();
    taffy_frames(&tree, taffy_root, [-root_location.x, -root_location.y], &mut theirs);
    let root_layout = tree.layout(taffy_root).unwrap();
    assert!(
        (ours.width - root_layout.size.width).abs() < 0.01 && (ours.height - root_layout.size.height).abs() < 0.01,
        "seed {seed}: root size ours {ours:?} taffy {:?}",
        root_layout.size
    );
    assert_eq!(our_frames.len(), theirs.len());
    for (i, (a, b)) in our_frames.iter().zip(theirs.iter()).enumerate() {
        for k in 0..4 {
            if (a[k] - b[k]).abs() >= 0.01 {
                dump_tree(root, 0);
                eprintln!("taffy frames: {theirs:?}");
                panic!("seed {seed}: node #{i} differs: ours {a:?} taffy {b:?}");
            }
        }
    }
}

fn from_taffy_available(a: AvailableSpace) -> hooke::Available<f32> {
    match a {
        AvailableSpace::Definite(v) => hooke::Available::Definite(v),
        AvailableSpace::MinContent => hooke::Available::MinContent,
        AvailableSpace::MaxContent => hooke::Available::MaxContent,
    }
}

/// Items get `min-size: 0` so Taffy's content-based automatic minimum, which the provisional
/// algorithm does not implement, never applies.
fn item_style(rng: &mut Rng) -> Style {
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
        s = s.with_align_self(Some(rng.pick(&[AlignItems::FlexStart, AlignItems::Center, AlignItems::FlexEnd, AlignItems::Stretch])));
    }
    if rng.chance(25) {
        s = s.with_max_size(Length::Px(30.0 + rng.below(100) as f32), Length::None);
    }
    s
}

fn container_style(rng: &mut Rng, base: Style) -> Style {
    let mut s = base
        .with_flex_direction(rng.pick(&[FlexDirection::Row, FlexDirection::Column, FlexDirection::RowReverse, FlexDirection::ColumnReverse]))
        .with_justify_content(rng.pick(&[
            JustifyContent::FlexStart,
            JustifyContent::FlexEnd,
            JustifyContent::Center,
            JustifyContent::SpaceBetween,
            JustifyContent::SpaceAround,
            JustifyContent::SpaceEvenly,
        ]))
        .with_align_items(rng.pick(&[AlignItems::Stretch, AlignItems::FlexStart, AlignItems::Center, AlignItems::FlexEnd]))
        .with_padding(Edges::all(Length::Px(rng.below(8) as f32)))
        .with_border(Edges::all(rng.below(3) as f32))
        .with_gap(Length::Px(rng.below(6) as f32), Length::Px(rng.below(6) as f32))
        .with_box_sizing(rng.pick(&[BoxSizing::ContentBox, BoxSizing::BorderBox]));
    if rng.chance(40) {
        s = s.with_size(Length::Px(60.0 + rng.below(300) as f32), Length::Auto);
    }
    if rng.chance(30) {
        s = s.with_size(s.width(), Length::Px(30.0 + rng.below(200) as f32));
    }
    if rng.chance(20) {
        s = s.with_size(Length::Percent(0.25 + rng.below(50) as f32 / 100.0), s.height());
    }
    s
}

fn random_tree(rng: &mut Rng, depth: u32, tag: &mut u32) -> Handle<Node> {
    *tag += 1;
    let t = *tag;
    if depth == 0 || rng.chance(30) {
        return text(t, item_style(rng), 10.0 + rng.below(90) as f32, 8.0 + rng.below(30) as f32);
    }
    let n = 1 + rng.below(4) as usize;
    let kids: Vec<_> = (0..n).map(|_| random_tree(rng, depth - 1, tag)).collect();
    let base = item_style(rng);
    view(t, container_style(rng, base), kids)
}

#[test]
fn matches_taffy_on_hand_written_cases() {
    // Row with grow, padding and gap.
    let a = text(1, Style::web().with_min_size(Length::zero(), Length::zero()).with_flex_grow(1.0), 20.0, 10.0);
    let b = text(2, Style::web().with_min_size(Length::zero(), Length::zero()).with_flex_grow(2.0), 20.0, 30.0);
    let mut root = view(
        0,
        Style::web().with_size(Length::Px(300.0), Length::Px(100.0)).with_padding(Edges::all(Length::Px(10.0))).with_gap(Length::zero(), Length::Px(5.0)),
        vec![a, b],
    );
    compare(&mut root, 0);

    // Column, centered, shrinking.
    let a = text(1, Style::web().with_min_size(Length::zero(), Length::zero()), 50.0, 80.0);
    let b = text(2, Style::web().with_min_size(Length::zero(), Length::zero()).with_flex_shrink(2.0), 50.0, 80.0);
    let mut root = view(
        0,
        Style::web()
            .with_flex_direction(FlexDirection::Column)
            .with_align_items(AlignItems::Center)
            .with_size(Length::Px(200.0), Length::Px(100.0)),
        vec![a, b],
    );
    compare(&mut root, 0);
}

#[test]
fn matches_taffy_on_random_trees() {
    for seed in 1..=150u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut tag = 0;
        let mut root = random_tree(&mut rng, 3, &mut tag);
        compare(&mut root, seed);
    }
}
