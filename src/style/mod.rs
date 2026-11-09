//! The flex style: one flat, pointer-free, 144-byte value.
//!
//! [`Style`] is built with [`Style::web`] (CSS initial values, also `Default`) or
//! [`Style::yoga`] (Yoga's and React Native's initial values) and then refined with `with_*`
//! builders or `set_*` setters. Properties are read back through typed accessors. The
//! representation is private: keyword properties share one bit word, and every length is a
//! four-bit unit tag plus a bare scalar payload, so the same layout serves `f32`, `f64` and
//! fixed point, carries no pointer, and can be compared or fingerprinted byte-wise.
//!
//! Percentages are fractions: `Length::Percent(0.5)` is `50%`, as in Yoga and Taffy.

mod keywords;

pub use keywords::{
    AlignContent, AlignItems, BoxSizing, Direction, Display, FlexDirection, FlexWrap, JustifyContent, Overflow,
    Position,
};
pub(crate) use keywords::Unit;

use crate::geometry::{Edges, Size};
use crate::scalar::{Opt, Scalar};
use core::fmt;

/// A CSS length or sizing keyword.
///
/// `Percent` holds a fraction (`0.5` is `50%`). `None` is the initial value of `max-width` and
/// `max-height`. `Content` is valid only as a `flex-basis`.
#[non_exhaustive]
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Length<S> {
    /// `auto`.
    Auto,
    /// An absolute length in layout units.
    Px(S),
    /// A fraction of the containing block's size on the relevant axis.
    Percent(S),
    /// `min-content`.
    MinContent,
    /// `max-content`.
    MaxContent,
    /// `fit-content`.
    FitContent,
    /// `stretch`.
    Stretch,
    /// `content` (flex-basis only).
    Content,
    /// `none` (max sizes only).
    None,
}

impl<S: Scalar> Length<S> {
    /// `Px(0)`.
    #[inline]
    pub fn zero() -> Self {
        Length::Px(S::ZERO)
    }
    /// Resolves an absolute or percentage length against `basis`; keywords resolve to none.
    #[inline]
    pub fn resolve(self, basis: Opt<S>) -> Opt<S> {
        match self {
            Length::Px(v) => Opt::some(v),
            Length::Percent(p) => basis.map(|b| b * p),
            _ => Opt::NONE,
        }
    }
    /// True for `auto`.
    #[inline]
    pub fn is_auto(self) -> bool {
        matches!(self, Length::Auto)
    }
}

impl<S: Scalar> Default for Length<S> {
    #[inline]
    fn default() -> Self {
        Length::Auto
    }
}

/// Indices of the tagged lengths inside `Style`. Crate-private; the algorithm reads lengths
/// by slot without going through the enum.
pub(crate) mod slot {
    pub const BASIS: usize = 0;
    pub const WIDTH: usize = 1;
    pub const HEIGHT: usize = 2;
    pub const MIN_WIDTH: usize = 3;
    pub const MIN_HEIGHT: usize = 4;
    pub const MAX_WIDTH: usize = 5;
    pub const MAX_HEIGHT: usize = 6;
    pub const MARGIN: usize = 7; // left, right, top, bottom
    pub const PADDING: usize = 11; // left, right, top, bottom
    pub const INSET: usize = 15; // left, right, top, bottom
    pub const ROW_GAP: usize = 19;
    pub const COLUMN_GAP: usize = 20;
    pub const COUNT: usize = 21;
    pub const TAG_BYTES: usize = COUNT.div_ceil(2);

    /// `(size, min, max)` slots for an axis.
    pub const fn sizing(axis: crate::Axis) -> (usize, usize, usize) {
        match axis {
            crate::Axis::Horizontal => (WIDTH, MIN_WIDTH, MAX_WIDTH),
            crate::Axis::Vertical => (HEIGHT, MIN_HEIGHT, MAX_HEIGHT),
        }
    }
}

/// Bit fields of `Style::bits`, as `(shift, width)`, packed back to back with widths taken
/// from the keyword enums themselves.
mod bits {
    use super::*;
    const fn next(prev: (u32, u32), width: u32) -> (u32, u32) {
        (prev.0 + prev.1, width)
    }
    pub const DISPLAY: (u32, u32) = (0, Display::BITS);
    pub const POSITION: (u32, u32) = next(DISPLAY, Position::BITS);
    pub const DIRECTION: (u32, u32) = next(POSITION, Direction::BITS);
    pub const FLEX_DIRECTION: (u32, u32) = next(DIRECTION, FlexDirection::BITS);
    pub const FLEX_WRAP: (u32, u32) = next(FLEX_DIRECTION, FlexWrap::BITS);
    pub const BOX_SIZING: (u32, u32) = next(FLEX_WRAP, BoxSizing::BITS);
    pub const OVERFLOW_X: (u32, u32) = next(BOX_SIZING, Overflow::BITS);
    pub const OVERFLOW_Y: (u32, u32) = next(OVERFLOW_X, Overflow::BITS);
    pub const JUSTIFY_CONTENT: (u32, u32) = next(OVERFLOW_Y, JustifyContent::BITS);
    pub const ALIGN_ITEMS: (u32, u32) = next(JUSTIFY_CONTENT, AlignItems::BITS);
    /// `0` is `auto`; otherwise `AlignItems as u8 + 1`, so one more value than the enum.
    pub const ALIGN_SELF: (u32, u32) = next(ALIGN_ITEMS, AlignItems::BITS + 1);
    pub const ALIGN_CONTENT: (u32, u32) = next(ALIGN_SELF, AlignContent::BITS);
    pub const USED: u32 = ALIGN_CONTENT.0 + ALIGN_CONTENT.1;
    const _: () = assert!(USED <= 64);
}

/// Every property that affects flex layout, in 144 bytes for `f32`.
///
/// See the [module documentation](self) for the representation and the builders below for use.
#[repr(C)]
#[derive(Copy, Clone, PartialEq)]
pub struct Style<S: Scalar = f32> {
    bits: u64,
    tags: [u8; slot::TAG_BYTES],
    _pad: u8,
    flex_grow: S,
    flex_shrink: S,
    lengths: [S; slot::COUNT],
    border: Edges<S>,
    aspect_ratio: Opt<S>,
    order: i32,
    scrollbar_gutter: S,
}

const _: () = assert!(core::mem::size_of::<Style<f32>>() == 144);

impl<S: Scalar> Default for Style<S> {
    #[inline]
    fn default() -> Self {
        Self::web()
    }
}

// ----------------------------------------------------------------------------------------------
// presets
// ----------------------------------------------------------------------------------------------

impl<S: Scalar> Style<S> {
    /// The CSS initial values, with one forced exception: `display` is `flex`, because a
    /// flexbox-only engine has no `inline` or `block` to default to.
    ///
    /// Row direction, no wrap, `flex: 0 1 auto`, `align-items: normal` (stretch),
    /// `align-content: normal` (stretch), `justify-content: normal` (flex-start),
    /// `min-size: auto`, `max-size: none`, `position: static`, `box-sizing: content-box`,
    /// zero margins, padding, border and gaps, `inset: auto`, no aspect ratio.
    pub fn web() -> Self {
        let mut s = Style {
            bits: 0,
            tags: [0; slot::TAG_BYTES],
            _pad: 0,
            flex_grow: S::ZERO,
            flex_shrink: S::ONE,
            lengths: [S::ZERO; slot::COUNT],
            border: Edges::ZERO,
            aspect_ratio: Opt::NONE,
            order: 0,
            scrollbar_gutter: S::ZERO,
        };
        s.set_length(slot::BASIS, Length::Auto);
        s.set_length(slot::WIDTH, Length::Auto);
        s.set_length(slot::HEIGHT, Length::Auto);
        s.set_length(slot::MIN_WIDTH, Length::Auto);
        s.set_length(slot::MIN_HEIGHT, Length::Auto);
        s.set_length(slot::MAX_WIDTH, Length::None);
        s.set_length(slot::MAX_HEIGHT, Length::None);
        for i in 0..4 {
            s.set_length(slot::MARGIN + i, Length::zero());
            s.set_length(slot::PADDING + i, Length::zero());
            s.set_length(slot::INSET + i, Length::Auto);
        }
        s.set_length(slot::ROW_GAP, Length::zero());
        s.set_length(slot::COLUMN_GAP, Length::zero());
        s
    }

    /// Yoga's and React Native's initial values: column direction, `flex-shrink: 0`,
    /// `position: relative`, `box-sizing: border-box`, `align-content: flex-start`.
    ///
    /// This is a different set of initial values and nothing more. There is one algorithm;
    /// Yoga's errata are not implemented.
    pub fn yoga() -> Self {
        Self::web()
            .with_flex_direction(FlexDirection::Column)
            .with_flex_shrink(S::ZERO)
            .with_position(Position::Relative)
            .with_box_sizing(BoxSizing::BorderBox)
            .with_align_content(AlignContent::FlexStart)
    }
}

// ----------------------------------------------------------------------------------------------
// representation helpers
// ----------------------------------------------------------------------------------------------

impl<S: Scalar> Style<S> {
    #[inline]
    fn field(&self, (shift, width): (u32, u32)) -> u8 {
        ((self.bits >> shift) & ((1u64 << width) - 1)) as u8
    }
    #[inline]
    fn set_field(&mut self, (shift, width): (u32, u32), v: u8) {
        let mask = ((1u64 << width) - 1) << shift;
        self.bits = (self.bits & !mask) | (((v as u64) << shift) & mask);
    }
    #[inline]
    #[allow(clippy::manual_is_multiple_of)] // `is_multiple_of` needs Rust 1.87; keep the MSRV low
    pub(crate) fn tag(&self, i: usize) -> Unit {
        let b = self.tags[i / 2];
        Unit::from_u8(if i % 2 == 0 { b & 0xF } else { b >> 4 })
    }
    #[inline]
    #[allow(clippy::manual_is_multiple_of)]
    fn set_tag(&mut self, i: usize, u: Unit) {
        let b = &mut self.tags[i / 2];
        if i % 2 == 0 {
            *b = (*b & 0xF0) | (u as u8)
        } else {
            *b = (*b & 0x0F) | ((u as u8) << 4)
        }
    }
    /// The length in `slot`.
    #[inline]
    pub(crate) fn length(&self, i: usize) -> Length<S> {
        match self.tag(i) {
            Unit::Auto => Length::Auto,
            Unit::Px => Length::Px(self.lengths[i]),
            Unit::Percent => Length::Percent(self.lengths[i]),
            Unit::MinContent => Length::MinContent,
            Unit::MaxContent => Length::MaxContent,
            Unit::FitContent => Length::FitContent,
            Unit::Stretch => Length::Stretch,
            Unit::Content => Length::Content,
            Unit::None => Length::None,
        }
    }
    #[inline]
    pub(crate) fn set_length(&mut self, i: usize, l: Length<S>) {
        let (u, v) = match l {
            Length::Auto => (Unit::Auto, S::ZERO),
            Length::Px(v) => (Unit::Px, v),
            Length::Percent(v) => (Unit::Percent, v),
            Length::MinContent => (Unit::MinContent, S::ZERO),
            Length::MaxContent => (Unit::MaxContent, S::ZERO),
            Length::FitContent => (Unit::FitContent, S::ZERO),
            Length::Stretch => (Unit::Stretch, S::ZERO),
            Length::Content => (Unit::Content, S::ZERO),
            Length::None => (Unit::None, S::ZERO),
        };
        self.set_tag(i, u);
        self.lengths[i] = v;
    }
    /// Resolves the length in `slot` against `basis`; keywords resolve to none.
    #[inline]
    pub(crate) fn resolve(&self, i: usize, basis: Opt<S>) -> Opt<S> {
        match self.tag(i) {
            Unit::Px => Opt::some(self.lengths[i]),
            Unit::Percent => basis.map(|b| b * self.lengths[i]),
            _ => Opt::NONE,
        }
    }
    #[inline]
    fn edges(&self, base: usize) -> Edges<Length<S>> {
        Edges {
            left: self.length(base),
            right: self.length(base + 1),
            top: self.length(base + 2),
            bottom: self.length(base + 3),
        }
    }
    #[inline]
    fn set_edges(&mut self, base: usize, e: Edges<Length<S>>) {
        self.set_length(base, e.left);
        self.set_length(base + 1, e.right);
        self.set_length(base + 2, e.top);
        self.set_length(base + 3, e.bottom);
    }
    /// Resolves four edge lengths against `basis` (percent margins and padding resolve
    /// against the containing block's inline size, so one basis serves all four).
    #[inline]
    pub(crate) fn resolve_edges(&self, base: usize, basis: Opt<S>) -> Edges<S> {
        Edges {
            left: self.resolve(base, basis).unwrap_or(S::ZERO),
            right: self.resolve(base + 1, basis).unwrap_or(S::ZERO),
            top: self.resolve(base + 2, basis).unwrap_or(S::ZERO),
            bottom: self.resolve(base + 3, basis).unwrap_or(S::ZERO),
        }
    }

    /// True if any length is a percentage or `stretch`, which makes the node's size depend on
    /// its containing block's size.
    pub fn uses_percentages(&self) -> bool {
        (0..slot::COUNT).any(|i| matches!(self.tag(i), Unit::Percent | Unit::Stretch))
    }

    /// A 64-bit fingerprint over every layout-affecting field. The default
    /// [`LayoutNode::style_key`](crate::LayoutNode::style_key).
    pub fn fingerprint(&self) -> u64 {
        const PRIME: u64 = 0x0100_0000_01b3;
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |w: u64| {
            h ^= w;
            h = h.wrapping_mul(PRIME);
        };
        mix(self.bits);
        let mut tags = [0u8; 16];
        tags[..slot::TAG_BYTES].copy_from_slice(&self.tags);
        mix(u64::from_le_bytes(tags[..8].try_into().unwrap()));
        mix(u64::from_le_bytes(tags[8..].try_into().unwrap()));
        mix(self.flex_grow.key_bits() as u64);
        mix(self.flex_shrink.key_bits() as u64);
        for v in self.lengths.iter() {
            mix(v.key_bits() as u64);
        }
        for v in [self.border.left, self.border.right, self.border.top, self.border.bottom] {
            mix(v.key_bits() as u64);
        }
        mix(self.aspect_ratio.key_bits() as u64);
        mix(self.order as u32 as u64);
        mix(self.scrollbar_gutter.key_bits() as u64);
        h
    }
}

// ----------------------------------------------------------------------------------------------
// property accessors
// ----------------------------------------------------------------------------------------------

macro_rules! keyword_props {
    ($( $(#[$m:meta])* $get:ident / $set:ident / $with:ident : $ty:ty = $field:expr ;)*) => {
        impl<S: Scalar> Style<S> {
            $(
                $(#[$m])*
                #[inline]
                pub fn $get(&self) -> $ty { <$ty>::from_u8(self.field($field)) }
                #[doc = concat!("Sets `", stringify!($get), "`.")]
                #[inline]
                pub fn $set(&mut self, v: $ty) { self.set_field($field, v as u8) }
                #[doc = concat!("Returns the style with `", stringify!($get), "` set.")]
                #[inline]
                #[must_use]
                pub fn $with(mut self, v: $ty) -> Self { self.set_field($field, v as u8); self }
            )*
        }
    };
}

keyword_props! {
    /// The `display` property.
    display / set_display / with_display : Display = bits::DISPLAY;
    /// The `position` property.
    position / set_position / with_position : Position = bits::POSITION;
    /// The `direction` property.
    direction / set_direction / with_direction : Direction = bits::DIRECTION;
    /// The `flex-direction` property.
    flex_direction / set_flex_direction / with_flex_direction : FlexDirection = bits::FLEX_DIRECTION;
    /// The `flex-wrap` property.
    flex_wrap / set_flex_wrap / with_flex_wrap : FlexWrap = bits::FLEX_WRAP;
    /// The `box-sizing` property.
    box_sizing / set_box_sizing / with_box_sizing : BoxSizing = bits::BOX_SIZING;
    /// The `overflow-x` property.
    overflow_x / set_overflow_x / with_overflow_x : Overflow = bits::OVERFLOW_X;
    /// The `overflow-y` property.
    overflow_y / set_overflow_y / with_overflow_y : Overflow = bits::OVERFLOW_Y;
    /// The `justify-content` property.
    justify_content / set_justify_content / with_justify_content : JustifyContent = bits::JUSTIFY_CONTENT;
    /// The `align-items` property.
    align_items / set_align_items / with_align_items : AlignItems = bits::ALIGN_ITEMS;
    /// The `align-content` property.
    align_content / set_align_content / with_align_content : AlignContent = bits::ALIGN_CONTENT;
}

macro_rules! length_props {
    ($( $(#[$m:meta])* $get:ident / $set:ident / $with:ident = $slot:expr ;)*) => {
        impl<S: Scalar> Style<S> {
            $(
                $(#[$m])*
                #[inline]
                pub fn $get(&self) -> Length<S> { self.length($slot) }
                #[doc = concat!("Sets `", stringify!($get), "`.")]
                #[inline]
                pub fn $set(&mut self, v: Length<S>) { self.set_length($slot, v) }
                #[doc = concat!("Returns the style with `", stringify!($get), "` set.")]
                #[inline]
                #[must_use]
                pub fn $with(mut self, v: Length<S>) -> Self { self.set_length($slot, v); self }
            )*
        }
    };
}

length_props! {
    /// The `flex-basis` property.
    flex_basis / set_flex_basis / with_flex_basis = slot::BASIS;
    /// The `width` property.
    width / set_width / with_width = slot::WIDTH;
    /// The `height` property.
    height / set_height / with_height = slot::HEIGHT;
    /// The `min-width` property.
    min_width / set_min_width / with_min_width = slot::MIN_WIDTH;
    /// The `min-height` property.
    min_height / set_min_height / with_min_height = slot::MIN_HEIGHT;
    /// The `max-width` property.
    max_width / set_max_width / with_max_width = slot::MAX_WIDTH;
    /// The `max-height` property.
    max_height / set_max_height / with_max_height = slot::MAX_HEIGHT;
    /// The `row-gap` property: space between lines.
    row_gap / set_row_gap / with_row_gap = slot::ROW_GAP;
    /// The `column-gap` property: space between items in a line.
    column_gap / set_column_gap / with_column_gap = slot::COLUMN_GAP;
}

impl<S: Scalar> Style<S> {
    /// `align-self`; `None` is `auto`, which takes the parent's `align-items`.
    #[inline]
    pub fn align_self(&self) -> Option<AlignItems> {
        match self.field(bits::ALIGN_SELF) {
            0 => None,
            v => Some(AlignItems::from_u8(v - 1)),
        }
    }
    /// Sets `align-self`.
    #[inline]
    pub fn set_align_self(&mut self, v: Option<AlignItems>) {
        self.set_field(bits::ALIGN_SELF, v.map_or(0, |a| a as u8 + 1));
    }
    /// Returns the style with `align-self` set.
    #[inline]
    #[must_use]
    pub fn with_align_self(mut self, v: Option<AlignItems>) -> Self {
        self.set_align_self(v);
        self
    }

    /// `width` and `height` together.
    #[inline]
    pub fn size(&self) -> Size<Length<S>> {
        Size { width: self.length(slot::WIDTH), height: self.length(slot::HEIGHT) }
    }
    /// Returns the style with `width` and `height` set.
    #[inline]
    #[must_use]
    pub fn with_size(mut self, width: Length<S>, height: Length<S>) -> Self {
        self.set_length(slot::WIDTH, width);
        self.set_length(slot::HEIGHT, height);
        self
    }
    /// Returns the style with `min-width` and `min-height` set.
    #[inline]
    #[must_use]
    pub fn with_min_size(mut self, width: Length<S>, height: Length<S>) -> Self {
        self.set_length(slot::MIN_WIDTH, width);
        self.set_length(slot::MIN_HEIGHT, height);
        self
    }
    /// Returns the style with `max-width` and `max-height` set.
    #[inline]
    #[must_use]
    pub fn with_max_size(mut self, width: Length<S>, height: Length<S>) -> Self {
        self.set_length(slot::MAX_WIDTH, width);
        self.set_length(slot::MAX_HEIGHT, height);
        self
    }

    /// The four margins.
    #[inline]
    pub fn margin(&self) -> Edges<Length<S>> {
        self.edges(slot::MARGIN)
    }
    /// Sets the four margins.
    #[inline]
    pub fn set_margin(&mut self, e: Edges<Length<S>>) {
        self.set_edges(slot::MARGIN, e)
    }
    /// Returns the style with the four margins set.
    #[inline]
    #[must_use]
    pub fn with_margin(mut self, e: Edges<Length<S>>) -> Self {
        self.set_edges(slot::MARGIN, e);
        self
    }
    /// The four paddings.
    #[inline]
    pub fn padding(&self) -> Edges<Length<S>> {
        self.edges(slot::PADDING)
    }
    /// Sets the four paddings.
    #[inline]
    pub fn set_padding(&mut self, e: Edges<Length<S>>) {
        self.set_edges(slot::PADDING, e)
    }
    /// Returns the style with the four paddings set.
    #[inline]
    #[must_use]
    pub fn with_padding(mut self, e: Edges<Length<S>>) -> Self {
        self.set_edges(slot::PADDING, e);
        self
    }
    /// The four insets (`left`, `right`, `top`, `bottom`).
    #[inline]
    pub fn inset(&self) -> Edges<Length<S>> {
        self.edges(slot::INSET)
    }
    /// Sets the four insets.
    #[inline]
    pub fn set_inset(&mut self, e: Edges<Length<S>>) {
        self.set_edges(slot::INSET, e)
    }
    /// Returns the style with the four insets set.
    #[inline]
    #[must_use]
    pub fn with_inset(mut self, e: Edges<Length<S>>) -> Self {
        self.set_edges(slot::INSET, e);
        self
    }
    /// The four border widths. Borders are always absolute lengths.
    #[inline]
    pub fn border(&self) -> Edges<S> {
        self.border
    }
    /// Sets the four border widths.
    #[inline]
    pub fn set_border(&mut self, e: Edges<S>) {
        self.border = e;
    }
    /// Returns the style with the four border widths set.
    #[inline]
    #[must_use]
    pub fn with_border(mut self, e: Edges<S>) -> Self {
        self.border = e;
        self
    }
    /// Returns the style with `row-gap` and `column-gap` set.
    #[inline]
    #[must_use]
    pub fn with_gap(mut self, row: Length<S>, column: Length<S>) -> Self {
        self.set_length(slot::ROW_GAP, row);
        self.set_length(slot::COLUMN_GAP, column);
        self
    }

    /// The `flex-grow` factor.
    #[inline]
    pub fn flex_grow(&self) -> S {
        self.flex_grow
    }
    /// Sets `flex-grow`.
    #[inline]
    pub fn set_flex_grow(&mut self, v: S) {
        self.flex_grow = v;
    }
    /// Returns the style with `flex-grow` set.
    #[inline]
    #[must_use]
    pub fn with_flex_grow(mut self, v: S) -> Self {
        self.flex_grow = v;
        self
    }
    /// The `flex-shrink` factor.
    #[inline]
    pub fn flex_shrink(&self) -> S {
        self.flex_shrink
    }
    /// Sets `flex-shrink`.
    #[inline]
    pub fn set_flex_shrink(&mut self, v: S) {
        self.flex_shrink = v;
    }
    /// Returns the style with `flex-shrink` set.
    #[inline]
    #[must_use]
    pub fn with_flex_shrink(mut self, v: S) -> Self {
        self.flex_shrink = v;
        self
    }
    /// Returns the style with the `flex` shorthand expanded: grow, shrink and basis.
    ///
    /// CSS `flex: 1` is `with_flex(1, 1, Length::Percent(0))`; Yoga's `flex: 1` is
    /// `with_flex(1, 1, Length::Auto)`. The expansion belongs to the host's props layer.
    #[inline]
    #[must_use]
    pub fn with_flex(mut self, grow: S, shrink: S, basis: Length<S>) -> Self {
        self.flex_grow = grow;
        self.flex_shrink = shrink;
        self.set_length(slot::BASIS, basis);
        self
    }
    /// The `aspect-ratio` as width divided by height; none is `auto`.
    #[inline]
    pub fn aspect_ratio(&self) -> Opt<S> {
        self.aspect_ratio
    }
    /// Sets `aspect-ratio`.
    #[inline]
    pub fn set_aspect_ratio(&mut self, v: Opt<S>) {
        self.aspect_ratio = v;
    }
    /// Returns the style with `aspect-ratio` set.
    #[inline]
    #[must_use]
    pub fn with_aspect_ratio(mut self, v: Opt<S>) -> Self {
        self.aspect_ratio = v;
        self
    }
    /// The `order` property.
    #[inline]
    pub fn order(&self) -> i32 {
        self.order
    }
    /// Sets `order`.
    #[inline]
    pub fn set_order(&mut self, v: i32) {
        self.order = v;
    }
    /// Returns the style with `order` set.
    #[inline]
    #[must_use]
    pub fn with_order(mut self, v: i32) -> Self {
        self.order = v;
        self
    }
    /// Space reserved on the inline end and block end when the node is a scroll container.
    #[inline]
    pub fn scrollbar_gutter(&self) -> S {
        self.scrollbar_gutter
    }
    /// Sets the scrollbar gutter.
    #[inline]
    pub fn set_scrollbar_gutter(&mut self, v: S) {
        self.scrollbar_gutter = v;
    }
    /// Returns the style with the scrollbar gutter set.
    #[inline]
    #[must_use]
    pub fn with_scrollbar_gutter(mut self, v: S) -> Self {
        self.scrollbar_gutter = v;
        self
    }
}

impl<S: Scalar> fmt::Debug for Style<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Style")
            .field("display", &self.display())
            .field("position", &self.position())
            .field("direction", &self.direction())
            .field("flex_direction", &self.flex_direction())
            .field("flex_wrap", &self.flex_wrap())
            .field("box_sizing", &self.box_sizing())
            .field("overflow", &(self.overflow_x(), self.overflow_y()))
            .field("justify_content", &self.justify_content())
            .field("align_items", &self.align_items())
            .field("align_self", &self.align_self())
            .field("align_content", &self.align_content())
            .field("flex", &(self.flex_grow, self.flex_shrink, self.flex_basis()))
            .field("size", &self.size())
            .field("min_size", &(self.min_width(), self.min_height()))
            .field("max_size", &(self.max_width(), self.max_height()))
            .field("margin", &self.margin())
            .field("padding", &self.padding())
            .field("border", &self.border)
            .field("inset", &self.inset())
            .field("gap", &(self.row_gap(), self.column_gap()))
            .field("aspect_ratio", &self.aspect_ratio)
            .field("order", &self.order)
            .field("scrollbar_gutter", &self.scrollbar_gutter)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_defaults_are_css_initial_values() {
        let s: Style = Style::web();
        assert_eq!(s.display(), Display::Flex);
        assert_eq!(s.position(), Position::Static);
        assert_eq!(s.flex_direction(), FlexDirection::Row);
        assert_eq!(s.box_sizing(), BoxSizing::ContentBox);
        assert_eq!(s.align_items(), AlignItems::Normal);
        assert_eq!(s.align_self(), None);
        assert_eq!(s.flex_grow(), 0.0);
        assert_eq!(s.flex_shrink(), 1.0);
        assert_eq!(s.flex_basis(), Length::Auto);
        assert_eq!(s.min_width(), Length::Auto);
        assert_eq!(s.max_width(), Length::None);
        assert_eq!(s.margin(), Edges::all(Length::Px(0.0)));
        assert_eq!(s.inset(), Edges::all(Length::Auto));
    }

    #[test]
    fn yoga_preset_differs_in_exactly_the_documented_places() {
        let w: Style = Style::web();
        let y: Style = Style::yoga();
        assert_eq!(y.flex_direction(), FlexDirection::Column);
        assert_eq!(y.flex_shrink(), 0.0);
        assert_eq!(y.position(), Position::Relative);
        assert_eq!(y.box_sizing(), BoxSizing::BorderBox);
        assert_eq!(y.align_content(), AlignContent::FlexStart);
        let back = y
            .with_flex_direction(w.flex_direction())
            .with_flex_shrink(w.flex_shrink())
            .with_position(w.position())
            .with_box_sizing(w.box_sizing())
            .with_align_content(w.align_content());
        assert_eq!(back, w);
    }

    #[test]
    fn tags_and_payloads_round_trip() {
        let s: Style = Style::web()
            .with_size(Length::Px(10.0), Length::Percent(0.5))
            .with_align_self(Some(AlignItems::Center))
            .with_margin(Edges { left: Length::Auto, right: Length::Px(1.0), top: Length::Px(2.0), bottom: Length::Percent(0.1) })
            .with_gap(Length::Px(3.0), Length::Px(4.0));
        assert_eq!(s.width(), Length::Px(10.0));
        assert_eq!(s.height(), Length::Percent(0.5));
        assert_eq!(s.align_self(), Some(AlignItems::Center));
        assert_eq!(s.margin().left, Length::Auto);
        assert_eq!(s.margin().bottom, Length::Percent(0.1));
        assert_eq!(s.row_gap(), Length::Px(3.0));
        assert_eq!(s.column_gap(), Length::Px(4.0));
        assert_eq!(s.resolve(slot::HEIGHT, Opt::some(200.0)).get(), Some(100.0));
        assert_eq!(s.resolve(slot::HEIGHT, Opt::NONE).get(), None);
    }

    #[test]
    fn fingerprint_tracks_every_field() {
        let a: Style = Style::web();
        assert_eq!(a.fingerprint(), Style::<f32>::web().fingerprint());
        assert_ne!(a.fingerprint(), a.with_width(Length::Px(1.0)).fingerprint());
        assert_ne!(a.fingerprint(), a.with_width(Length::Percent(0.0)).fingerprint(), "tag change alone must change the key");
        assert_ne!(a.fingerprint(), a.with_order(1).fingerprint());
        assert_ne!(a.fingerprint(), a.with_align_self(Some(AlignItems::Normal)).fingerprint());
        assert_ne!(a.fingerprint(), a.with_border(Edges::all(1.0)).fingerprint());
    }
}
