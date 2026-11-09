//! Flex lines: breaking (§9.3), resolving flexible lengths (§9.7) and cross sizes (§9.4).

use super::items::{ask, child_content_box, cross_constraint, resolve_auto_min, Item};
use super::{count, transfer_bb, Ctx, F_AUTO_MIN, F_BASELINE, F_FROZEN, F_SCROLL, F_STRETCHED};
use crate::geometry::{Axis, Size};
use crate::measure::{Available, Measure};
use crate::pass::{Cancelled, Pass};
use crate::scalar::{Opt, Scalar};
use crate::style::{AlignContent, Unit};
use crate::tree::LayoutNode;
use alloc::vec::Vec;
use core::ops::Range;

/// One flex line: a contiguous run of the item list plus its cross-axis results.
#[derive(Copy, Clone)]
pub(crate) struct Line<S: Scalar> {
    pub start: u32,
    pub end: u32,
    /// Used cross size of the line.
    pub cross: S,
    /// Largest distance from a baseline-aligned item's outer cross-start edge to its baseline.
    pub max_ascent: S,
    /// Largest distance from a baseline-aligned item's baseline to its outer cross-end edge.
    pub max_descent: S,
    /// Logical cross offset of the line within the container's content box.
    pub offset_cross: S,
}

impl<S: Scalar> Line<S> {
    #[inline]
    pub fn range(&self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
    #[inline]
    pub fn len(&self) -> usize {
        (self.end - self.start) as usize
    }
}

/// §9.3 step 5: breaks the items into lines against the container's inner main size, using
/// outer hypothetical main sizes. A single-line container, or one with no items, yields at most
/// one line.
pub(crate) fn collect<S: Scalar>(ctx: &Ctx<S>, items: &[Item<S>], lines: &mut Vec<Line<S>>, inner_main: S) {
    let main = ctx.main();
    let n = items.len();
    if n == 0 {
        return;
    }
    let line = |start: u32, end: u32| Line { start, end, cross: S::ZERO, max_ascent: S::ZERO, max_descent: S::ZERO, offset_cross: S::ZERO };
    if !ctx.flow.multiline {
        lines.push(line(0, n as u32));
        return;
    }
    let mut start = 0usize;
    while start < n {
        let mut length = S::ZERO;
        let mut end = start;
        while end < n {
            let outer = items[end].hypothetical + items[end].margin.sum(main);
            let with = length + outer + if end > start { ctx.gap_main } else { S::ZERO };
            if end > start && with > inner_main {
                break;
            }
            length = with;
            end += 1;
        }
        lines.push(line(start as u32, end as u32));
        start = end;
    }
}

/// The outer hypothetical length of a run of items, gaps included.
pub(crate) fn line_length<S: Scalar>(ctx: &Ctx<S>, items: &[Item<S>]) -> S {
    let main = ctx.main();
    let n = items.len();
    let gaps = if n > 1 { ctx.gap_main * count(n - 1) } else { S::ZERO };
    items.iter().fold(gaps, |acc, it| acc + it.hypothetical + it.margin.sum(main))
}

/// §9.7: resolves the flexible lengths of one line, writing each item's `target`.
///
/// Free space is handed out in Blink's formulation: each unfrozen item takes its share of the
/// space *remaining* after the items before it, so the last item absorbs any rounding and the
/// targets always sum to the line length when nothing is clamped.
pub(crate) fn resolve_flexible_lengths<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    items: &mut [Item<S>],
    paths: &[u32],
    inner_main: S,
) -> Result<(), Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let main = ctx.main();
    let n = items.len();
    if n == 0 {
        return Ok(());
    }
    let gaps = if n > 1 { ctx.gap_main * count(n - 1) } else { S::ZERO };

    // 1. Used flex factor.
    let hypothetical_sum = items.iter().fold(S::ZERO, |acc, it| acc + it.hypothetical + it.margin.sum(main));
    let used = gaps + hypothetical_sum;
    let growing = used < inner_main;
    let shrinking = used > inner_main;

    // 2. Inflexible items.
    for it in items.iter_mut() {
        it.target = it.hypothetical;
        it.violation = 0;
        let frozen = (!growing && !shrinking)
            || (it.grow == S::ZERO && it.shrink == S::ZERO)
            || (growing && it.base > it.hypothetical)
            || (shrinking && it.base < it.hypothetical);
        it.set(F_FROZEN, frozen);
    }
    if !growing && !shrinking {
        return Ok(());
    }
    // A shrinking line needs the deferred automatic minimums of the items that can shrink.
    if shrinking {
        for it in items.iter_mut().filter(|it| !it.has(F_FROZEN) && it.has(F_AUTO_MIN)) {
            resolve_auto_min(pass, node, ctx, it, paths)?;
        }
    }

    // 3. Initial free space.
    let used_space = |items: &[Item<S>]| {
        items.iter().fold(gaps, |acc, it| {
            acc + if it.has(F_FROZEN) { it.target } else { it.base } + it.margin.sum(main)
        })
    };
    let initial_free = inner_main - used_space(items);

    // 4. Loop. Each round freezes at least one item, so `n` rounds suffice.
    for _ in 0..=n {
        if items.iter().all(|it| it.has(F_FROZEN)) {
            break;
        }
        // b. Remaining free space, with the "sum of factors below one" rule.
        let mut remaining = inner_main - used_space(items);
        let factor_sum = items
            .iter()
            .filter(|it| !it.has(F_FROZEN))
            .fold(S::ZERO, |acc, it| acc + if growing { it.grow } else { it.shrink });
        if factor_sum < S::ONE {
            let scaled = initial_free * factor_sum;
            if scaled.abs() < remaining.abs() {
                remaining = scaled;
            }
        }
        // c. Distribute.
        if remaining != S::ZERO && factor_sum > S::ZERO {
            if growing {
                let mut rem = remaining;
                let mut fsum = factor_sum;
                for it in items.iter_mut().filter(|it| !it.has(F_FROZEN)) {
                    let share = if fsum > S::ZERO { rem.mul_div(it.grow, fsum) } else { S::ZERO };
                    it.target = it.base + share;
                    rem = rem - share;
                    fsum = fsum - it.grow;
                }
            } else {
                let scaled = |it: &Item<S>| it.shrink * it.inner_base(main);
                let mut ssum = items.iter().filter(|it| !it.has(F_FROZEN)).fold(S::ZERO, |acc, it| acc + scaled(it));
                let mut rem = remaining.abs();
                for it in items.iter_mut().filter(|it| !it.has(F_FROZEN)) {
                    let s = scaled(it);
                    let share = if ssum > S::ZERO { rem.mul_div(s, ssum) } else { S::ZERO };
                    it.target = it.base - share;
                    rem = rem - share;
                    ssum = ssum - s;
                }
            }
        }
        // d. Min/max violations, flooring the content box at zero.
        let mut total_violation = S::ZERO;
        for it in items.iter_mut().filter(|it| !it.has(F_FROZEN)) {
            let clamped = it.target.clamp_opt(Opt::some(it.min_main), it.max.get(main)).max(it.pb.get(main));
            let v = clamped - it.target;
            it.violation = if v > S::ZERO {
                1
            } else if v < S::ZERO {
                -1
            } else {
                0
            };
            total_violation = total_violation + v;
            it.target = clamped;
        }
        // e. Freeze over-flexed items.
        for it in items.iter_mut().filter(|it| !it.has(F_FROZEN)) {
            let freeze = if total_violation > S::ZERO {
                it.violation > 0
            } else if total_violation < S::ZERO {
                it.violation < 0
            } else {
                true
            };
            if freeze {
                it.set(F_FROZEN, true);
            }
        }
    }
    Ok(())
}

/// §9.4 steps 7 to 11 and 15: hypothetical cross sizes, baselines, line cross sizes,
/// `align-content: stretch`, used cross sizes, and the container's inner cross size.
#[allow(clippy::too_many_arguments)]
pub(crate) fn cross_sizes<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    items: &mut [Item<S>],
    paths: &[u32],
    lines: &mut [Line<S>],
    inner_main: S,
) -> Result<S, Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let main = ctx.main();
    let cross = ctx.cross();
    let container_cross = ctx.inner.get(cross);
    let single_line_definite = !ctx.flow.multiline && container_cross.is_some();

    // 7. Hypothetical cross size of each item.
    for item in items.iter_mut() {
        let it = *item;
        let needs_baseline = it.has(F_BASELINE);
        if single_line_definite && it.has(F_STRETCHED) && !needs_baseline {
            continue;
        }
        let content_box = child_content_box(node, it.path(paths));
        let tmin = it.transferred_min(it.pb, content_box, cross);
        let tmax = it.transferred_max(it.pb, content_box, cross);
        let pb_cross = it.pb.get(cross);
        let known_cross = it.size.get(cross).map(|v| v.clamp_opt(tmin, tmax).max(pb_cross));
        let (_, cross_available, cross_definite) = cross_constraint(ctx, &it, content_box);
        let cross_available = match it.size_tag[cross.index()] {
            Unit::MinContent => Available::MinContent,
            Unit::MaxContent => Available::MaxContent,
            Unit::FitContent => match container_cross.get() {
                Some(v) => Available::Definite((v - it.margin.sum(cross)).max(S::ZERO)),
                None => cross_available,
            },
            _ => match cross_available {
                Available::Definite(v) => Available::Definite(v.max(pb_cross)),
                other => other,
            },
        };
        let main_available = Available::Definite((inner_main - it.margin.sum(main)).max(S::ZERO));
        let indefinite = definiteness(ctx, &it, cross_definite);
        let (used_cross, baseline) = match known_cross.get() {
            Some(c) => {
                // A definite cross size needs no measurement, unless the baseline is wanted.
                if needs_baseline {
                    let mut known = Size::new(Opt::NONE, Opt::NONE);
                    known.set(main, Opt::some(it.target));
                    known.set(cross, Opt::some(c));
                    let mut available = Size::new(Available::MaxContent, Available::MaxContent);
                    available.set(main, Available::Definite(it.target));
                    available.set(cross, Available::Definite(c));
                    let (_, b) = ask(pass, node, ctx, &it, paths, known, available, indefinite, false)?;
                    (c, b)
                } else {
                    (c, Opt::NONE)
                }
            }
            None => {
                let mut known = Size::new(Opt::NONE, Opt::NONE);
                known.set(main, Opt::some(it.target));
                let mut available = Size::new(Available::MaxContent, Available::MaxContent);
                available.set(main, main_available);
                available.set(cross, cross_available);
                let (measured, b) = ask(pass, node, ctx, &it, paths, known, available, indefinite, false)?;
                let mut c = measured.get(cross).clamp_opt(tmin, tmax).max(pb_cross);
                // An aspect ratio with an auto cross size: the ratio-dependent size is a floor.
                if let Some(r) = it.aspect_ratio.get() {
                    let via_ratio = transfer_bb(content_box, it.pb, main, it.target, r);
                    c = c.max(via_ratio.clamp_opt(tmin, tmax));
                }
                (c, b)
            }
        };
        item.cross = used_cross;
        item.baseline = baseline;
    }

    // 8. Cross size of each line.
    for line in lines.iter_mut() {
        if single_line_definite {
            line.cross = container_cross.unwrap_or(S::ZERO);
            // Baseline groups still matter for alignment within the line.
            let (a, d) = baseline_extents(ctx, &items[line.range()]);
            line.max_ascent = a;
            line.max_descent = d;
            continue;
        }
        let (max_ascent, max_descent) = baseline_extents(ctx, &items[line.range()]);
        let mut largest = if max_ascent + max_descent > S::ZERO { max_ascent + max_descent } else { S::ZERO };
        for it in &items[line.range()] {
            if !it.has(F_BASELINE) {
                largest = largest.max(it.cross + it.margin.sum(cross));
            }
        }
        line.cross = largest;
        line.max_ascent = max_ascent;
        line.max_descent = max_descent;
    }
    if !ctx.flow.multiline && !lines.is_empty() && !single_line_definite {
        lines[0].cross = lines[0].cross.clamp_opt(ctx.min_inner.get(cross), ctx.max_inner.get(cross));
    }

    // 9. `align-content: stretch`.
    if matches!(ctx.align_content, AlignContent::Normal | AlignContent::Stretch) && !lines.is_empty() {
        let target = container_cross.or(ctx.min_inner.get(cross)).map(|v| v.clamp_opt(ctx.min_inner.get(cross), ctx.max_inner.get(cross)));
        if let Some(t) = target.get() {
            let gaps = if lines.len() > 1 { ctx.gap_cross * count(lines.len() - 1) } else { S::ZERO };
            let total = lines.iter().fold(gaps, |acc, l| acc + l.cross);
            if total < t {
                let add = (t - total) / count(lines.len());
                for l in lines.iter_mut() {
                    l.cross = l.cross + add;
                }
            }
        }
    }

    // 11. Used cross size of each item.
    for line in lines.iter() {
        for it in &mut items[line.range()] {
            if it.has(F_STRETCHED) {
                it.cross = (line.cross - it.margin.sum(cross))
                    .max(S::ZERO)
                    .clamp_opt(it.min.get(cross), it.max.get(cross))
                    .max(it.pb.get(cross));
            }
        }
    }

    // 15. The container's inner cross size.
    let inner_cross = match container_cross.get() {
        Some(v) => v,
        None => {
            let gaps = if lines.len() > 1 { ctx.gap_cross * count(lines.len() - 1) } else { S::ZERO };
            let total = lines.iter().fold(gaps, |acc, l| acc + l.cross);
            total.clamp_opt(ctx.min_inner.get(cross), ctx.max_inner.get(cross))
        }
    };
    Ok(inner_cross)
}

/// The item's first baseline for alignment, below its *border-box* top: the measured one, or
/// the border-box bottom when it has none. A scroll container's baseline is clamped to its box.
pub(crate) fn inner_baseline<S: Scalar>(it: &Item<S>, cross_size: S) -> S {
    match it.baseline.get() {
        Some(b) if it.has(F_SCROLL) => b.min(cross_size).max(S::ZERO),
        Some(b) => b,
        None => cross_size,
    }
}

/// Ascent of a baseline-aligned item: margin-top plus its baseline.
#[inline]
pub(crate) fn ascent<S: Scalar>(it: &Item<S>) -> S {
    it.margin.top + inner_baseline(it, it.cross)
}

/// The largest ascent and descent among the baseline-aligned items of a line.
fn baseline_extents<S: Scalar>(ctx: &Ctx<S>, items: &[Item<S>]) -> (S, S) {
    let cross = ctx.cross();
    let mut a = S::ZERO;
    let mut d = S::ZERO;
    for it in items.iter().filter(|it| it.has(F_BASELINE)) {
        let asc = ascent(it);
        a = a.max(asc);
        d = d.max(it.cross + it.margin.sum(cross) - asc);
    }
    (a, d)
}

/// Which imposed sizes of an item are not percentage bases for its children (§9.8). Only a
/// height can be indefinite: an imposed width is a used inline size. A height is definite when
/// it is the item's own definite size, a stretch size, or a flexed size in a container whose
/// height is definite or with a definite flex basis.
pub(crate) fn definiteness<S: Scalar>(ctx: &Ctx<S>, it: &Item<S>, cross_definite: bool) -> Size<bool> {
    let main = ctx.main();
    let height_definite = if main == Axis::Vertical {
        (ctx.inner.height.is_some() && !ctx.indefinite_height) || it.has(super::F_BASIS_DEFINITE)
    } else {
        cross_definite || it.size.height.is_some() || it.has(F_STRETCHED)
    };
    Size::new(false, !height_definite)
}
