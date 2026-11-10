//! The flex layout algorithm: CSS Flexible Box Layout Module Level 1, §9, over the storage layer.
//!
//! The entry point is [`layout_container`]. It runs in two modes that share every step: a
//! *size query* computes the container's border-box size and first baseline without writing
//! anything into the tree, and a *final layout* additionally lays each child out at its used
//! size and writes the child's offset into this container's list.
//!
//! The steps follow the specification's numbering. Items are collected (§9.1, with `order`,
//! `display: none`, `display: contents` flattening and absolutely positioned children set
//! aside), their flex base sizes and hypothetical main sizes are determined (§9.2 with the §4.5
//! automatic minimum size), they are broken into lines (§9.3), flexed (§9.7), given cross sizes
//! with baseline groups and `align-content: stretch` (§9.4), aligned on the main axis with auto
//! margins and `justify-content` (§9.5) and on the cross axis with auto margins, `align-self` and
//! `align-content` (§9.6), and finally positioned, after which absolutely positioned children are
//! placed against the §4.1 static-position rectangle.
//!
//! Chrome is the arbiter where the specification leaves room: the container's intrinsic main
//! sizes follow the web-compatible algorithm of §9.9.1.2 as Chrome implements it, the inline
//! axis of an auto-sized container is resolved to a definite size *before* its items are laid
//! out (so item percentages resolve against it), and a block-axis size that comes from content
//! is never a percentage basis for descendants.
//!
//! Geometry is kept in main/cross terms with physical axes underneath, so `direction: rtl`, the
//! `*-reverse` flex directions and `wrap-reverse` are each a single mirroring step.

mod items;
mod lines;
mod place;

use super::{Cancelled, Input, Pass};
use crate::geometry::{Axis, Edges, Size};
use crate::measure::{Available, Measure};
use crate::scalar::{Opt, Scalar};
use crate::style::{slot, AlignContent, AlignItems, BoxSizing, Direction, FlexWrap, JustifyContent, Overflow, Position, Style, Unit};
use crate::tree::LayoutNode;
use alloc::vec::Vec;

pub(crate) use items::Item;
pub(crate) use lines::Line;

// Item flags.
const F_STRETCHED: u16 = 1 << 0; // auto cross size, `align-self: stretch`, no auto cross margins
const F_SCROLL: u16 = 1 << 1; // a scroll container: zero automatic minimum size
const F_BASELINE: u16 = 1 << 2; // participates in baseline alignment
const F_FROZEN: u16 = 1 << 3;
const F_CONTENT_BASE: u16 = 1 << 4; // the flex base size is the item's max-content main size
const F_BASIS_DEFINITE: u16 = 1 << 5; // the flex base size came from a definite basis or main size
const F_RELATIVE: u16 = 1 << 6; // `position: relative` with insets
const F_AUTO_MIN: u16 = 1 << 7; // the §4.5 automatic minimum is deferred until the line shrinks
const F_CONTENT_BOX: u16 = 1 << 8; // the child's `box-sizing` is `content-box`

// Auto-margin bits, physical edges.
const M_LEFT: u8 = 1;
const M_RIGHT: u8 = 2;
const M_TOP: u8 = 4;
const M_BOTTOM: u8 = 8;

/// The container's axes and orientation, resolved once.
#[derive(Copy, Clone)]
pub(crate) struct Flow {
    pub main: Axis,
    pub cross: Axis,
    /// The flex main-start is at the physical end of the main axis.
    pub main_reversed: bool,
    /// The flex cross-start is at the physical end of the cross axis.
    pub cross_reversed: bool,
    /// Resolved inline direction of the container.
    pub direction: Direction,
    pub multiline: bool,
    pub wrap_reverse: bool,
}

impl Flow {
    fn new<S: Scalar>(style: &Style<S>, direction: Direction) -> Self {
        let fd = style.flex_direction();
        let rtl = direction == Direction::Rtl;
        let main = if fd.is_column() { Axis::Vertical } else { Axis::Horizontal };
        let wrap = style.flex_wrap();
        let wrap_reverse = wrap == FlexWrap::WrapReverse;
        Flow {
            main,
            cross: main.cross(),
            main_reversed: fd.is_reverse() ^ (rtl && main == Axis::Horizontal),
            cross_reversed: wrap_reverse ^ (rtl && main == Axis::Vertical),
            direction,
            multiline: wrap != FlexWrap::NoWrap,
            wrap_reverse,
        }
    }
    /// True when `axis` runs right to left: the horizontal axis of an rtl container.
    #[inline]
    pub fn axis_rtl(&self, axis: Axis) -> bool {
        axis == Axis::Horizontal && self.direction == Direction::Rtl
    }
    /// Whether the flex start of `axis` is at its physical end.
    #[inline]
    pub fn reversed(&self, axis: Axis) -> bool {
        if axis == self.main { self.main_reversed } else { self.cross_reversed }
    }
    /// Whether the writing-mode `start` of `axis` is the same edge as its flex start.
    #[inline]
    pub fn start_is_flex_start(&self, axis: Axis) -> bool {
        self.axis_rtl(axis) == self.reversed(axis)
    }
}

/// The per-container constants every step reads.
pub(crate) struct Ctx<S: Scalar> {
    pub flow: Flow,
    /// Padding plus border plus scrollbar gutter.
    pub pb: Edges<S>,
    /// Border plus scrollbar gutter: the inset of the padding box that absolutely positioned
    /// children are placed in.
    pub border_gutter: Edges<S>,
    /// The container's inner size where definite (the inline size always is at layout time).
    pub inner: Size<Opt<S>>,
    /// Percentage basis for the items: `inner` without any axis the parent marked indefinite.
    pub percent: Size<Opt<S>>,
    /// Space available to the items, less the container's own padding and border.
    pub available: Size<Available<S>>,
    /// Inner min and max sizes of the container, used on auto-sized axes. None in a content-size
    /// question, where the parent applies the item's min and max.
    pub min_inner: Size<Opt<S>>,
    pub max_inner: Size<Opt<S>>,
    /// The same from the style regardless of the question: a wrapping container breaks its
    /// lines within its own max main size even when its content size is asked for.
    pub style_min_inner: Size<Opt<S>>,
    pub style_max_inner: Size<Opt<S>>,
    /// Border-box min and max sizes, for clamping a content-derived border-box size.
    pub min_bb: Size<Opt<S>>,
    pub max_bb: Size<Opt<S>>,
    pub gap_main: S,
    pub gap_cross: S,
    pub justify: JustifyContent,
    pub align_items: AlignItems,
    pub align_content: AlignContent,
    /// The parent said the imposed height is not a percentage basis.
    pub indefinite_height: bool,
}

impl<S: Scalar> Ctx<S> {
    #[inline]
    fn main(&self) -> Axis {
        self.flow.main
    }
    #[inline]
    fn cross(&self) -> Axis {
        self.flow.cross
    }
}

/// The scrollbar gutter as edges: a vertical scrollbar (`overflow-y: scroll`) takes space at
/// the inline end, a horizontal one at the bottom.
pub(crate) fn scrollbar_gutter<S: Scalar>(style: &Style<S>, direction: Direction) -> Edges<S> {
    let gutter = style.scrollbar_gutter();
    let mut e = Edges::ZERO;
    if style.overflow_y() == Overflow::Scroll {
        if direction == Direction::Rtl {
            e.left = gutter;
        } else {
            e.right = gutter;
        }
    }
    if style.overflow_x() == Overflow::Scroll {
        e.bottom = gutter;
    }
    e
}

/// Padding plus border, with percentage padding resolved against the containing block's inline
/// size. This is what `box-sizing: content-box` adds and what a box's size is floored at.
pub(crate) fn padding_border<S: Scalar>(style: &Style<S>, basis: Opt<S>) -> Edges<S> {
    let p = style.resolve_edges(slot::PADDING, basis);
    let b = style.border();
    Edges { left: p.left + b.left, right: p.right + b.right, top: p.top + b.top, bottom: p.bottom + b.bottom }
}

/// Padding plus border plus the scrollbar gutter: the inset of the content box within the
/// border box.
pub(crate) fn content_inset<S: Scalar>(style: &Style<S>, basis: Opt<S>, direction: Direction) -> Edges<S> {
    let pb = padding_border(style, basis);
    let g = scrollbar_gutter(style, direction);
    Edges { left: pb.left + g.left, right: pb.right + g.right, top: pb.top + g.top, bottom: pb.bottom + g.bottom }
}

/// Inner (content-box) min and max sizes of a container on both axes from its style, for
/// clamping content-derived sizes.
fn inner_bounds<S: Scalar>(style: &Style<S>, input: &Input<S>, pb_only: Size<S>, inset: Size<S>) -> Bounds<S> {
    let mut b = Bounds { min_inner: Size::new(Opt::NONE, Opt::NONE), max_inner: Size::new(Opt::NONE, Opt::NONE), min_bb: Size::new(Opt::NONE, Opt::NONE), max_bb: Size::new(Opt::NONE, Opt::NONE) };
    for axis in [Axis::Horizontal, Axis::Vertical] {
        let (_, min_slot, max_slot) = slot::sizing(axis);
        let basis = input.parent_size.get(axis);
        let (mn, _) = resolve_bb(style, min_slot, basis, pb_only.get(axis));
        let (mx, _) = resolve_bb(style, max_slot, basis, pb_only.get(axis));
        b.min_bb.set(axis, mn);
        b.max_bb.set(axis, mx);
        b.min_inner.set(axis, mn.map(|v| (v - inset.get(axis)).max(S::ZERO)));
        b.max_inner.set(axis, mx.map(|v| (v - inset.get(axis)).max(S::ZERO)));
    }
    b
}

/// Min and max sizes of a container, in border-box and content-box terms.
struct Bounds<S: Scalar> {
    min_inner: Size<Opt<S>>,
    max_inner: Size<Opt<S>>,
    min_bb: Size<Opt<S>>,
    max_bb: Size<Opt<S>>,
}

/// Resolves the length in slot `i` to a border-box size against `basis`: a definite value for
/// `Px` and resolvable `Percent`, none otherwise. The unit tag comes back for keyword handling.
#[inline]
pub(crate) fn resolve_bb<S: Scalar>(style: &Style<S>, i: usize, basis: Opt<S>, pb_axis: S) -> (Opt<S>, Unit) {
    let (v, tag) = style.resolve_tagged(i, basis);
    let v = if style.box_sizing() == BoxSizing::ContentBox { v.map(|v| v + pb_axis) } else { v };
    (v, tag)
}

/// Converts a border-box size on `from` to the other axis through `ratio` (width / height),
/// honouring `box-sizing`: the ratio relates the box that `width` and `height` set.
#[inline]
pub(crate) fn transfer<S: Scalar>(style: &Style<S>, pb: Size<S>, from: Axis, value: S, ratio: S) -> S {
    transfer_bb(style.box_sizing() == BoxSizing::ContentBox, pb, from, value, ratio)
}

/// [`transfer`] given the `box-sizing` directly.
pub(crate) fn transfer_bb<S: Scalar>(content_box: bool, pb: Size<S>, from: Axis, value: S, ratio: S) -> S {
    let inner = if content_box { (value - pb.get(from)).max(S::ZERO) } else { value };
    let other = match from {
        Axis::Horizontal => inner / ratio,
        Axis::Vertical => inner * ratio,
    };
    if content_box { other + pb.get(from.cross()) } else { other }
}

/// The resolved direction of a node whose own `direction` may be `inherit`.
#[inline]
pub(crate) fn resolve_direction(own: Direction, inherited: Direction) -> Direction {
    match own {
        Direction::Inherit => inherited,
        d => d,
    }
}

/// How an auto-sized inline axis is resolved.
#[derive(Copy, Clone, PartialEq)]
enum IntrinsicMode<S> {
    Min,
    Max,
    /// `fit-content` against a definite available size: `min(max-content, max(min-content, v))`.
    Fit(S),
}

/// Lays out `node` under `input`; see the [module documentation](self).
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
    let direction = resolve_direction(style.direction(), input.direction);
    let flow = Flow::new(&style, direction);
    let pb = content_inset(&style, input.parent_size.width, direction);
    let pb_sum = pb.sums();
    let pb_only = padding_border(&style, input.parent_size.width).sums();

    // The container's own box, per axis: an imposed size wins; otherwise the style, unless the
    // question is for the content size, in which case the style's sizes are ignored on axes
    // the parent did not fix.
    let own = |axis: Axis| -> (Opt<S>, Unit, Opt<S>, Opt<S>) {
        let (size_slot, min_slot, max_slot) = slot::sizing(axis);
        let basis = input.parent_size.get(axis);
        if input.known.get(axis).is_some() || input.content_size {
            return (Opt::NONE, Unit::Auto, Opt::NONE, Opt::NONE);
        }
        let (size, tag) = resolve_bb(&style, size_slot, basis, pb_only.get(axis));
        let (min, _) = resolve_bb(&style, min_slot, basis, pb_only.get(axis));
        let (max, _) = resolve_bb(&style, max_slot, basis, pb_only.get(axis));
        (size, tag, min, max)
    };
    let (own_w, tag_w, min_w, max_w) = own(Axis::Horizontal);
    let (own_h, _, min_h, max_h) = own(Axis::Vertical);
    let ratio = if input.content_size { Opt::NONE } else { style.aspect_ratio() };
    // A definite size on one axis transfers through the aspect ratio to the other.
    let (own_w, own_h) = match (own_w.get(), own_h.get(), ratio.get()) {
        (None, Some(h), Some(r)) => (Opt::some(transfer(&style, pb_only, Axis::Vertical, h, r)), own_h),
        (Some(w), None, Some(r)) => (own_w, Opt::some(transfer(&style, pb_only, Axis::Horizontal, w, r))),
        _ => (own_w, own_h),
    };
    // The height, if decided: imposed or from the style. Otherwise it is a result of layout.
    let height_bb: Opt<S> = match input.known.height.get() {
        Some(h) => Opt::some(h),
        None => own_h.map(|h| h.clamp_opt(min_h, max_h).max(pb_only.height)),
    };
    let inner_height = height_bb.map(|h| (h - pb_sum.height).max(S::ZERO));
    let height_indefinite = input.known.height.is_some() && input.indefinite.height;

    // The width is always resolved to a definite border-box value before the items are laid out.
    let width_bb: S = match input.known.width.get() {
        Some(w) => w,
        None => match own_w.get() {
            Some(w) => w.clamp_opt(min_w, max_w).max(pb_only.width),
            None => {
                let inner = match (tag_w, input.available.width) {
                    (Unit::Stretch, Available::Definite(a)) => (a - pb_sum.width).max(S::ZERO),
                    (Unit::MinContent, _) | (_, Available::MinContent) => {
                        intrinsic_inline(pass, node, &style, flow, pb, IntrinsicMode::Min, inner_height, &input)?
                    }
                    (Unit::MaxContent, _) | (_, Available::MaxContent) => {
                        intrinsic_inline(pass, node, &style, flow, pb, IntrinsicMode::Max, inner_height, &input)?
                    }
                    (_, Available::Definite(a)) => {
                        let mode = IntrinsicMode::Fit((a - pb_sum.width).max(S::ZERO));
                        intrinsic_inline(pass, node, &style, flow, pb, mode, inner_height, &input)?
                    }
                };
                (inner + pb_sum.width).clamp_opt(min_w, max_w).max(pb_only.width)
            }
        },
    };
    run(pass, node, &style, flow, pb, width_bb, height_bb, height_indefinite, &input)
}

/// The container's intrinsic inline size: §9.9.1 for a row container (the inline axis is the
/// main axis), §9.9.2 for a column container (the inline axis is the cross axis).
#[allow(clippy::too_many_arguments)]
fn intrinsic_inline<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    style: &Style<S>,
    flow: Flow,
    pb: Edges<S>,
    mode: IntrinsicMode<S>,
    inner_height: Opt<S>,
    input: &Input<S>,
) -> Result<S, Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    match mode {
        IntrinsicMode::Max => {
            intrinsic_inline_one(pass, node, style, flow, pb, Available::MaxContent, inner_height, input)
        }
        IntrinsicMode::Min => {
            intrinsic_inline_one(pass, node, style, flow, pb, Available::MinContent, inner_height, input)
        }
        IntrinsicMode::Fit(avail) => {
            let max = intrinsic_inline_one(pass, node, style, flow, pb, Available::MaxContent, inner_height, input)?;
            if max <= avail {
                return Ok(max);
            }
            let min = intrinsic_inline_one(pass, node, style, flow, pb, Available::MinContent, inner_height, input)?;
            Ok(max.min(avail.max(min)))
        }
    }
}

/// One intrinsic inline size, under a min-content or max-content constraint. Items are
/// collected with an indefinite inline percentage basis: a cyclic percentage behaves as `auto`
/// for the purpose of intrinsic contributions (css-sizing-3 §5.2.1).
#[allow(clippy::too_many_arguments)]
fn intrinsic_inline_one<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    style: &Style<S>,
    flow: Flow,
    pb: Edges<S>,
    constraint: Available<S>,
    inner_height: Opt<S>,
    input: &Input<S>,
) -> Result<S, Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let pb_sum = pb.sums();
    let mut available = Size::new(constraint, match inner_height.get() {
        Some(h) => Available::Definite(h),
        None => input.available.height.shrink(pb_sum.height),
    });
    if flow.main == Axis::Vertical && inner_height.is_none() {
        // A column container's height is content-based: its items never wrap against the
        // available height, and are measured under a max-content constraint.
        available.height = Available::MaxContent;
    }
    let inner = Size::new(Opt::NONE, inner_height);
    let percent = Size::new(Opt::NONE, if input.indefinite.height { Opt::NONE } else { inner_height });
    let (gap_main, gap_cross) = gaps(style, flow, percent);
    let pb_only = padding_border(style, input.parent_size.width).sums();
    let bounds = inner_bounds(style, input, pb_only, pb_sum);
    let none = Size::new(Opt::NONE, Opt::NONE);
    let own = !input.content_size;
    let ctx = Ctx {
        flow,
        pb,
        border_gutter: Edges::ZERO,
        inner,
        percent,
        available,
        min_inner: if own { bounds.min_inner } else { none },
        max_inner: if own { bounds.max_inner } else { none },
        style_min_inner: bounds.min_inner,
        style_max_inner: bounds.max_inner,
        min_bb: if own { bounds.min_bb } else { none },
        max_bb: if own { bounds.max_bb } else { none },
        gap_main,
        gap_cross,
        justify: style.justify_content(),
        align_items: style.align_items(),
        align_content: style.align_content(),
        indefinite_height: input.indefinite.height,
    };

    let mut items = pass.cx.scratch.take_items();
    let mut paths = pass.cx.scratch.take_indices();
    let mut absolute = pass.cx.scratch.take_indices();
    items::collect(node, &ctx, &mut items, &mut paths, &mut absolute);
    let result = (|| {
        items::flex_base_sizes(pass, node, &ctx, &mut items, &paths)?;
        if flow.main == Axis::Horizontal {
            items::intrinsic_main_size(pass, node, &ctx, &items, &paths, constraint)
        } else {
            items::intrinsic_cross_size(pass, node, &ctx, &mut items, &paths, constraint)
        }
    })();
    pass.cx.scratch.give_items(items);
    pass.cx.scratch.give_indices(paths);
    pass.cx.scratch.give_indices(absolute);
    result
}

/// Resolves the two gaps for `flow` against the container's inner size; a percentage against
/// an indefinite size is zero.
fn gaps<S: Scalar>(style: &Style<S>, flow: Flow, inner: Size<Opt<S>>) -> (S, S) {
    let column_gap = style.resolve(slot::COLUMN_GAP, inner.width).unwrap_or(S::ZERO);
    let row_gap = style.resolve(slot::ROW_GAP, inner.height).unwrap_or(S::ZERO);
    match flow.main {
        Axis::Horizontal => (column_gap, row_gap),
        Axis::Vertical => (row_gap, column_gap),
    }
}

/// The flex layout proper, with a definite border-box width and a decided or auto height.
#[allow(clippy::too_many_arguments)]
fn run<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    style: &Style<S>,
    flow: Flow,
    pb: Edges<S>,
    width_bb: S,
    height_bb: Opt<S>,
    height_indefinite: bool,
    input: &Input<S>,
) -> Result<(Size<S>, Opt<S>), Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let pb_sum = pb.sums();
    let pb_only = padding_border(style, input.parent_size.width).sums();
    // A scrollbar gutter wider than the box is clamped to it, so the content box starts within
    // the border box (the inner size is zero either way).
    let pb = {
        let mut pb = pb;
        let gutter = scrollbar_gutter(style, flow.direction);
        let room = (width_bb - pb_only.width).max(S::ZERO);
        pb.left = pb.left - gutter.left + gutter.left.min(room);
        pb.right = pb.right - gutter.right + gutter.right.min(room);
        if let Some(h) = height_bb.get() {
            let room = (h - pb_only.height).max(S::ZERO);
            pb.bottom = pb.bottom - gutter.bottom + gutter.bottom.min(room);
        }
        pb
    };
    let inner_width = (width_bb - pb_sum.width).max(S::ZERO);
    let inner_height = height_bb.map(|h| (h - pb_sum.height).max(S::ZERO));
    let inner = Size::new(Opt::some(inner_width), inner_height);
    let percent = Size::new(Opt::some(inner_width), if height_indefinite { Opt::NONE } else { inner_height });
    // Percentage gaps resolve against definite sizes only.
    let (gap_main, gap_cross) = gaps(style, flow, percent);
    let bounds = inner_bounds(style, input, pb_only, pb_sum);
    let none = Size::new(Opt::NONE, Opt::NONE);
    let own = !input.content_size;
    let border = style.border();
    let gutter = scrollbar_gutter(style, flow.direction);
    let ctx = Ctx {
        flow,
        pb,
        border_gutter: Edges {
            left: border.left + gutter.left,
            right: border.right + gutter.right,
            top: border.top + gutter.top,
            bottom: border.bottom + gutter.bottom,
        },
        inner,
        percent,
        available: Size::new(Available::Definite(inner_width), match inner_height.get() {
            Some(h) => Available::Definite(h),
            None => input.available.height.shrink(pb_sum.height),
        }),
        min_inner: if own { bounds.min_inner } else { none },
        max_inner: if own { bounds.max_inner } else { none },
        style_min_inner: bounds.min_inner,
        style_max_inner: bounds.max_inner,
        min_bb: if own { bounds.min_bb } else { none },
        max_bb: if own { bounds.max_bb } else { none },
        gap_main,
        gap_cross,
        justify: style.justify_content(),
        align_items: style.align_items(),
        align_content: style.align_content(),
        indefinite_height: height_indefinite,
    };

    let mut items = pass.cx.scratch.take_items();
    let mut paths = pass.cx.scratch.take_indices();
    let mut absolute = pass.cx.scratch.take_indices();
    let mut flex_lines = pass.cx.scratch.take_lines();
    items::collect(node, &ctx, &mut items, &mut paths, &mut absolute);

    let decided = Size::new(Opt::some(width_bb), height_bb);
    let result = run_steps(pass, node, &ctx, &mut items, &paths, &absolute, &mut flex_lines, input, decided, pb_only);

    pass.cx.scratch.give_items(items);
    pass.cx.scratch.give_indices(paths);
    pass.cx.scratch.give_indices(absolute);
    pass.cx.scratch.give_lines(flex_lines);
    result
}

/// Steps 3 to 16 of §9 plus the final layout and §4.1, over collected items.
#[allow(clippy::too_many_arguments)]
fn run_steps<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    items: &mut [Item<S>],
    paths: &[u32],
    absolute: &[u32],
    flex_lines: &mut Vec<Line<S>>,
    input: &Input<S>,
    decided: Size<Opt<S>>,
    pb_only: Size<S>,
) -> Result<(Size<S>, Opt<S>), Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let main = ctx.main();
    let cross = ctx.cross();
    let pb_sum = ctx.pb.sums();

    // §9.2 step 3: flex base sizes and hypothetical main sizes.
    items::flex_base_sizes(pass, node, ctx, items, paths)?;

    // §9.2 step 4: the container's main size. The inline axis is already definite; a column
    // container's auto height is its max-content main size.
    let mut inner_main: S = match ctx.inner.get(main).get() {
        Some(v) => v,
        None => {
            let content = items::intrinsic_main_size(pass, node, ctx, items, paths, Available::MaxContent)?;
            content.clamp_opt(ctx.min_inner.get(main), ctx.max_inner.get(main))
        }
    };
    let main_definite = ctx.inner.get(main).is_some() && !(main == Axis::Vertical && ctx.indefinite_height);

    // §9.3 step 5: lines. A multi-line container with an auto main size breaks its lines
    // within its own max main size (even when asked for its content size) and is then as long
    // as its longest line.
    let limit = if ctx.inner.get(main).is_none() && ctx.flow.multiline {
        inner_main.clamp_opt(ctx.style_min_inner.get(main), ctx.style_max_inner.get(main))
    } else {
        inner_main
    };
    lines::collect(ctx, items, flex_lines, limit);
    if ctx.inner.get(main).is_none() && ctx.flow.multiline && flex_lines.len() > 1 {
        let longest = flex_lines.iter().fold(S::ZERO, |acc, l| acc.max(lines::line_length(ctx, &items[l.range()])));
        inner_main = longest.clamp_opt(ctx.min_inner.get(main), ctx.max_inner.get(main));
    }

    // §9.7 step 6: flexible lengths.
    for line in flex_lines.iter() {
        lines::resolve_flexible_lengths(pass, node, ctx, &mut items[line.range()], paths, inner_main)?;
    }

    // §9.4 steps 7 to 11 and 15: cross sizes.
    let inner_cross = lines::cross_sizes(pass, node, ctx, items, paths, flex_lines, inner_main)?;

    // The border-box size: a decided size stands (its content box may be squeezed to zero by
    // padding, border or a scrollbar gutter); a content-derived one adds the inset back.
    let mut size = Size::ZERO;
    for axis in [main, cross] {
        let inner = if axis == main { inner_main } else { inner_cross };
        let bb = decided
            .get(axis)
            .unwrap_or((inner + pb_sum.get(axis)).clamp_opt(ctx.min_bb.get(axis), ctx.max_bb.get(axis)))
            .max(pb_only.get(axis));
        size.set(axis, bb);
    }

    // §9.5 and §9.6: positions along both axes, in logical coordinates.
    place::distribute_main(ctx, items, flex_lines, inner_main);
    place::align_cross(ctx, items, flex_lines, inner_cross);

    if !input.final_layout {
        let baseline =
            place::container_baseline(pass, node, ctx, items, paths, flex_lines, inner_main, inner_cross, false)?;
        return Ok((size, baseline));
    }

    // Final layout: children at their used sizes, offsets into this list, then §4.1.
    let baseline =
        place::final_layout(pass, node, ctx, items, paths, flex_lines, inner_main, inner_cross, main_definite)?;
    place::absolute_children(pass, node, ctx, absolute, paths, size)?;
    Ok((size, baseline))
}

/// Whether a child style's `position` takes it out of flow.
#[inline]
fn is_absolute<S: Scalar>(style: &Style<S>) -> bool {
    style.position() == Position::Absolute
}

/// The two edges of `e` along `axis` as (logical start, logical end).
#[inline]
fn along<S: Scalar>(e: &Edges<S>, axis: Axis, reversed: bool) -> (S, S) {
    let (a, b) = e.along(axis);
    if reversed { (b, a) } else { (a, b) }
}

#[inline]
fn two<S: Scalar>() -> S {
    S::from_f32(2.0)
}

#[inline]
fn count<S: Scalar>(n: usize) -> S {
    S::from_f32(n as f32)
}
