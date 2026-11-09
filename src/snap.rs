//! Pixel snapping at mount time.
//!
//! The tree holds unrounded values only. Snapping is a pure function the host applies while it
//! walks the tree to mount, which it must do anyway to turn parent-relative offsets into frames.
//! Nothing is written back, so rounding can never race, never double-round, and a display-scale
//! change needs no relayout.

use crate::geometry::{Point, Rect, Size};
use crate::scalar::Scalar;

/// Snaps one node's frame to the device pixel grid.
///
/// `parent_origin` is the parent's snapped absolute origin, `offset` the child's offset from its
/// parent's [`Slot`](crate::Slot), `size` the child's border-box size, and `scale` the device
/// pixels per layout unit. Edges are rounded in absolute space and the size is the difference of
/// rounded edges, so adjacent siblings tile without gaps or overlaps, as Yoga's pixel-grid pass
/// and Taffy's rounding do.
///
/// The returned rectangle is absolute. Hosts mounting parent-relative frames subtract the
/// parent's snapped origin.
#[must_use]
pub fn snap<S: Scalar>(parent_origin: Point<S>, offset: Point<S>, size: Size<S>, scale: S) -> Rect<S> {
    let round = |v: S| (v * scale).round() / scale;
    let x0 = parent_origin.x + offset.x;
    let y0 = parent_origin.y + offset.y;
    let (sx0, sy0) = (round(x0), round(y0));
    let (sx1, sy1) = (round(x0 + size.width), round(y0 + size.height));
    Rect { x: sx0, y: sy0, width: sx1 - sx0, height: sy1 - sy0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siblings_tile_without_gaps() {
        let scale = 2.0f32;
        let parent = Point::new(0.3, 0.0);
        let a = snap(parent, Point::new(0.0, 0.0), Size::new(10.26, 5.0), scale);
        let b = snap(parent, Point::new(10.26, 0.0), Size::new(7.51, 5.0), scale);
        assert_eq!(a.x + a.width, b.x, "end of a is start of b");
        assert_eq!((b.x * scale).round(), b.x * scale, "on the pixel grid");
    }
}
