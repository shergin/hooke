//! Small geometry types: axes, sizes, points, rectangles and per-edge values.
//!
//! These are plain `#[repr(C)]` structs with public fields. They are the same size as the
//! arrays they replace and exist to make signatures readable: `Size<Opt<f32>>` says more than
//! `[Opt<f32>; 2]`.

use crate::scalar::Scalar;

/// A layout axis.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Axis {
    /// The x axis: widths, left and right.
    Horizontal,
    /// The y axis: heights, top and bottom.
    Vertical,
}

impl Axis {
    /// The other axis.
    #[inline]
    #[must_use]
    pub const fn cross(self) -> Axis {
        match self {
            Axis::Horizontal => Axis::Vertical,
            Axis::Vertical => Axis::Horizontal,
        }
    }
    /// `0` for horizontal, `1` for vertical; for indexing two-element arrays.
    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Axis::Horizontal => 0,
            Axis::Vertical => 1,
        }
    }
}

/// A width and a height.
#[repr(C)]
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Size<T> {
    /// Extent along the horizontal axis.
    pub width: T,
    /// Extent along the vertical axis.
    pub height: T,
}

impl<T> Size<T> {
    /// Builds a size.
    #[inline]
    pub const fn new(width: T, height: T) -> Self {
        Size { width, height }
    }
    /// The extent along `axis`.
    #[inline]
    pub fn get(&self, axis: Axis) -> T
    where
        T: Copy,
    {
        match axis {
            Axis::Horizontal => self.width,
            Axis::Vertical => self.height,
        }
    }
    /// Sets the extent along `axis`.
    #[inline]
    pub fn set(&mut self, axis: Axis, value: T) {
        match axis {
            Axis::Horizontal => self.width = value,
            Axis::Vertical => self.height = value,
        }
    }
    /// Applies `f` to both components.
    #[inline]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Size<U> {
        Size {
            width: f(self.width),
            height: f(self.height),
        }
    }
    /// Builds a size from a per-axis function.
    #[inline]
    pub fn from_fn(mut f: impl FnMut(Axis) -> T) -> Self {
        Size {
            width: f(Axis::Horizontal),
            height: f(Axis::Vertical),
        }
    }
}

impl<S: Scalar> Size<S> {
    /// A zero size.
    pub const ZERO: Self = Size {
        width: S::ZERO,
        height: S::ZERO,
    };
}

impl<T> From<[T; 2]> for Size<T> {
    #[inline]
    fn from([width, height]: [T; 2]) -> Self {
        Size { width, height }
    }
}

/// An x and a y.
#[repr(C)]
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Point<T> {
    /// Horizontal coordinate.
    pub x: T,
    /// Vertical coordinate.
    pub y: T,
}

impl<T> Point<T> {
    /// Builds a point.
    #[inline]
    pub const fn new(x: T, y: T) -> Self {
        Point { x, y }
    }
    /// The coordinate along `axis`.
    #[inline]
    pub fn get(&self, axis: Axis) -> T
    where
        T: Copy,
    {
        match axis {
            Axis::Horizontal => self.x,
            Axis::Vertical => self.y,
        }
    }
    /// Sets the coordinate along `axis`.
    #[inline]
    pub fn set(&mut self, axis: Axis, value: T) {
        match axis {
            Axis::Horizontal => self.x = value,
            Axis::Vertical => self.y = value,
        }
    }
}

impl<S: Scalar> Point<S> {
    /// The origin.
    pub const ZERO: Self = Point { x: S::ZERO, y: S::ZERO };
    /// Component-wise sum.
    #[inline]
    #[must_use]
    pub fn offset(self, by: Point<S>) -> Self {
        Point {
            x: self.x + by.x,
            y: self.y + by.y,
        }
    }
}

/// An axis-aligned rectangle: origin plus size.
#[repr(C)]
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Rect<T> {
    /// Left edge.
    pub x: T,
    /// Top edge.
    pub y: T,
    /// Width.
    pub width: T,
    /// Height.
    pub height: T,
}

impl<T: Copy> Rect<T> {
    /// Builds a rectangle from an origin and a size.
    #[inline]
    pub fn new(origin: Point<T>, size: Size<T>) -> Self {
        Rect {
            x: origin.x,
            y: origin.y,
            width: size.width,
            height: size.height,
        }
    }
    /// The origin.
    #[inline]
    pub fn origin(&self) -> Point<T> {
        Point { x: self.x, y: self.y }
    }
    /// The size.
    #[inline]
    pub fn size(&self) -> Size<T> {
        Size {
            width: self.width,
            height: self.height,
        }
    }
}

/// One value per box edge, in CSS order of meaning rather than of writing.
#[repr(C)]
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Edges<T> {
    /// Left edge.
    pub left: T,
    /// Right edge.
    pub right: T,
    /// Top edge.
    pub top: T,
    /// Bottom edge.
    pub bottom: T,
}

impl<T: Copy> Edges<T> {
    /// The same value on all four edges.
    #[inline]
    pub const fn all(v: T) -> Self {
        Edges {
            left: v,
            right: v,
            top: v,
            bottom: v,
        }
    }
    /// `horizontal` on left and right, `vertical` on top and bottom.
    #[inline]
    pub const fn symmetric(horizontal: T, vertical: T) -> Self {
        Edges {
            left: horizontal,
            right: horizontal,
            top: vertical,
            bottom: vertical,
        }
    }
    /// The two edges that bound `axis`, start first.
    #[inline]
    pub fn along(&self, axis: Axis) -> (T, T) {
        match axis {
            Axis::Horizontal => (self.left, self.right),
            Axis::Vertical => (self.top, self.bottom),
        }
    }
    /// Applies `f` to every edge.
    #[inline]
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Edges<U> {
        Edges {
            left: f(self.left),
            right: f(self.right),
            top: f(self.top),
            bottom: f(self.bottom),
        }
    }
}

impl<S: Scalar> Edges<S> {
    /// Zero on every edge.
    pub const ZERO: Self = Edges {
        left: S::ZERO,
        right: S::ZERO,
        top: S::ZERO,
        bottom: S::ZERO,
    };
    /// `left + right` for the horizontal axis, `top + bottom` for the vertical.
    #[inline]
    pub fn sum(&self, axis: Axis) -> S {
        let (a, b) = self.along(axis);
        a + b
    }
    /// Both sums as a size.
    #[inline]
    pub fn sums(&self) -> Size<S> {
        Size {
            width: self.left + self.right,
            height: self.top + self.bottom,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_arrays() {
        assert_eq!(core::mem::size_of::<Size<f32>>(), 8);
        assert_eq!(core::mem::size_of::<Point<f32>>(), 8);
        assert_eq!(core::mem::size_of::<Rect<f32>>(), 16);
        assert_eq!(core::mem::size_of::<Edges<f32>>(), 16);
    }
}
