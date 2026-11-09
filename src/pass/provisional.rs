//! The provisional container algorithm: single-line flexbox over the storage layer.
//!
//! This exists so that Phase 0 can exercise the storage layer, the invalidation rules and the
//! commit protocol on realistic trees, and compare them against Taffy. It implements the spec's
//! line-length, flexible-length, cross-size and alignment steps for **one line** and ignores:
//! wrapping, `order`, baselines, the automatic minimum size (`min-size: auto` acts as `0`),
//! aspect ratio, `display: contents` flattening, percentage gaps against indefinite sizes, and
//! auto margins (they act as `0`). Absolutely positioned children are placed against their
//! parent's padding box only. The spec-complete algorithm replaces this file in Phase 1.
//!
//! Because Taffy is this algorithm's only oracle (`tests/taffy_oracle.rs`), three Taffy
//! behaviours that depart from CSS are mirrored on purpose and must not survive into Phase 1,
//! where Chrome fixtures become the arbiter: a main size imposed by the parent is used as is
//! even when it is smaller than padding plus border (CSS floors the content box at zero); an
//! item's margin sits inside the min/max clamp of its intrinsic contribution; and a size from
//! the style is not clamped by the node's own min/max when the question is for its content size.
//! Each is marked `TAFFY QUIRK` where it occurs.

#![allow(clippy::needless_range_loop)] // index loops re-borrow the pooled `items` around recursive calls

use super::{Cancelled, Input, Pass};
use crate::geometry::{Axis, Edges, Point, Size};
use crate::measure::{Available, Measure};
use crate::scalar::{Opt, Scalar};
use crate::style::{slot, AlignItems, BoxSizing, Direction, Display, JustifyContent, Length, Position, Style};
use crate::tree::LayoutNode;

/// Per-item working state. Lives in a pooled `Vec` in the scratch.
#[derive(Copy, Clone)]
pub(crate) struct Item<S: Scalar> {
    child: u32,
    margin: Edges<S>,
    grow: S,
    shrink: S,
    align: AlignItems,
    /// Stretched to the line: `align-self: stretch` with an `auto` cross size. A percentage cross
    /// size that cannot be resolved is not `auto` (Taffy's `FlexItem::is_stretched`).
    stretched: bool,
    /// Border-box main size from the style, if definite.
    style_main: Opt<S>,
    /// Border-box cross size from the style, if definite.
    style_cross: Opt<S>,
    min_main: Opt<S>,
    max_main: Opt<S>,
    min_cross: Opt<S>,
    max_cross: Opt<S>,
    /// Flex basis from the style, resolved to a border-box length, if definite.
    basis: Opt<S>,
    /// The item's own padding plus border on the main axis.
    pb_main: S,
    /// The item's own padding plus border on the cross axis.
    pb_cross: S,
    base: S,
    hypothetical: S,
    target: S,
    cross: S,
    /// Main size the child actually took in its final layout (its content box is floored at
    /// zero, so this can exceed `target`).
    used_main: S,
    frozen: bool,
    violation: i8,
}

/// Padding plus border, resolved against the containing block's inline size.
pub(crate) fn padding_border<S: Scalar>(style: &Style<S>, basis: Opt<S>) -> Edges<S> {
    let p = style.resolve_edges(slot::PADDING, basis);
    let b = style.border();
    Edges { left: p.left + b.left, right: p.right + b.right, top: p.top + b.top, bottom: p.bottom + b.bottom }
}

/// Resolves a `(size, min, max)` triple on `axis` to border-box terms against `parent`.
fn sizing<S: Scalar>(style: &Style<S>, axis: Axis, parent: Opt<S>, pb_sum: S) -> (Opt<S>, Opt<S>, Opt<S>) {
    let (size_slot, min_slot, max_slot) = slot::sizing(axis);
    let content_box = style.box_sizing() == BoxSizing::ContentBox;
    let to_bb = |v: S| if content_box { v + pb_sum } else { v };
    (
        style.resolve(size_slot, parent).map(to_bb),
        style.resolve(min_slot, parent).map(to_bb),
        style.resolve(max_slot, parent).map(to_bb),
    )
}

fn gap_slot(main: Axis) -> usize {
    match main {
        Axis::Horizontal => slot::COLUMN_GAP,
        Axis::Vertical => slot::ROW_GAP,
    }
}

/// Lays out `node`'s children as a single flex line and returns the node's border-box size.
pub(crate) fn layout_container<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    input: Input<S>,
) -> Result<(Size<S>, Opt<S>), Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let style = *node.parts().style;
    let direction = match style.direction() {
        Direction::Inherit => input.direction,
        d => d,
    };
    let flex_direction = style.flex_direction();
    let main = if flex_direction.is_column() { Axis::Vertical } else { Axis::Horizontal };
    let cross = main.cross();

    // ---- the container's own box ----
    let pb = padding_border(&style, input.parent_size.width);
    let pb_sum = pb.sums();
    let border_box: Size<Opt<S>> = Size::from_fn(|axis| {
        if input.known.get(axis).is_some() {
            return input.known.get(axis);
        }
        let (size, min, max) = sizing(&style, axis, input.parent_size.get(axis), pb_sum.get(axis));
        size.map(|v| v.clamp_opt(min, max))
    });
    let inner: Size<Opt<S>> = Size::from_fn(|axis| border_box.get(axis).map(|v| (v - pb_sum.get(axis)).max(S::ZERO)));
    let (inner_min, inner_max): (Size<Opt<S>>, Size<Opt<S>>) = {
        let mut min = Size::new(Opt::NONE, Opt::NONE);
        let mut max = Size::new(Opt::NONE, Opt::NONE);
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let (_, mn, mx) = sizing(&style, axis, input.parent_size.get(axis), pb_sum.get(axis));
            min.set(axis, mn.map(|v| (v - pb_sum.get(axis)).max(S::ZERO)));
            max.set(axis, mx.map(|v| (v - pb_sum.get(axis)).max(S::ZERO)));
        }
        (min, max)
    };
    let available_inner: Size<Available<S>> = Size::from_fn(|axis| match inner.get(axis).get() {
        Some(v) => Available::Definite(v),
        None => input.available.get(axis).shrink(pb_sum.get(axis)),
    });
    let gap = style.resolve(gap_slot(main), inner.get(main)).unwrap_or(S::ZERO);
    // Definiteness (Taffy's `known_dimensions_are_definite`): a size imposed by the parent is
    // definite unless the parent marked it otherwise; a size from the node's own style always is.
    let known_definite = Size::from_fn(|axis| input.known.get(axis).is_none() || !input.indefinite.get(axis));
    let has_definite_main = known_definite.get(main) && inner.get(main).is_some();
    let has_definite_cross = known_definite.get(cross) && inner.get(cross).is_some();
    let cross_available_definite = has_definite_cross || matches!(input.available.get(cross), Available::Definite(_));
    // Items' percentages resolve against the inner size, but not against an indefinite main size.
    let mut percent_basis = inner;
    if !known_definite.get(main) {
        percent_basis.set(main, Opt::NONE);
    }

    // ---- collect items ----
    let mut items = pass.cx.scratch.take_items();
    let mut absolute = pass.cx.scratch.take_indices();
    let child_count = node.parts().children.len();
    for i in 0..child_count {
        let cs = *node.parts().children[i].node.parts().style;
        if cs.display() == Display::None {
            continue;
        }
        if cs.position() == Position::Absolute {
            absolute.push(i as u32);
            continue;
        }
        let margin = cs.resolve_edges(slot::MARGIN, inner.width);
        let child_pb = padding_border(&cs, inner.width).sums();
        let (style_main, min_main, max_main) = sizing(&cs, main, percent_basis.get(main), child_pb.get(main));
        let (style_cross, min_cross, max_cross) = sizing(&cs, cross, percent_basis.get(cross), child_pb.get(cross));
        let basis = match cs.flex_basis() {
            Length::Px(v) => {
                Opt::some(if cs.box_sizing() == BoxSizing::ContentBox { v + child_pb.get(main) } else { v })
            }
            Length::Percent(p) => percent_basis
                .get(main)
                .map(|m| m * p)
                .map(|v| if cs.box_sizing() == BoxSizing::ContentBox { v + child_pb.get(main) } else { v }),
            _ => Opt::NONE,
        };
        items.push(Item {
            child: i as u32,
            margin,
            grow: cs.flex_grow(),
            shrink: cs.flex_shrink(),
            align: cs.align_self().unwrap_or(style.align_items()),
            stretched: cs.align_self().unwrap_or(style.align_items()).is_stretch()
                && matches!(if cross == Axis::Horizontal { cs.width() } else { cs.height() }, Length::Auto),
            style_main,
            style_cross,
            min_main,
            max_main,
            min_cross,
            max_cross,
            basis,
            pb_main: child_pb.get(main),
            pb_cross: child_pb.get(cross),
            base: S::ZERO,
            hypothetical: S::ZERO,
            target: S::ZERO,
            cross: S::ZERO,
            used_main: S::ZERO,
            frozen: false,
            violation: 0,
        });
    }
    let n = items.len();

    // ---- flex base sizes and hypothetical main sizes (§9.2) ----
    for idx in 0..n {
        let it = items[idx];
        let base = if let Some(b) = it.basis.get() {
            b
        } else if let Some(m) = it.style_main.get() {
            m
        } else {
            // Content-based: size the child with max-content on the main axis. A stretched
            // item with a definite container cross size is measured at that cross size.
            let stretched_cross = if it.align.is_stretch() && it.style_cross.is_none() {
                inner.get(cross).map(|c| (c - it.margin.sum(cross)).max(S::ZERO).clamp_opt(it.min_cross, it.max_cross))
            } else {
                it.style_cross
            };
            let mut known = Size::new(Opt::NONE, Opt::NONE);
            known.set(cross, stretched_cross);
            // The item's own margins come out of the space available to it.
            let mut available = Size::from_fn(|axis| available_inner.get(axis).shrink(it.margin.sum(axis)));
            available.set(main, Available::MaxContent);
            // Taffy measures the flex base size in `SizingMode::ContentSize`: a leaf's own
            // min/max sizes do not clamp it (the hypothetical main size does that).
            let child_input = Input::new(known, available, percent_basis, direction).content_size();
            pass.visit_child(node, it.child as usize, child_input)?.get(main)
        };
        items[idx].base = base;
        items[idx].hypothetical = base.clamp_opt(it.min_main, it.max_main);
    }

    let gaps = if n > 1 { gap * S::from_f32((n - 1) as f32) } else { S::ZERO };
    let inner_main = match inner.get(main).get() {
        // TAFFY QUIRK: a main size imposed by the parent is taken as the border-box size as is,
        // without flooring the content box at zero (a size from the style is floored).
        Some(v) => match input.known.get(main).get() {
            Some(k) => k - pb_sum.get(main),
            None => v,
        },
        None => {
            // Taffy's rule for an auto main size: under definite available space the line is
            // the sum of base sizes clamped by min only (not by max); under an intrinsic
            // constraint it is the sum of hypothetical sizes.
            let sum = match available_inner.get(main) {
                Available::Definite(_) => items.iter().fold(S::ZERO, |acc, it| {
                    acc + (it.base.max(it.min_main.unwrap_or(S::ZERO)) + it.margin.sum(main)).max(it.pb_main)
                }),
                _ => items.iter().fold(S::ZERO, |acc, it| acc + intrinsic_contribution(it, main)),
            };
            (sum + gaps).clamp_opt(inner_min.get(main), inner_max.get(main))
        }
    };

    // Taffy's `item_known_dimension_definiteness`: an item's flexed main size is definite only if
    // the container's main size is or the item's flex basis was; its cross size is definite if
    // stretched, set in the style, or (in a column) under definite cross space.
    let item_indefinite = |it: &Item<S>| {
        let main_definite = has_definite_main || it.basis.is_some() || it.style_main.is_some();
        let cross_definite = it.stretched
            || it.style_cross.is_some()
            || (main == Axis::Vertical && cross_available_definite);
        let mut s = Size::new(false, false);
        s.set(main, !main_definite);
        s.set(cross, !cross_definite);
        s
    };

    // ---- resolve flexible lengths (§9.7) ----
    let margins_main = items.iter().fold(S::ZERO, |acc, it| acc + it.margin.sum(main));
    resolve_flexible_lengths(&mut items, inner_main - gaps - margins_main);

    // ---- cross sizes (§9.4) ----
    for idx in 0..n {
        let it = items[idx];
        let hypothetical_cross = if let Some(c) = it.style_cross.get() {
            c
        } else if it.stretched && inner.get(cross).is_some() {
            // §9.8 rule 3: a stretched item of a single-line container with a definite cross
            // size has a definite cross size; no measurement needed.
            (inner.get(cross).unwrap_or(S::ZERO) - it.margin.sum(cross)).max(S::ZERO)
        } else {
            let mut known = Size::new(Opt::NONE, Opt::NONE);
            known.set(main, Opt::some(it.target));
            let available = Size::from_fn(|axis| available_inner.get(axis).shrink(it.margin.sum(axis)));
            let child_input = Input::new(known, available, inner, direction).with_indefinite(item_indefinite(&it));
            pass.visit_child(node, it.child as usize, child_input)?.get(cross)
        };
        // Taffy floors the hypothetical cross size at the item's padding plus border
        // (`determine_hypothetical_cross_size`); a stretched item is re-sized below without it.
        items[idx].cross = hypothetical_cross.clamp_opt(it.min_cross, it.max_cross).max(it.pb_cross);
    }
    let inner_cross = match inner.get(cross).get() {
        Some(v) => v,
        None => items
            .iter()
            .fold(S::ZERO, |acc, it| acc.max(it.cross + it.margin.sum(cross)))
            .clamp_opt(inner_min.get(cross), inner_max.get(cross)),
    };
    for it in items.iter_mut() {
        if it.stretched {
            it.cross = (inner_cross - it.margin.sum(cross)).max(S::ZERO).clamp_opt(it.min_cross, it.max_cross);
        }
    }

    let size = {
        let mut s = Size::ZERO;
        s.set(main, inner_main + pb_sum.get(main));
        s.set(cross, inner_cross + pb_sum.get(cross));
        s
    };

    // A size query stops here: children are not positioned and nothing more is written.
    if !input.final_layout {
        pass.cx.scratch.give_items(items);
        pass.cx.scratch.give_indices(absolute);
        return Ok((size, Opt::NONE));
    }

    // ---- final layout of each child at its definite size ----
    for idx in 0..n {
        let it = items[idx];
        let mut known = Size::new(Opt::NONE, Opt::NONE);
        known.set(main, Opt::some(it.target));
        known.set(cross, Opt::some(it.cross));
        let mut available = Size::new(Available::Definite(S::ZERO), Available::Definite(S::ZERO));
        available.set(main, Available::Definite(it.target));
        available.set(cross, Available::Definite(it.cross));
        let child_input = Input::new(known, available, inner, direction).with_indefinite(item_indefinite(&it)).finalize();
        items[idx].used_main = pass.visit_child(node, it.child as usize, child_input)?.get(main);
    }

    // ---- main-axis distribution (§9.5) and cross-axis alignment (§9.6) ----
    // Positions are computed in physical order along the main axis (reversed item order for the
    // `*-reverse` directions), with css-align's fallbacks for overflow, then mirrored for RTL rows.
    let used_main = items.iter().fold(S::ZERO, |acc, it| acc + it.target + it.margin.sum(main)) + gaps;
    let free = inner_main - used_main;
    let flex_reversed = flex_direction.is_reverse();
    let justify = justify_fallback(style.justify_content(), free, n);
    let (pb_main_start, _) = pb.along(main);
    let (pb_cross_start, _) = pb.along(cross);
    let mut position = alignment_offset(free, n, gap, justify, flex_reversed, true);
    for step in 0..n {
        let idx = if flex_reversed { n - 1 - step } else { step };
        let it = items[idx];
        if step > 0 {
            position = position + alignment_offset(free, n, gap, justify, flex_reversed, false);
        }
        let (margin_main_start, _) = it.margin.along(main);
        let (margin_cross_start, _) = it.margin.along(cross);
        let mut main_pos = position + margin_main_start;
        if direction == Direction::Rtl && main == Axis::Horizontal {
            main_pos = inner_main - main_pos - it.used_main;
        }
        let outer_cross = it.cross + it.margin.sum(cross);
        let cross_pos = match it.align {
            AlignItems::FlexEnd | AlignItems::End | AlignItems::SelfEnd => inner_cross - outer_cross,
            AlignItems::Center => (inner_cross - outer_cross) / S::from_f32(2.0),
            _ => S::ZERO,
        } + margin_cross_start;
        let mut offset = Point::ZERO;
        offset.set(main, pb_main_start + main_pos);
        offset.set(cross, pb_cross_start + cross_pos);
        pass.set_offset(node, it.child as usize, offset);
        position = position + it.margin.sum(main) + it.used_main;
    }

    // ---- absolutely positioned children, against this node's padding box ----
    if !absolute.is_empty() {
        let border = style.border();
        let padding_box = Size::new(size.width - border.left - border.right, size.height - border.top - border.bottom);
        let padding_box_opt = padding_box.map(Opt::some);
        for &k in absolute.iter() {
            let i = k as usize;
            let cs = *node.parts().children[i].node.parts().style;
            let child_pb = padding_border(&cs, padding_box_opt.width).sums();
            let mut known = Size::new(Opt::NONE, Opt::NONE);
            let mut bounds = Size::new((Opt::NONE, Opt::NONE), (Opt::NONE, Opt::NONE));
            for axis in [Axis::Horizontal, Axis::Vertical] {
                let (sz, mn, mx) = sizing(&cs, axis, padding_box_opt.get(axis), child_pb.get(axis));
                known.set(axis, sz.map(|v| v.clamp_opt(mn, mx)));
                bounds.set(axis, (mn, mx));
            }
            let measured = if known.width.is_none() || known.height.is_none() {
                let available = Size::from_fn(|axis| match known.get(axis).get() {
                    Some(v) => Available::Definite(v),
                    None => Available::MaxContent,
                });
                let probe_input = Input::new(known, available, padding_box_opt, direction);
                let s = pass.visit_child(node, i, probe_input)?;
                Size::from_fn(|axis| match known.get(axis).get() {
                    Some(v) => v,
                    None => {
                        let (mn, mx) = bounds.get(axis);
                        s.get(axis).clamp_opt(mn, mx)
                    }
                })
            } else {
                known.map(|k| k.unwrap_or(S::ZERO))
            };
            let final_input = Input::new(
                measured.map(Opt::some),
                measured.map(Available::Definite),
                padding_box_opt,
                direction,
            )
            .finalize();
            pass.visit_child(node, i, final_input)?;
            let inset = cs.inset();
            let margin = cs.resolve_edges(slot::MARGIN, padding_box_opt.width);
            let mut offset = Point::ZERO;
            for axis in [Axis::Horizontal, Axis::Vertical] {
                let (start, end) = inset.along(axis);
                let (margin_start, margin_end) = margin.along(axis);
                let extent = padding_box.get(axis);
                let basis = padding_box_opt.get(axis);
                let pos = if let Some(s) = start.resolve(basis).get() {
                    s + margin_start
                } else if let Some(e) = end.resolve(basis).get() {
                    extent - e - margin_end - measured.get(axis)
                } else {
                    // Static position: the content-box start.
                    let (pb_start, _) = pb.along(axis);
                    let (border_start, _) = border.along(axis);
                    pb_start - border_start + margin_start
                };
                let (border_start, _) = border.along(axis);
                offset.set(axis, border_start + pos);
            }
            pass.set_offset(node, i, offset);
        }
    }

    pass.cx.scratch.give_items(items);
    pass.cx.scratch.give_indices(absolute);
    Ok((size, Opt::NONE))
}

/// An item's outer contribution to its container's min-/max-content main size, mirroring the
/// per-item step of Taffy's `determine_container_main_size` (intrinsic branch): the content
/// contribution, with the margins *inside* the min/max clamp (TAFFY QUIRK), bounded by the flex base size for
/// an item that cannot grow (or shrink). For an item whose base is its content size the
/// measured size is the base itself; an item with only a definite `flex-basis` uses its base.
fn intrinsic_contribution<S: Scalar>(it: &Item<S>, main: Axis) -> S {
    let margin = it.margin.sum(main);
    let row = main == Axis::Horizontal;
    let resolved_min = it.min_main.unwrap_or(S::ZERO);
    let clamping_basis = if row { it.style_main.get().map_or(it.base, |p| it.base.max(p)) } else { it.base };
    let basis_min = if it.shrink == S::ZERO { Opt::some(clamping_basis) } else { Opt::NONE };
    let basis_max = if it.grow == S::ZERO { Opt::some(clamping_basis) } else { Opt::NONE };
    let min = match (it.min_main.get(), basis_min.get()) {
        (Some(a), Some(b)) => a.max(b),
        (a, b) => a.or(b).unwrap_or(resolved_min),
    }
    .max(resolved_min);
    let max = match (it.max_main.get(), basis_max.get()) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let contribution = match (it.style_main.get(), max) {
        (Some(pref), Some(max)) if max <= min || max <= pref => pref.min(max).max(min) + margin,
        (_, Some(max)) if max <= min => min + margin,
        (pref, _) => {
            let inner = match pref {
                Some(p) => p.max(it.pb_main),
                None => it.base,
            };
            let inner = if row { inner } else { inner.max(it.base) };
            (inner + margin).clamp_opt(it.min_main, it.max_main)
        }
    };
    // An item whose inner flex base size is zero has nothing to shrink, so a contribution below
    // its base is raised back to the base.
    let wants_to_shrink = contribution - margin < it.base;
    if wants_to_shrink && it.base - it.pb_main == S::ZERO {
        it.base + margin
    } else {
        contribution
    }
}

/// §9.7 "Resolving Flexible Lengths" for one line. `available` is the inner main size less
/// gaps and item margins, so it is compared against border-box item sizes directly.
fn resolve_flexible_lengths<S: Scalar>(items: &mut [Item<S>], available: S) {
    let hypothetical_sum = items.iter().fold(S::ZERO, |acc, it| acc + it.hypothetical);
    let growing = hypothetical_sum < available;
    for it in items.iter_mut() {
        let factor = if growing { it.grow } else { it.shrink };
        it.frozen = factor <= S::ZERO || (growing && it.base > it.hypothetical) || (!growing && it.base < it.hypothetical);
        it.target = it.hypothetical;
        it.violation = 0;
    }
    let initial_free = items.iter().fold(available, |acc, it| acc - if it.frozen { it.target } else { it.base });

    for _ in 0..=items.len() {
        if items.iter().all(|it| it.frozen) {
            break;
        }
        let mut remaining = items.iter().fold(available, |acc, it| acc - if it.frozen { it.target } else { it.base });
        let factor_sum = items
            .iter()
            .filter(|it| !it.frozen)
            .fold(S::ZERO, |acc, it| acc + if growing { it.grow } else { it.shrink });
        if factor_sum < S::ONE {
            let scaled = initial_free * factor_sum;
            if scaled.abs() < remaining.abs() {
                remaining = scaled;
            }
        }
        if growing {
            for it in items.iter_mut().filter(|it| !it.frozen) {
                it.target = if factor_sum > S::ZERO { it.base + remaining.mul_div(it.grow, factor_sum) } else { it.base };
            }
        } else {
            // The scaled flex shrink factor uses the *inner* flex base size (§9.7 step 4c).
            let inner_base = |it: &Item<S>| (it.base - it.pb_main).max(S::ZERO);
            let scaled_sum = items.iter().filter(|it| !it.frozen).fold(S::ZERO, |acc, it| acc + it.shrink * inner_base(it));
            for it in items.iter_mut().filter(|it| !it.frozen) {
                let scaled = it.shrink * inner_base(it);
                it.target = if scaled_sum > S::ZERO { it.base - remaining.abs().mul_div(scaled, scaled_sum) } else { it.base };
            }
        }
        let mut total_violation = S::ZERO;
        for it in items.iter_mut().filter(|it| !it.frozen) {
            let clamped = it.target.max(S::ZERO).clamp_opt(it.min_main, it.max_main);
            let violation = clamped - it.target;
            it.violation = if violation > S::ZERO {
                1
            } else if violation < S::ZERO {
                -1
            } else {
                0
            };
            total_violation = total_violation + violation;
            it.target = clamped;
        }
        for it in items.iter_mut().filter(|it| !it.frozen) {
            if total_violation == S::ZERO
                || (total_violation > S::ZERO && it.violation > 0)
                || (total_violation < S::ZERO && it.violation < 0)
            {
                it.frozen = true;
            }
        }
    }
}

/// css-align's fallback alignment for a distributed keyword that cannot distribute: with one
/// item or negative free space, `space-between` and `stretch` fall back to `flex-start`,
/// `space-around` and `space-evenly` to `center`, and those fallbacks are *safe*, so on overflow
/// they become the physical `start`. Mirrors Taffy's `apply_alignment_fallback`.
fn justify_fallback<S: Scalar>(justify: JustifyContent, free: S, n: usize) -> JustifyContent {
    let mut keyword = match justify {
        JustifyContent::Normal => JustifyContent::Stretch,
        k => k,
    };
    let mut safe = false;
    if n <= 1 || free <= S::ZERO {
        (keyword, safe) = match keyword {
            JustifyContent::Stretch | JustifyContent::SpaceBetween => (JustifyContent::FlexStart, true),
            JustifyContent::SpaceAround | JustifyContent::SpaceEvenly => (JustifyContent::Center, true),
            other => (other, safe),
        };
    }
    if free <= S::ZERO && safe {
        keyword = JustifyContent::Start;
    }
    keyword
}

/// The leading offset (`first`) or the gap before a subsequent item, in physical main-axis
/// order. Mirrors Taffy's `compute_alignment_offset`.
fn alignment_offset<S: Scalar>(free: S, n: usize, gap: S, justify: JustifyContent, reversed: bool, first: bool) -> S {
    let two = S::from_f32(2.0);
    if first {
        match justify {
            JustifyContent::Start => S::ZERO,
            JustifyContent::FlexStart => if reversed { free } else { S::ZERO },
            JustifyContent::End => free,
            JustifyContent::FlexEnd => if reversed { S::ZERO } else { free },
            JustifyContent::Center => free / two,
            JustifyContent::Normal | JustifyContent::Stretch | JustifyContent::SpaceBetween => S::ZERO,
            JustifyContent::SpaceAround => {
                if free >= S::ZERO { (free / S::from_f32(n as f32)) / two } else { free / two }
            }
            JustifyContent::SpaceEvenly => {
                if free >= S::ZERO { free / S::from_f32((n + 1) as f32) } else { free / two }
            }
        }
    } else {
        let free = free.max(S::ZERO);
        gap + match justify {
            JustifyContent::SpaceBetween => free / S::from_f32((n - 1) as f32),
            JustifyContent::SpaceAround => free / S::from_f32(n as f32),
            JustifyContent::SpaceEvenly => free / S::from_f32((n + 1) as f32),
            _ => S::ZERO,
        }
    }
}
