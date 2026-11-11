//! Keyword properties. Each is a small `#[repr(u8)]` enum packed into [`Style`](super::Style)'s
//! bit word; the enum is the API, the bits are an implementation detail.

/// Defines a keyword enum with a `Default` variant and a lossless round trip through `u8`.
macro_rules! keyword {
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident { $( $(#[$vmeta:meta])* $variant:ident ),+ $(,)? }
        default = $default:ident
    ) => {
        $(#[$meta])*
        #[repr(u8)]
        #[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
        $vis enum $name { $( $(#[$vmeta])* $variant ),+ }

        impl Default for $name {
            #[inline]
            fn default() -> Self { $name::$default }
        }

        impl $name {
            const ALL: &'static [$name] = &[ $( $name::$variant ),+ ];
            /// Number of bits needed to store this keyword.
            #[allow(dead_code)]
            pub(crate) const BITS: u32 = {
                let n = Self::ALL.len() as u32;
                // ceil(log2(n)), at least 1
                let mut bits = 1;
                while (1u32 << bits) < n { bits += 1; }
                bits
            };
            #[inline]
            pub(crate) fn from_u8(v: u8) -> Self {
                Self::ALL.get(v as usize).copied().unwrap_or_default()
            }
        }
    };
}

keyword! {
    /// What kind of box the node generates. There is no `block` or `inline`: this is a
    /// flexbox-only engine, so a box is always a flex container.
    pub Display {
        /// A flex container (the default; the web's `inline`/`block` have no meaning here).
        Flex,
        /// Generates no box and hides the whole subtree. Nothing below is laid out or written.
        None,
        /// Generates no box; the node's children are laid out as items of the nearest ancestor box.
        Contents,
    }
    default = Flex
}

keyword! {
    /// The CSS `position` property.
    #[non_exhaustive]
    pub Position {
        /// Normal flow; insets have no effect. The CSS initial value.
        Static,
        /// Normal flow, then shifted by insets. Yoga's default.
        Relative,
        /// Taken out of flow and positioned against the nearest positioned ancestor.
        Absolute,
    }
    default = Static
}

keyword! {
    /// Inline direction. `Inherit` takes the value from the parent, or from the root constraints.
    pub Direction {
        /// Inherit from the parent.
        Inherit,
        /// Left to right.
        Ltr,
        /// Right to left.
        Rtl,
    }
    default = Inherit
}

keyword! {
    /// The CSS `flex-direction` property.
    pub FlexDirection {
        /// Main axis is horizontal, items start at the inline start.
        Row,
        /// Main axis is horizontal, items start at the inline end.
        RowReverse,
        /// Main axis is vertical, items start at the top.
        Column,
        /// Main axis is vertical, items start at the bottom.
        ColumnReverse,
    }
    default = Row
}

impl FlexDirection {
    /// True for `column` and `column-reverse`.
    #[inline]
    #[must_use]
    pub const fn is_column(self) -> bool {
        matches!(self, FlexDirection::Column | FlexDirection::ColumnReverse)
    }
    /// True for the two `*-reverse` values.
    #[inline]
    #[must_use]
    pub const fn is_reverse(self) -> bool {
        matches!(self, FlexDirection::RowReverse | FlexDirection::ColumnReverse)
    }
}

keyword! {
    /// The CSS `flex-wrap` property.
    #[non_exhaustive]
    pub FlexWrap {
        /// Single line.
        NoWrap,
        /// Multiple lines, stacked in the cross-start direction.
        Wrap,
        /// Multiple lines, stacked in the cross-end direction.
        WrapReverse,
    }
    default = NoWrap
}

keyword! {
    /// The CSS `box-sizing` property.
    pub BoxSizing {
        /// `width` and `height` set the content box. The CSS initial value.
        ContentBox,
        /// `width` and `height` set the border box. Yoga's and Taffy's default.
        BorderBox,
    }
    default = ContentBox
}

keyword! {
    /// The CSS `overflow` property, as a layout input only. `scroll` makes the node a scroll
    /// container, which zeroes its automatic minimum size and reserves the scrollbar gutter.
    #[non_exhaustive]
    pub Overflow {
        /// Content may overflow; the automatic minimum size applies.
        Visible,
        /// Content is clipped; the automatic minimum size applies.
        Clip,
        /// Content is clipped; the automatic minimum size is zero.
        Hidden,
        /// A scroll container.
        Scroll,
    }
    default = Visible
}

impl Overflow {
    /// True when the automatic minimum size of a flex item is zero rather than content-based.
    #[inline]
    #[must_use]
    pub const fn is_scroll_container(self) -> bool {
        matches!(self, Overflow::Hidden | Overflow::Scroll)
    }
}

keyword! {
    /// The CSS `justify-content` property (main-axis distribution).
    #[non_exhaustive]
    pub JustifyContent {
        /// Behaves as `flex-start`. The CSS initial value.
        Normal,
        /// Pack toward the main start.
        FlexStart,
        /// Pack toward the main end.
        FlexEnd,
        /// Center.
        Center,
        /// First and last items at the edges, equal space between.
        SpaceBetween,
        /// Equal space around every item.
        SpaceAround,
        /// Equal space between items and at the edges.
        SpaceEvenly,
        /// Pack toward the writing-mode start (independent of `flex-direction`).
        Start,
        /// Pack toward the writing-mode end.
        End,
        /// Behaves as `flex-start` in flexbox.
        Stretch,
    }
    default = Normal
}

keyword! {
    /// The CSS `align-items` property, and the value of `align-self` when it is not `auto`.
    #[non_exhaustive]
    pub AlignItems {
        /// Behaves as `stretch`. The CSS initial value.
        Normal,
        /// Stretch auto-sized items to the line.
        Stretch,
        /// Align to the cross start.
        FlexStart,
        /// Align to the cross end.
        FlexEnd,
        /// Center on the cross axis.
        Center,
        /// Align first baselines.
        Baseline,
        /// Align to the writing-mode start.
        Start,
        /// Align to the writing-mode end.
        End,
        /// Align to the item's own start edge.
        SelfStart,
        /// Align to the item's own end edge.
        SelfEnd,
    }
    default = Normal
}

impl AlignItems {
    /// True for `normal` and `stretch`.
    #[inline]
    #[must_use]
    pub const fn is_stretch(self) -> bool {
        matches!(self, AlignItems::Normal | AlignItems::Stretch)
    }
}

keyword! {
    /// The CSS `align-content` property (cross-axis distribution of lines).
    #[non_exhaustive]
    pub AlignContent {
        /// Behaves as `stretch`. The CSS initial value.
        Normal,
        /// Lines packed to the cross start. Yoga's default.
        FlexStart,
        /// Lines packed to the cross end.
        FlexEnd,
        /// Lines centered.
        Center,
        /// Equal space between lines.
        SpaceBetween,
        /// Equal space around lines.
        SpaceAround,
        /// Equal space between lines and at the edges.
        SpaceEvenly,
        /// Lines stretched to fill the container.
        Stretch,
        /// Lines packed to the writing-mode start.
        Start,
        /// Lines packed to the writing-mode end.
        End,
    }
    default = Normal
}

keyword! {
    /// Unit tags for lengths; four bits each inside `Style`.
    pub(crate) Unit {
        Auto,
        Px,
        Percent,
        MinContent,
        MaxContent,
        FitContent,
        Stretch,
        Content,
        None,
    }
    default = Auto
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_widths_fit_the_packing_plan() {
        assert_eq!(Display::BITS, 2);
        assert_eq!(Position::BITS, 2);
        assert_eq!(Direction::BITS, 2);
        assert_eq!(FlexDirection::BITS, 2);
        assert_eq!(FlexWrap::BITS, 2);
        assert_eq!(BoxSizing::BITS, 1);
        assert_eq!(Overflow::BITS, 2);
        const _: () = assert!(JustifyContent::BITS <= 5);
        const _: () = assert!(AlignItems::BITS <= 5);
        const _: () = assert!(AlignContent::BITS <= 5);
        assert_eq!(Unit::BITS, 4);
    }

    #[test]
    fn round_trips() {
        for (i, v) in AlignItems::ALL.iter().enumerate() {
            assert_eq!(AlignItems::from_u8(i as u8), *v);
        }
        assert_eq!(AlignItems::from_u8(200), AlignItems::Normal);
    }
}
