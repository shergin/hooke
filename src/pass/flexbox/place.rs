//! Placement: main-axis alignment (§9.5), cross-axis alignment (§9.6), the final layout of
//! every child, container baselines (§8.5) and absolutely positioned children (§4.1).

use super::items::{Item, ask};
use super::lines::{Line, definiteness, inner_baseline};
use super::{
    Ctx, F_BASELINE, F_BASIS_DEFINITE, F_RELATIVE, Flow, M_BOTTOM, M_LEFT, M_RIGHT, M_TOP, along, count,
    padding_border, resolve_bb, resolve_direction, transfer_bb, two,
};
use crate::geometry::{Axis, Edges, Point, Size};
use crate::measure::{Available, Measure};
use crate::pass::{Cancelled, Input, Pass};
use crate::scalar::{Opt, Scalar};
use crate::style::{AlignContent, AlignItems, BoxSizing, Direction, JustifyContent, Style, Unit, slot};
use crate::tree::LayoutNode;

/// A distribution keyword after css-align's fallbacks, in logical (flex-relative) terms.
#[derive(Copy, Clone, PartialEq)]
enum Distribute {
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// A content-distribution keyword as (flex-relative keyword, writing-mode `start`/`end`).
type Keyword = (Distribute, Option<bool>);

/// css-align's fallback alignment: with one subject or negative free space the distributed
/// keywords fall back. `space-between` (and `stretch`) become plain `flex-start`, so an
/// overflowing reversed line overflows past its main-start edge, as Chrome does (css-flexbox-1
/// §8.2: "identical to flex-start"). `space-around` and `space-evenly` become *safe* `center`,
/// which on overflow is the writing-mode `start`. `start`/`end` are writing-mode relative;
/// `start_is_flex_start` says whether that is the flex start on this axis.
fn resolve_distribution<S: Scalar>(kw: Keyword, free: S, start_is_flex_start: bool, n: usize) -> Distribute {
    let (mut k, start_end) = kw;
    let mut safe = false;
    if n <= 1 || free <= S::ZERO {
        (k, safe) = match k {
            Distribute::SpaceBetween => (Distribute::FlexStart, false),
            Distribute::SpaceAround | Distribute::SpaceEvenly => (Distribute::Center, true),
            other => (other, false),
        };
    }
    let ws = |start: bool| {
        if start == start_is_flex_start {
            Distribute::FlexStart
        } else {
            Distribute::FlexEnd
        }
    };
    if free <= S::ZERO && safe {
        return ws(true);
    }
    match start_end {
        Some(start) => ws(start),
        None => k,
    }
}

fn justify_keyword(j: JustifyContent) -> Keyword {
    match j {
        JustifyContent::Normal | JustifyContent::Stretch | JustifyContent::FlexStart => (Distribute::FlexStart, None),
        JustifyContent::FlexEnd => (Distribute::FlexEnd, None),
        JustifyContent::Center => (Distribute::Center, None),
        JustifyContent::SpaceBetween => (Distribute::SpaceBetween, None),
        JustifyContent::SpaceAround => (Distribute::SpaceAround, None),
        JustifyContent::SpaceEvenly => (Distribute::SpaceEvenly, None),
        JustifyContent::Start => (Distribute::FlexStart, Some(true)),
        JustifyContent::End => (Distribute::FlexEnd, Some(false)),
    }
}

fn align_content_keyword(a: AlignContent) -> Keyword {
    match a {
        AlignContent::Normal | AlignContent::Stretch | AlignContent::FlexStart => (Distribute::FlexStart, None),
        AlignContent::FlexEnd => (Distribute::FlexEnd, None),
        AlignContent::Center => (Distribute::Center, None),
        AlignContent::SpaceBetween => (Distribute::SpaceBetween, None),
        AlignContent::SpaceAround => (Distribute::SpaceAround, None),
        AlignContent::SpaceEvenly => (Distribute::SpaceEvenly, None),
        AlignContent::Start => (Distribute::FlexStart, Some(true)),
        AlignContent::End => (Distribute::FlexEnd, Some(false)),
    }
}

/// Leading offset and spacing between subjects for a distribution, in logical coordinates.
fn distribution<S: Scalar>(d: Distribute, free: S, n: usize) -> (S, S) {
    match d {
        Distribute::FlexStart => (S::ZERO, S::ZERO),
        Distribute::FlexEnd => (free, S::ZERO),
        Distribute::Center => (free / two(), S::ZERO),
        Distribute::SpaceBetween => (
            S::ZERO,
            if n > 1 {
                free.max(S::ZERO) / count(n - 1)
            } else {
                S::ZERO
            },
        ),
        Distribute::SpaceAround => {
            let each = free.max(S::ZERO) / count(n);
            (each / two(), each)
        }
        Distribute::SpaceEvenly => {
            let each = free.max(S::ZERO) / count(n + 1);
            (each, each)
        }
    }
}

/// §9.5 step 12: main-axis auto margins and `justify-content`, per line. Writes each item's
/// logical `offset_main` (of its margin box) and resolved auto margins.
pub(crate) fn distribute_main<S: Scalar>(ctx: &Ctx<S>, items: &mut [Item<S>], lines: &[Line<S>], inner_main: S) {
    let main = ctx.main();
    for line in lines {
        let n = line.len();
        let its = &mut items[line.range()];
        let gaps = if n > 1 { ctx.gap_main * count(n - 1) } else { S::ZERO };
        let used = its.iter().fold(gaps, |acc, it| acc + it.target + it.margin.sum(main));
        let mut free = inner_main - used;
        let auto_count: usize = its
            .iter()
            .map(|it| {
                let (a, b) = it.margin_auto(main);
                a as usize + b as usize
            })
            .sum();
        if free > S::ZERO && auto_count > 0 {
            let each = free / count(auto_count);
            for it in its.iter_mut() {
                let (a, b) = it.margin_auto(main);
                let (sa, sb) = edges_of(main);
                if a {
                    set_margin(it, sa, each);
                }
                if b {
                    set_margin(it, sb, each);
                }
            }
            free = S::ZERO;
        }
        let d = resolve_distribution(
            justify_keyword(ctx.justify),
            free,
            ctx.flow.start_is_flex_start(main),
            n,
        );
        let (lead, between) = distribution(d, free, n);
        let mut pos = lead;
        for it in its.iter_mut() {
            it.offset_main = pos;
            pos = pos + it.target + it.margin.sum(main) + ctx.gap_main + between;
        }
    }
}

#[inline]
fn edges_of(axis: Axis) -> (u8, u8) {
    match axis {
        Axis::Horizontal => (M_LEFT, M_RIGHT),
        Axis::Vertical => (M_TOP, M_BOTTOM),
    }
}

fn set_margin<S: Scalar>(it: &mut Item<S>, edge: u8, v: S) {
    match edge {
        M_LEFT => it.margin.left = v,
        M_RIGHT => it.margin.right = v,
        M_TOP => it.margin.top = v,
        _ => it.margin.bottom = v,
    }
}

/// §9.6 steps 13, 14 and 16: cross-axis auto margins, `align-self` within each line and
/// `align-content` across lines. Writes each item's logical `offset_cross` (of its margin box
/// within the line) and each line's logical `offset_cross`.
pub(crate) fn align_cross<S: Scalar>(ctx: &Ctx<S>, items: &mut [Item<S>], lines: &mut [Line<S>], inner_cross: S) {
    let cross = ctx.cross();
    let start_is_flex_start = ctx.flow.start_is_flex_start(cross);
    let n_lines = lines.len();
    // 16. Lines.
    let gaps = if n_lines > 1 {
        ctx.gap_cross * count(n_lines - 1)
    } else {
        S::ZERO
    };
    let used = lines.iter().fold(gaps, |acc, l| acc + l.cross);
    let free = inner_cross - used;
    let d = resolve_distribution(
        align_content_keyword(ctx.align_content),
        free,
        start_is_flex_start,
        n_lines,
    );
    let (lead, between) = distribution(d, free, n_lines);
    let mut pos = lead;
    for line in lines.iter_mut() {
        line.offset_cross = pos;
        pos = pos + line.cross + ctx.gap_cross + between;
    }
    // 13 and 14. Items within their line.
    let ws_start = |free: S| if start_is_flex_start { S::ZERO } else { free };
    let ws_end = |free: S| if start_is_flex_start { free } else { S::ZERO };
    for line in lines.iter() {
        let line_cross = line.cross;
        for it in &mut items[line.range()] {
            let outer = it.cross + it.margin.sum(cross);
            let free = line_cross - outer;
            let (a, b) = it.margin_auto(cross);
            let (sa, sb) = edges_of(cross);
            it.offset_cross = S::ZERO;
            if a && b {
                set_margin(it, sa, free / two());
                set_margin(it, sb, free / two());
            } else if a {
                set_margin(it, sa, free);
            } else if b {
                set_margin(it, sb, free);
            } else {
                it.offset_cross = match it.align {
                    AlignItems::FlexStart | AlignItems::Stretch | AlignItems::Normal => S::ZERO,
                    AlignItems::FlexEnd => free,
                    AlignItems::Center => free / two(),
                    AlignItems::Start | AlignItems::SelfStart => ws_start(free),
                    AlignItems::End | AlignItems::SelfEnd => ws_end(free),
                    AlignItems::Baseline => {
                        if it.has(F_BASELINE) {
                            let asc = super::lines::ascent(it);
                            if ctx.flow.wrap_reverse {
                                // The group packs against the cross-start, which is the bottom.
                                line.max_descent - (outer - asc)
                            } else {
                                line.max_ascent - asc
                            }
                        } else if ctx.main() == Axis::Vertical {
                            // No baseline alignment in the cross axis of a column container.
                            // Chrome packs the item at the physical start of its line, except in
                            // a single-line rtl container, where it packs at the physical end.
                            let physical_end = ctx.flow.axis_rtl(cross) && !ctx.flow.multiline;
                            let physical = if physical_end { free } else { S::ZERO };
                            if ctx.flow.cross_reversed {
                                free - physical
                            } else {
                                physical
                            }
                        } else {
                            S::ZERO
                        }
                    }
                };
            }
        }
    }
}

/// Physical origin of an item's border box within the container's border box.
fn item_origin<S: Scalar>(ctx: &Ctx<S>, it: &Item<S>, line: &Line<S>, inner_main: S, inner_cross: S) -> Point<S> {
    let main = ctx.main();
    let cross = ctx.cross();
    let (ms, _) = along(&it.margin, main, ctx.flow.main_reversed);
    let (cs, _) = along(&it.margin, cross, ctx.flow.cross_reversed);
    let lm = it.offset_main + ms;
    let lc = line.offset_cross + it.offset_cross + cs;
    let pm = if ctx.flow.main_reversed {
        inner_main - lm - it.target
    } else {
        lm
    };
    let pc = if ctx.flow.cross_reversed {
        inner_cross - lc - it.cross
    } else {
        lc
    };
    let (pb_main_start, _) = ctx.pb.along(main);
    let (pb_cross_start, _) = ctx.pb.along(cross);
    let mut p = Point::ZERO;
    p.set(main, pb_main_start + pm);
    p.set(cross, pb_cross_start + pc);
    if it.has(F_RELATIVE) {
        p = p.offset(it.inset);
    }
    p
}

/// Lays out every item at its used size and writes its offset into this container's list.
/// Returns the container's first baseline.
#[allow(clippy::too_many_arguments)]
pub(crate) fn final_layout<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    items: &mut [Item<S>],
    paths: &[u32],
    lines: &[Line<S>],
    inner_main: S,
    inner_cross: S,
    main_definite: bool,
) -> Result<Opt<S>, Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let main = ctx.main();
    let cross = ctx.cross();
    for line in lines {
        for idx in line.range() {
            let it = items[idx];
            let mut known = Size::new(Opt::NONE, Opt::NONE);
            known.set(main, Opt::some(it.target));
            known.set(cross, Opt::some(it.cross));
            let available = known.map(|k| Available::Definite(k.unwrap_or(S::ZERO)));
            let mut indefinite = definiteness(ctx, &it, false);
            if main == Axis::Vertical {
                indefinite.height = !(main_definite || it.has(F_BASIS_DEFINITE));
            }
            let (_, baseline) = ask(pass, node, ctx, &it, paths, known, available, indefinite, true)?;
            items[idx].baseline = baseline;
            let origin = item_origin(ctx, &it, line, inner_main, inner_cross);
            pass.set_offset_path(node, it.path(paths), origin);
        }
    }
    container_baseline(pass, node, ctx, items, paths, lines, inner_main, inner_cross, true)
}

/// §8.5: the container's first baseline, below its border-box top. From the visually first
/// line: the first baseline-aligned item of a row container, else the startmost item; synthesized
/// from an item's border-box bottom when it has none. None without items.
#[allow(clippy::too_many_arguments)]
pub(crate) fn container_baseline<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    items: &mut [Item<S>],
    paths: &[u32],
    lines: &[Line<S>],
    inner_main: S,
    inner_cross: S,
    laid_out: bool,
) -> Result<Opt<S>, Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let Some(line) = (if ctx.flow.wrap_reverse {
        lines.last()
    } else {
        lines.first()
    }) else {
        return Ok(Opt::NONE);
    };
    if line.len() == 0 {
        return Ok(Opt::NONE);
    }
    let idx = if ctx.main() == Axis::Horizontal {
        items[line.range()]
            .iter()
            .position(|it| it.has(F_BASELINE))
            .map(|p| line.start as usize + p)
            .unwrap_or(line.start as usize)
    } else if ctx.flow.main_reversed {
        line.end as usize - 1
    } else {
        line.start as usize
    };
    let it = items[idx];
    let origin = item_origin(ctx, &it, line, inner_main, inner_cross);
    // Without a final layout the baseline may still be unknown: ask for it.
    let baseline = if it.baseline.is_none() && !laid_out {
        let main = ctx.main();
        let cross = ctx.cross();
        let mut known = Size::new(Opt::NONE, Opt::NONE);
        known.set(main, Opt::some(it.target));
        known.set(cross, Opt::some(it.cross));
        let available = known.map(|k| Available::Definite(k.unwrap_or(S::ZERO)));
        let indefinite = definiteness(ctx, &it, false);
        let (_, b) = ask(pass, node, ctx, &it, paths, known, available, indefinite, false)?;
        items[idx].baseline = b;
        b
    } else {
        it.baseline
    };
    let mut probe = it;
    probe.baseline = baseline;
    let height = if ctx.cross() == Axis::Vertical {
        it.cross
    } else {
        it.target
    };
    Ok(Opt::some(origin.y + inner_baseline(&probe, height)))
}

/// The physical edge of the static-position rectangle an absolute child aligns to on one axis:
/// 0 start, 1 center, 2 end. `flex_start` is the flex-relative keyword's start, `ws` the
/// writing-mode one, `item_rtl` the item's own inline direction for `self-*`.
fn static_edge(flow: &Flow, axis: Axis, kind: StaticAlign, item_rtl: bool) -> u8 {
    let flex = |start: bool| if start ^ flow.reversed(axis) { 0 } else { 2 };
    let ws = |start: bool| if start ^ flow.axis_rtl(axis) { 0 } else { 2 };
    let own = |start: bool| {
        if start ^ (axis == Axis::Horizontal && item_rtl) {
            0
        } else {
            2
        }
    };
    match kind {
        StaticAlign::FlexStart => flex(true),
        StaticAlign::FlexEnd => flex(false),
        StaticAlign::Center => 1,
        StaticAlign::Start => ws(true),
        StaticAlign::End => ws(false),
        StaticAlign::SelfStart => own(true),
        StaticAlign::SelfEnd => own(false),
    }
}

#[derive(Copy, Clone)]
enum StaticAlign {
    FlexStart,
    FlexEnd,
    Center,
    Start,
    End,
    SelfStart,
    SelfEnd,
}

fn static_main_align(justify: JustifyContent) -> StaticAlign {
    match justify {
        JustifyContent::Normal | JustifyContent::Stretch | JustifyContent::FlexStart | JustifyContent::SpaceBetween => {
            StaticAlign::FlexStart
        }
        JustifyContent::FlexEnd => StaticAlign::FlexEnd,
        JustifyContent::Center | JustifyContent::SpaceAround | JustifyContent::SpaceEvenly => StaticAlign::Center,
        JustifyContent::Start => StaticAlign::Start,
        JustifyContent::End => StaticAlign::End,
    }
}

fn static_cross_align(align: AlignItems) -> StaticAlign {
    match align {
        AlignItems::Normal | AlignItems::Stretch | AlignItems::FlexStart => StaticAlign::FlexStart,
        AlignItems::FlexEnd => StaticAlign::FlexEnd,
        AlignItems::Center => StaticAlign::Center,
        AlignItems::Baseline | AlignItems::Start => StaticAlign::Start,
        AlignItems::End => StaticAlign::End,
        AlignItems::SelfStart => StaticAlign::SelfStart,
        AlignItems::SelfEnd => StaticAlign::SelfEnd,
    }
}

/// §4.1: absolutely positioned children, against this container's padding box as their
/// containing block, with the static-position rectangle for insets that are `auto`.
pub(crate) fn absolute_children<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    absolute: &[u32],
    paths: &[u32],
    size: Size<S>,
) -> Result<(), Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    if absolute.is_empty() {
        return Ok(());
    }
    // The containing block is the padding box less any scrollbar gutter.
    let cb_inset = ctx.border_gutter;
    let cb = Size::new(
        (size.width - cb_inset.left - cb_inset.right).max(S::ZERO),
        (size.height - cb_inset.top - cb_inset.bottom).max(S::ZERO),
    );
    let cb_opt = cb.map(Opt::some);
    // The static-position rectangle is the content box.
    let content_origin = Point::new(ctx.pb.left, ctx.pb.top);
    let content_size = Size::new(
        (size.width - ctx.pb.sums().width).max(S::ZERO),
        (size.height - ctx.pb.sums().height).max(S::ZERO),
    );

    for pair in absolute.as_chunks::<2>().0 {
        let path = &paths[pair[0] as usize..(pair[0] + pair[1]) as usize];
        let cs: Style<S> = {
            let mut n: &N = node;
            for &i in path {
                n = &n.parts().children[i as usize].node;
            }
            *n.parts().style
        };
        let direction = resolve_direction(cs.direction(), ctx.flow.direction);
        let item_rtl = direction == Direction::Rtl;
        let content_box = cs.box_sizing() == BoxSizing::ContentBox;
        let pb = padding_border(&cs, cb_opt.width).sums();
        let margin = cs.resolve_edges(slot::MARGIN, cb_opt.width);
        let ml = cs.margin();
        let auto = Edges {
            left: ml.left.is_auto(),
            right: ml.right.is_auto(),
            top: ml.top.is_auto(),
            bottom: ml.bottom.is_auto(),
        };
        let inset_len = cs.inset();
        let inset = Edges {
            left: inset_len.left.resolve(cb_opt.width),
            right: inset_len.right.resolve(cb_opt.width),
            top: inset_len.top.resolve(cb_opt.height),
            bottom: inset_len.bottom.resolve(cb_opt.height),
        };
        let ratio = cs.aspect_ratio();

        // Style sizes, min and max, border-box; min and max transfer through the ratio.
        let mut sized = Size::new(Opt::NONE, Opt::NONE);
        let mut tags = [Unit::Auto; 2];
        let mut min = Size::new(Opt::NONE, Opt::NONE);
        let mut max = Size::new(Opt::NONE, Opt::NONE);
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let (size_slot, min_slot, max_slot) = slot::sizing(axis);
            let (v, tag) = resolve_bb(&cs, size_slot, cb_opt.get(axis), pb.get(axis));
            sized.set(axis, v);
            tags[axis.index()] = tag;
            min.set(axis, resolve_bb(&cs, min_slot, cb_opt.get(axis), pb.get(axis)).0);
            max.set(axis, resolve_bb(&cs, max_slot, cb_opt.get(axis), pb.get(axis)).0);
        }
        if let Some(r) = ratio.get() {
            for axis in [Axis::Horizontal, Axis::Vertical] {
                let other = axis.cross();
                if min.get(axis).is_none()
                    && let Some(v) = min.get(other).get()
                {
                    min.set(axis, Opt::some(transfer_bb(content_box, pb, other, v, r)));
                }
                if max.get(axis).is_none()
                    && let Some(v) = max.get(other).get()
                {
                    max.set(axis, Opt::some(transfer_bb(content_box, pb, other, v, r)));
                }
            }
        }
        let clamp = |axis: Axis, v: S| v.clamp_opt(min.get(axis), max.get(axis)).max(pb.get(axis));

        // Per axis: the available space (CSS 2 §10.3.7) and the size when the style or both
        // insets decide it; `stretch` fills the available space.
        let mut decided = Size::new(Opt::NONE, Opt::NONE);
        let mut available = Size::new(Available::MaxContent, Available::MaxContent);
        let mut from_insets = Size::new(false, false);
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let (s, e) = inset.along(axis);
            let (ma, mb) = margin.along(axis);
            let (aa, ab) = auto.along(axis);
            let margins = if aa { S::ZERO } else { ma } + if ab { S::ZERO } else { mb };
            let avail = (cb.get(axis) - s.unwrap_or(S::ZERO) - e.unwrap_or(S::ZERO) - margins).max(S::ZERO);
            let v = match sized.get(axis).get() {
                Some(v) => Opt::some(v),
                None => match tags[axis.index()] {
                    Unit::Stretch => Opt::some(avail),
                    Unit::MinContent => {
                        available.set(axis, Available::MinContent);
                        Opt::NONE
                    }
                    Unit::MaxContent => {
                        available.set(axis, Available::MaxContent);
                        Opt::NONE
                    }
                    _ => match (s.get(), e.get()) {
                        (Some(_), Some(_)) => {
                            from_insets.set(axis, true);
                            Opt::some(avail)
                        }
                        _ => Opt::NONE,
                    },
                },
            };
            if !matches!(tags[axis.index()], Unit::MinContent | Unit::MaxContent) {
                available.set(axis, Available::Definite(avail));
            }
            decided.set(axis, v.map(|v| clamp(axis, v)));
        }
        // With a preferred aspect ratio the height is the ratio-dependent axis unless the style
        // gives it directly: a width transfers to it, overriding insets.
        if let Some(r) = ratio.get() {
            if sized.height.is_none()
                && let Some(w) = decided.width.get()
            {
                decided.height = Opt::some(clamp(
                    Axis::Vertical,
                    transfer_bb(content_box, pb, Axis::Horizontal, w, r),
                ));
            } else if sized.width.is_none()
                && !from_insets.width
                && let Some(h) = decided.height.get()
            {
                decided.width = Opt::some(clamp(
                    Axis::Horizontal,
                    transfer_bb(content_box, pb, Axis::Vertical, h, r),
                ));
            }
        }
        let used: Size<S> = if decided.width.is_some() && decided.height.is_some() {
            decided.map(|v| v.unwrap_or(S::ZERO))
        } else {
            let input = Input::new(decided, available, cb_opt, ctx.flow.direction).content_size();
            let (m, _) = pass.visit_path(node, path, input)?;
            let mut used = Size::from_fn(|axis| match decided.get(axis).get() {
                Some(v) => v,
                None => clamp(axis, m.get(axis)),
            });
            if let Some(r) = ratio.get() {
                if decided.width.is_none() && decided.height.is_none() {
                    // Both content-based: the height follows the width through the ratio, but
                    // not below its content.
                    let via = transfer_bb(content_box, pb, Axis::Horizontal, used.width, r);
                    used.height = clamp(Axis::Vertical, used.height.max(via));
                } else if decided.width.is_none() {
                    let via = transfer_bb(content_box, pb, Axis::Vertical, used.height, r);
                    used.width = clamp(Axis::Horizontal, via);
                }
            }
            used
        };
        let final_input = Input::new(
            used.map(Opt::some),
            used.map(Available::Definite),
            cb_opt,
            ctx.flow.direction,
        )
        .content_size()
        .finalize();
        pass.visit_path(node, path, final_input)?;

        // Position per axis.
        let mut origin = Point::ZERO;
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let (s, e) = inset.along(axis);
            let (mut ma, mut mb) = margin.along(axis);
            let (aa, ab) = auto.along(axis);
            let extent = used.get(axis);
            let (cb_start, _) = cb_inset.along(axis);
            let pos = match (s.get(), e.get()) {
                (Some(s), Some(e)) => {
                    // Auto margins absorb the remaining space between definite insets.
                    let free =
                        cb.get(axis) - s - e - extent - if aa { S::ZERO } else { ma } - if ab { S::ZERO } else { mb };
                    if aa && ab {
                        ma = free / two();
                        mb = free / two();
                    } else if aa {
                        ma = free;
                    } else if ab {
                        mb = free;
                    }
                    if axis == Axis::Horizontal && item_rtl && !(aa || ab) {
                        // Over-constrained: in rtl the `left` inset gives way.
                        cb.get(axis) - e - mb - extent
                    } else {
                        s + ma
                    }
                }
                (Some(s), None) => s + ma,
                (None, Some(e)) => cb.get(axis) - e - mb - extent,
                (None, None) => {
                    // Static position: as the sole item of the container, auto margins as zero.
                    let outer = extent + ma + mb;
                    let free = content_size.get(axis) - outer;
                    let kind = if axis == ctx.main() {
                        static_main_align(ctx.justify)
                    } else {
                        static_cross_align(cs.align_self().unwrap_or(ctx.align_items))
                    };
                    let off = match static_edge(&ctx.flow, axis, kind, item_rtl) {
                        0 => S::ZERO,
                        1 => free / two(),
                        _ => free,
                    };
                    content_origin.get(axis) - cb_start + off + ma
                }
            };
            origin.set(axis, cb_start + pos);
        }
        pass.set_offset_path(node, path, origin);
    }
    Ok(())
}
