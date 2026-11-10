//! Flex items: collection (§9.1), flex base and hypothetical main sizes (§9.2, §4.5) and the
//! intrinsic contributions of §9.9.

use super::{
    is_absolute, padding_border, resolve_bb, resolve_direction, transfer, transfer_bb, Ctx, F_AUTO_MIN, F_BASELINE,
    F_BASIS_DEFINITE, F_CONTENT_BASE, F_CONTENT_BOX, F_RELATIVE, F_SCROLL, F_STRETCHED, M_BOTTOM, M_LEFT, M_RIGHT, M_TOP,
};
use crate::geometry::{Axis, Edges, Point, Size};
use crate::measure::{Available, Measure};
use crate::pass::{Cancelled, Input, Pass};
use crate::scalar::{Opt, Scalar};
use crate::state::NodeKind;
use crate::style::{slot, AlignItems, Direction, Display, Position, Style, Unit};
use crate::tree::LayoutNode;
use alloc::vec::Vec;

/// Per-item working state, in a pooled `Vec` in the scratch.
#[derive(Copy, Clone)]
pub(crate) struct Item<S: Scalar> {
    /// Path to the child from this container: `path_len` indices starting at `path_start` in
    /// the paths pool. Longer than one for children of `display: contents` nodes.
    pub path_start: u32,
    pub path_len: u8,
    pub flags: u16,
    /// Auto margins, as `M_*` bits on physical edges.
    pub auto_margin: u8,
    pub violation: i8,
    /// Resolved `align-self`: `normal` is `stretch`, `self-start`/`self-end` are `start`/`end`.
    pub align: AlignItems,
    pub order: i32,
    /// Document order among the items; the tie-break when sorting by `order`.
    pub index: u32,
    pub grow: S,
    pub shrink: S,
    /// Margins, with `auto` as zero until distributed.
    pub margin: Edges<S>,
    /// Border-box sizes from the style where definite, with the aspect ratio applied.
    pub size: Size<Opt<S>>,
    /// Border-box min and max sizes from the style where definite (`auto`/`none` are none).
    pub min: Size<Opt<S>>,
    pub max: Size<Opt<S>>,
    /// Unit tags of `width` and `height`, for the sizing keywords.
    pub size_tag: [Unit; 2],
    /// A definite `flex-basis`, border-box.
    pub basis: Opt<S>,
    pub basis_tag: Unit,
    /// Padding plus border (plus gutter) per axis.
    pub pb: Size<S>,
    pub aspect_ratio: Opt<S>,
    /// Flex base size, hypothetical main size and target main size, all border-box.
    pub base: S,
    pub hypothetical: S,
    pub target: S,
    /// The used minimum main size: the style's, or the §4.5 automatic minimum.
    pub min_main: S,
    /// Hypothetical, then used, cross size, border-box.
    pub cross: S,
    /// First baseline below the border-box top, when a measurement produced one.
    pub baseline: Opt<S>,
    /// Logical offsets: along the main axis within the line, along the cross axis within the line.
    pub offset_main: S,
    pub offset_cross: S,
    /// Shift from `position: relative` insets, physical.
    pub inset: Point<S>,
}

impl<S: Scalar> Item<S> {
    #[inline]
    pub fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }
    #[inline]
    pub fn set(&mut self, flag: u16, on: bool) {
        if on {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }
    #[inline]
    pub fn path<'a>(&self, paths: &'a [u32]) -> &'a [u32] {
        &paths[self.path_start as usize..self.path_start as usize + self.path_len as usize]
    }
    #[inline]
    pub fn margin_auto(&self, axis: Axis) -> (bool, bool) {
        match axis {
            Axis::Horizontal => (self.auto_margin & M_LEFT != 0, self.auto_margin & M_RIGHT != 0),
            Axis::Vertical => (self.auto_margin & M_TOP != 0, self.auto_margin & M_BOTTOM != 0),
        }
    }
    /// Inner flex base size: the content box of the flex base size, floored at zero.
    #[inline]
    pub fn inner_base(&self, main: Axis) -> S {
        (self.base - self.pb.get(main)).max(S::ZERO)
    }
    /// The minimum on `axis`, transferred through the aspect ratio from the other axis when the
    /// style gives none on `axis`.
    pub fn transferred_min(&self, style_pb: Size<S>, content_box: bool, axis: Axis) -> Opt<S> {
        self.min.get(axis).or(self.transfer_other(self.min, style_pb, content_box, axis))
    }
    pub fn transferred_max(&self, style_pb: Size<S>, content_box: bool, axis: Axis) -> Opt<S> {
        self.max.get(axis).or(self.transfer_other(self.max, style_pb, content_box, axis))
    }
    fn transfer_other(&self, from: Size<Opt<S>>, pb: Size<S>, content_box: bool, axis: Axis) -> Opt<S> {
        match (self.aspect_ratio.get(), from.get(axis.cross()).get()) {
            (Some(r), Some(v)) => {
                let inner = if content_box { (v - pb.get(axis.cross())).max(S::ZERO) } else { v };
                let other = match axis.cross() {
                    Axis::Horizontal => inner / r,
                    Axis::Vertical => inner * r,
                };
                Opt::some(if content_box { other + pb.get(axis) } else { other })
            }
            _ => Opt::NONE,
        }
    }
}

/// Depth limit for nested `display: contents`; deeper nodes are laid out as ordinary items.
const MAX_CONTENTS_DEPTH: usize = 16;

/// §9.1: collects the in-flow children of `node` as items, flattening `display: contents`,
/// skipping `display: none`, setting absolutely positioned children aside (as `start, len`
/// pairs in `absolute`) and ordering by `order`.
pub(crate) fn collect<S, N>(node: &N, ctx: &Ctx<S>, items: &mut Vec<Item<S>>, paths: &mut Vec<u32>, absolute: &mut Vec<u32>)
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
{
    let mut stack = [0u32; MAX_CONTENTS_DEPTH];
    let mut index = 0u32;
    let mut any_order = false;
    collect_from(node, ctx, &mut stack, 0, items, paths, absolute, &mut index, &mut any_order);
    if any_order {
        // In place and allocation-free; the index makes it stable.
        items.sort_unstable_by(|a, b| a.order.cmp(&b.order).then(a.index.cmp(&b.index)));
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_from<S, N>(
    node: &N,
    ctx: &Ctx<S>,
    stack: &mut [u32; MAX_CONTENTS_DEPTH],
    depth: usize,
    items: &mut Vec<Item<S>>,
    paths: &mut Vec<u32>,
    absolute: &mut Vec<u32>,
    index: &mut u32,
    any_order: &mut bool,
) where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
{
    let parts = node.parts();
    for (i, slot) in parts.children.iter().enumerate() {
        let child: &N = &slot.node;
        let cp = child.parts();
        let cs = cp.style;
        if cs.display() == Display::None {
            continue;
        }
        stack[depth] = i as u32;
        let contents = cs.display() == Display::Contents || cp.state.kind() == NodeKind::Contents;
        if contents && depth + 1 < MAX_CONTENTS_DEPTH {
            collect_from(child, ctx, stack, depth + 1, items, paths, absolute, index, any_order);
            continue;
        }
        let start = paths.len() as u32;
        paths.extend_from_slice(&stack[..=depth]);
        if is_absolute(cs) {
            absolute.push(start);
            absolute.push(depth as u32 + 1);
            continue;
        }
        let item = make_item(cs, ctx, start, depth as u8 + 1, *index);
        *any_order |= item.order != 0;
        items.push(item);
        *index += 1;
    }
}

/// Resolves a child's style into an item against the container's constants.
fn make_item<S: Scalar>(cs: &Style<S>, ctx: &Ctx<S>, path_start: u32, path_len: u8, index: u32) -> Item<S> {
    let flow = ctx.flow;
    let main = flow.main;
    let cross = flow.cross;
    let direction = resolve_direction(cs.direction(), flow.direction);
    let pb = padding_border(cs, ctx.inner.width).sums();
    let margin = cs.resolve_edges(slot::MARGIN, ctx.inner.width);
    let mut auto_margin = 0u8;
    if cs.has_auto_margin() {
        for (k, bit) in [M_LEFT, M_RIGHT, M_TOP, M_BOTTOM].into_iter().enumerate() {
            if cs.tag(slot::MARGIN + k) == Unit::Auto {
                auto_margin |= bit;
            }
        }
    }

    let mut size = Size::new(Opt::NONE, Opt::NONE);
    let mut min = Size::new(Opt::NONE, Opt::NONE);
    let mut max = Size::new(Opt::NONE, Opt::NONE);
    let mut size_tag = [Unit::Auto; 2];
    for axis in [Axis::Horizontal, Axis::Vertical] {
        let (size_slot, min_slot, max_slot) = slot::sizing(axis);
        let basis = ctx.percent.get(axis);
        let (v, tag) = resolve_bb(cs, size_slot, basis, pb.get(axis));
        size.set(axis, v);
        size_tag[axis.index()] = tag;
        min.set(axis, resolve_bb(cs, min_slot, basis, pb.get(axis)).0);
        max.set(axis, resolve_bb(cs, max_slot, basis, pb.get(axis)).0);
    }
    let aspect_ratio = cs.aspect_ratio();
    if let Some(r) = aspect_ratio.get() {
        match (size.width.get(), size.height.get()) {
            (Some(w), None) => size.height = Opt::some(transfer(cs, pb, Axis::Horizontal, w, r)),
            (None, Some(h)) => size.width = Opt::some(transfer(cs, pb, Axis::Vertical, h, r)),
            _ => {}
        }
    }

    // A percentage flex basis resolves only against a definite main size.
    let main_percent = ctx.percent.get(main);
    let (basis, basis_tag) = resolve_bb(cs, slot::BASIS, main_percent, pb.get(main));

    let mut align = cs.align_self().unwrap_or(ctx.align_items);
    align = match align {
        AlignItems::Normal => AlignItems::Stretch,
        AlignItems::SelfStart | AlignItems::SelfEnd => {
            // Self-relative keywords follow the item's own inline direction, which can only
            // differ from the container's on a horizontal cross axis.
            let same = cross == Axis::Vertical || direction == flow.direction;
            match (align, same) {
                (AlignItems::SelfStart, true) | (AlignItems::SelfEnd, false) => AlignItems::Start,
                _ => AlignItems::End,
            }
        }
        a => a,
    };
    let (ms, me) = match cross {
        Axis::Horizontal => (auto_margin & M_LEFT != 0, auto_margin & M_RIGHT != 0),
        Axis::Vertical => (auto_margin & M_TOP != 0, auto_margin & M_BOTTOM != 0),
    };
    let cross_auto_margin = ms || me;
    let cross_tag = size_tag[cross.index()];
    let stretched = !cross_auto_margin
        && (cross_tag == Unit::Stretch || (align == AlignItems::Stretch && cross_tag == Unit::Auto));
    let scroll = cs.overflow_x().is_scroll_container() || cs.overflow_y().is_scroll_container();
    let baseline = align == AlignItems::Baseline && main == Axis::Horizontal && !cross_auto_margin;

    let mut inset = Point::ZERO;
    let relative = cs.position() == Position::Relative;
    if relative && cs.has_inset() {
        // Insets resolve against the containing block; a percentage against an indefinite
        // height is zero.
        let left = cs.resolve(slot::INSET, ctx.percent.width);
        let right = cs.resolve(slot::INSET + 1, ctx.percent.width);
        let top = cs.resolve(slot::INSET + 2, ctx.percent.height);
        let bottom = cs.resolve(slot::INSET + 3, ctx.percent.height);
        inset.x = match (left.get(), right.get()) {
            (Some(l), Some(r)) => if direction == Direction::Rtl { -r } else { l },
            (Some(l), None) => l,
            (None, Some(r)) => -r,
            (None, None) => S::ZERO,
        };
        inset.y = match (top.get(), bottom.get()) {
            (Some(t), _) => t,
            (None, Some(b)) => -b,
            (None, None) => S::ZERO,
        };
    }

    let mut flags = 0u16;
    if stretched {
        flags |= F_STRETCHED;
    }
    if scroll {
        flags |= F_SCROLL;
    }
    if baseline {
        flags |= F_BASELINE;
    }
    if relative {
        flags |= F_RELATIVE;
    }
    if cs.box_sizing() == crate::style::BoxSizing::ContentBox {
        flags |= F_CONTENT_BOX;
    }
    Item {
        path_start,
        path_len,
        flags,
        auto_margin,
        violation: 0,
        align,
        order: cs.order(),
        index,
        grow: cs.flex_grow(),
        shrink: cs.flex_shrink(),
        margin,
        size,
        min,
        max,
        size_tag,
        basis,
        basis_tag,
        pb,
        aspect_ratio,
        base: S::ZERO,
        hypothetical: S::ZERO,
        target: S::ZERO,
        min_main: S::ZERO,
        cross: S::ZERO,
        baseline: Opt::NONE,
        offset_main: S::ZERO,
        offset_cross: S::ZERO,
        inset,
    }
}

/// Asks the child behind `it` a sizing question. Children are always asked for their content
/// size: their own main-axis size, min and max properties are the container's business.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ask<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    it: &Item<S>,
    paths: &[u32],
    known: Size<Opt<S>>,
    available: Size<Available<S>>,
    indefinite: Size<bool>,
    final_layout: bool,
) -> Result<(Size<S>, Opt<S>), Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let mut input = Input::new(known, available, ctx.percent, ctx.flow.direction).content_size().with_indefinite(indefinite);
    if final_layout {
        input = input.finalize();
    }
    pass.visit_path(node, it.path(paths), input)
}

/// The cross-axis constraint an item is measured under: a known cross size (its definite style
/// size, or the stretch size), the available cross space less its margins, clamped by its cross
/// min and max, and whether the known size is definite in the §9.8 sense.
pub(crate) fn cross_constraint<S: Scalar>(ctx: &Ctx<S>, it: &Item<S>, content_box: bool) -> (Opt<S>, Available<S>, bool) {
    let cross = ctx.cross();
    let margins = it.margin.sum(cross);
    let tmin = it.transferred_min(it.pb, content_box, cross);
    let tmax = it.transferred_max(it.pb, content_box, cross);
    let available = match ctx.available.get(cross) {
        Available::Definite(v) => Available::Definite((v - margins).max(S::ZERO).clamp_opt(tmin, tmax)),
        Available::MinContent => match tmin.get() {
            Some(v) => Available::Definite(v),
            None => Available::MinContent,
        },
        Available::MaxContent => match tmax.get() {
            Some(v) => Available::Definite(v),
            None => Available::MaxContent,
        },
    };
    let mut known = it.size.get(cross).map(|v| v.clamp_opt(tmin, tmax));
    let mut definite = known.is_some();
    if it.has(F_STRETCHED) && known.is_none() {
        // §9.8: the stretched size of an item in a single-line container with a definite cross
        // size is definite, and is imposed. Otherwise the item is measured into the available
        // space, so a cached answer can be reused when that space changes (Yoga's rules).
        let container_cross_definite =
            ctx.inner.get(cross).is_some() && !(cross == Axis::Vertical && ctx.indefinite_height);
        if !ctx.flow.multiline && container_cross_definite {
            known = available.definite();
            definite = true;
        }
    }
    (known, available, definite)
}

/// §9.2 step 3: the flex base size, the §4.5 automatic minimum and the hypothetical main size of
/// every item.
pub(crate) fn flex_base_sizes<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    items: &mut [Item<S>],
    paths: &[u32],
) -> Result<(), Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let main = ctx.main();
    let cross = ctx.cross();
    for item in items.iter_mut() {
        let it = *item;
        let content_box = it.has(F_CONTENT_BOX);
        let (cross_known, cross_available, cross_definite) = cross_constraint(ctx, &it, content_box);
        let main_tag = it.size_tag[main.index()];
        let main_stretch = ctx.inner.get(main).map(|v| (v - it.margin.sum(main)).max(S::ZERO));

        // The main-axis constraint a content-based basis is measured under: the container's own
        // intrinsic constraint (case C), or max-content (case E), unless a keyword says otherwise.
        let keyword = |tag: Unit| -> Option<Available<S>> {
            match tag {
                Unit::MinContent => Some(Available::MinContent),
                Unit::MaxContent => Some(Available::MaxContent),
                Unit::FitContent => Some(match main_stretch.get() {
                    Some(v) => Available::Definite(v),
                    None => Available::MaxContent,
                }),
                _ => None,
            }
        };
        let default_constraint = match ctx.available.get(main) {
            Available::MinContent => Available::MinContent,
            _ => Available::MaxContent,
        };

        let mut flags = it.flags & !(F_CONTENT_BASE | F_BASIS_DEFINITE);
        let mut base: S;
        'basis: {
            // A. A definite flex basis, or (with `flex-basis: auto`) a definite main size.
            if let Some(b) = it.basis.get() {
                base = b;
                flags |= F_BASIS_DEFINITE;
                break 'basis;
            }
            let measure_under: Option<Available<S>> = match it.basis_tag {
                Unit::Auto => {
                    if let Some(m) = it.size.get(main).get() {
                        base = m;
                        flags |= F_BASIS_DEFINITE;
                        break 'basis;
                    }
                    if main_tag == Unit::Stretch
                        && let Some(v) = main_stretch.get()
                    {
                        base = v;
                        flags |= F_BASIS_DEFINITE;
                        break 'basis;
                    }
                    keyword(main_tag)
                }
                Unit::Stretch => {
                    if let Some(v) = main_stretch.get() {
                        base = v;
                        flags |= F_BASIS_DEFINITE;
                        break 'basis;
                    }
                    None
                }
                tag => keyword(tag),
            };
            // B. An aspect ratio and a definite cross size: transfer.
            if cross_definite
                && let (Some(r), Some(c)) = (it.aspect_ratio.get(), cross_known.get())
            {
                base = transfer_bb(content_box, it.pb, cross, c, r);
                flags |= F_BASIS_DEFINITE;
                break 'basis;
            }
            // C and E: size the item into the available space with the basis as its main size.
            let constraint = measure_under.unwrap_or(default_constraint);
            if measure_under.is_none() && constraint == Available::MaxContent {
                flags |= F_CONTENT_BASE;
            }
            let mut known = Size::new(Opt::NONE, Opt::NONE);
            known.set(cross, cross_known);
            let mut available = Size::new(Available::MaxContent, Available::MaxContent);
            available.set(main, constraint);
            available.set(cross, cross_available);
            let (measured, _) = ask(pass, node, ctx, &it, paths, known, available, Size::new(false, false), false)?;
            base = measured.get(main);
        }
        // The flex base size is floored at the item's padding plus border, as Chrome does.
        base = base.max(it.pb.get(main));
        item.base = base;

        // §4.5: the used minimum main size.
        let mut flags = flags & !F_AUTO_MIN;
        let min_main = if let Some(m) = it.min.get(main).get() {
            m
        } else if it.has(F_SCROLL) {
            it.pb.get(main)
        } else if main == Axis::Horizontal && flags & F_CONTENT_BASE != 0 {
            // A content-based base is at least the min-content size, so the automatic minimum
            // can only matter when the line shrinks; it is resolved there (`resolve_auto_min`).
            flags |= F_AUTO_MIN;
            it.pb.get(main)
        } else if it.size.get(main).get().is_some_and(|s| base >= s) {
            // The automatic minimum is clamped by the specified size suggestion, so with a
            // definite main size no larger than the base it cannot raise the hypothetical main
            // size either: it matters only when the line shrinks. Deferring it saves the
            // min-content question (a size query of the whole subtree) for every fixed-size item.
            flags |= F_AUTO_MIN;
            it.pb.get(main)
        } else {
            // Content size suggestion: the min-content main size. In the block axis that is the
            // content height already measured for a content-based basis.
            let min_content = if main == Axis::Vertical && flags & F_CONTENT_BASE != 0 {
                base
            } else {
                let mut known = Size::new(Opt::NONE, Opt::NONE);
                known.set(cross, cross_known);
                let mut available = Size::new(Available::MinContent, Available::MinContent);
                available.set(cross, cross_available);
                ask(pass, node, ctx, &it, paths, known, available, Size::new(false, false), false)?.0.get(main)
            };
            // Clamped by the specified size suggestion (which includes a size transferred
            // through the aspect ratio) and by the max main size.
            let tmax = it.transferred_max(it.pb, content_box, main);
            let mut m = min_content;
            if let Some(s) = it.size.get(main).get() {
                m = m.min(s);
            }
            if let Some(x) = tmax.get() {
                m = m.min(x);
            }
            m.max(it.pb.get(main))
        };
        item.min_main = min_main;
        item.flags = flags;

        // The hypothetical main size: the base clamped by the used min and max, with sizes
        // transferred through the aspect ratio applying here only.
        let tmin = it.transferred_min(it.pb, content_box, main).unwrap_or(S::ZERO).max(min_main).max(it.pb.get(main));
        let tmax = it.transferred_max(it.pb, content_box, main);
        item.hypothetical = base.clamp_opt(Opt::some(tmin), tmax);
    }
    Ok(())
}

/// Resolves a deferred §4.5 automatic minimum (`F_AUTO_MIN`): the min-content main size, clamped
/// by the specified size suggestion and the max main size, floored at padding plus border.
pub(crate) fn resolve_auto_min<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    it: &mut Item<S>,
    paths: &[u32],
) -> Result<(), Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    if !it.has(F_AUTO_MIN) {
        return Ok(());
    }
    let main = ctx.main();
    let cross = ctx.cross();
    let content_box = it.has(F_CONTENT_BOX);
    let (cross_known, cross_available, _) = cross_constraint(ctx, it, content_box);
    let mut known = Size::new(Opt::NONE, Opt::NONE);
    known.set(cross, cross_known);
    let mut available = Size::new(Available::MinContent, Available::MinContent);
    available.set(cross, cross_available);
    let min_content = ask(pass, node, ctx, it, paths, known, available, Size::new(false, false), false)?.0.get(main);
    let mut m = min_content;
    if let Some(s) = it.size.get(main).get() {
        m = m.min(s);
    }
    if let Some(x) = it.transferred_max(it.pb, content_box, main).get() {
        m = m.min(x);
    }
    it.min_main = m.max(it.pb.get(main));
    it.set(F_AUTO_MIN, false);
    Ok(())
}

/// §9.9.1.2, as Chrome implements it: the container's inner max-content (or min-content) main
/// size is the sum, over one infinite line, of each item's flex base size adjusted by the
/// difference to its content contribution where its flex factors allow, clamped by its min and
/// max main sizes, plus the gaps. For a wrapping row container under a min-content constraint
/// it is the largest contribution instead.
pub(crate) fn intrinsic_main_size<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    items: &[Item<S>],
    paths: &[u32],
    constraint: Available<S>,
) -> Result<S, Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let main = ctx.main();
    let cross = ctx.cross();
    let is_row = main == Axis::Horizontal;
    let min_content = constraint == Available::MinContent;
    let mut sum = S::ZERO;
    let mut largest = S::ZERO;
    for it in items.iter() {
        let it = *it;
        let margin = it.margin.sum(main);
        let style_min = it.min.get(main);
        let style_max = it.max.get(main);
        let pref = it.size.get(main);
        // §9.9.3: the content contribution, "clamped by its flex base size as a maximum (if it is
        // not growable) and/or as a minimum (if it is not shrinkable), and then further clamped by
        // its min/max main size"; margins are added outside every clamp (css-sizing-3 §5.1: min
        // and max sizes never apply to margins).
        let clamping_basis = if is_row { pref.map(|p| p.max(it.base)).unwrap_or(it.base) } else { it.base };
        let inflexible = it.grow == S::ZERO && it.shrink == S::ZERO;
        let min_c = style_min.unwrap_or(S::ZERO).max(it.min_main);
        // The clamps decide regardless of the content when the item is inflexible (its
        // contribution is its base) or its maximum is below its minimum: no measurement needed.
        let settled = inflexible || style_max.get().is_some_and(|x| x <= min_c);
        let content: S = if settled {
            clamping_basis
        } else if let Some(p) = pref.get() {
            let inner = p.max(it.pb.get(main));
            if is_row { inner } else { inner.max(it.base) }
        } else if it.has(F_SCROLL) {
            it.base
        } else {
            let measured = if !min_content && it.has(F_CONTENT_BASE) {
                it.base
            } else {
                let content_box = it.has(F_CONTENT_BOX);
                let (cross_known, cross_available, _) = cross_constraint(ctx, &it, content_box);
                let mut known = Size::new(Opt::NONE, Opt::NONE);
                known.set(cross, cross_known);
                let mut available = Size::new(constraint, constraint);
                available.set(cross, cross_available);
                let m = ask(pass, node, ctx, &it, paths, known, available, Size::new(false, false), false)?.0.get(main);
                match (it.aspect_ratio.get(), cross_known.get()) {
                    (Some(r), Some(c)) => m.max(transfer_bb(content_box, it.pb, cross, c, r)),
                    _ => m,
                }
            };
            if is_row { measured } else { measured.max(it.base) }
        };
        let mut inner = content;
        if it.shrink == S::ZERO {
            inner = inner.max(clamping_basis);
        }
        if it.grow == S::ZERO {
            inner = inner.min(clamping_basis);
        }
        let contribution = inner.clamp_opt(Opt::some(min_c), style_max) + margin;
        // The item's share of the line: its base adjusted towards its contribution where its
        // flex factors allow (a shrink factor on a zero inner base cannot shrink).
        let diff = contribution - margin - it.base;
        let adjustment = if diff > S::ZERO {
            diff
        } else if diff < S::ZERO {
            if it.inner_base(main) == S::ZERO { S::ZERO } else { diff }
        } else {
            S::ZERO
        };
        let size = it.base + adjustment + margin;
        largest = largest.max(size);
        sum = sum + size;
    }
    let n = items.len();
    if min_content && ctx.flow.multiline {
        return Ok(largest);
    }
    let gaps = if n > 1 { ctx.gap_main * S::from_f32((n - 1) as f32) } else { S::ZERO };
    let total = sum + gaps;
    // A wrapping row container is at least as large as its largest item (negative margins).
    Ok(if is_row && ctx.flow.multiline { total.max(largest) } else { total })
}

/// §9.9.2: the inner intrinsic cross size of a (column) container: the largest outer cross
/// contribution of its items, or, when it wraps, the sum of its lines' largest contributions
/// after breaking at the container's main size.
pub(crate) fn intrinsic_cross_size<S, N, M>(
    pass: &mut Pass<'_, '_, N, M>,
    node: &mut N,
    ctx: &Ctx<S>,
    items: &mut [Item<S>],
    paths: &[u32],
    constraint: Available<S>,
) -> Result<S, Cancelled>
where
    S: Scalar,
    N: LayoutNode<Scalar = S>,
    M: Measure<N>,
{
    let main = ctx.main();
    let cross = ctx.cross();
    for item in items.iter_mut() {
        let it = *item;
        let content_box = it.has(F_CONTENT_BOX);
        let tmin = it.transferred_min(it.pb, content_box, cross);
        let tmax = it.transferred_max(it.pb, content_box, cross);
        let inner = match it.size.get(cross).get() {
            Some(c) => c,
            None => {
                let mut known = Size::new(Opt::NONE, Opt::NONE);
                // The item's main size is its hypothetical main size.
                known.set(main, Opt::some(it.hypothetical));
                let mut available = Size::new(constraint, constraint);
                available.set(main, Available::Definite(it.hypothetical));
                let m = ask(pass, node, ctx, &it, paths, known, available, Size::new(false, false), false)?.0.get(cross);
                match it.aspect_ratio.get() {
                    Some(r) => m.max(transfer_bb(content_box, it.pb, main, it.hypothetical, r)),
                    None => m,
                }
            }
        };
        item.cross = inner.clamp_opt(tmin, tmax).max(it.pb.get(cross));
    }
    let outer = |it: &Item<S>| it.cross + it.margin.sum(cross);
    if !ctx.flow.multiline || items.len() < 2 {
        return Ok(items.iter().fold(S::ZERO, |acc, it| acc.max(outer(it))));
    }
    // Break lines against the container's main size, as the layout will.
    let inner_main = match ctx.inner.get(main).get() {
        Some(v) => v,
        None => {
            let content = intrinsic_main_size(pass, node, ctx, items, paths, Available::MaxContent)?;
            content.clamp_opt(ctx.min_inner.get(main), ctx.max_inner.get(main))
        }
    };
    let mut lines = pass.cx.scratch.take_lines();
    super::lines::collect(ctx, items, &mut lines, inner_main);
    let mut total = S::ZERO;
    for (i, line) in lines.iter().enumerate() {
        let largest = items[line.range()].iter().fold(S::ZERO, |acc, it| acc.max(outer(it)));
        total = total + largest + if i > 0 { ctx.gap_cross } else { S::ZERO };
    }
    pass.cx.scratch.give_lines(lines);
    Ok(total)
}
