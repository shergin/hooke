//! The scalar abstraction: `f32` by default, with room for `f64` and fixed point.
//!
//! Every public type takes `S: Scalar` with `f32` as its default, so most hosts never write
//! the parameter. The trait is deliberately small: flexbox needs arithmetic, comparison,
//! rounding, one ratio operation and a way to say "undefined". It does not need NaN, infinity
//! or transcendental functions, which is what makes a fixed-point implementation possible.

use core::fmt;
use core::ops::{Add, Div, Mul, Neg, Sub};

/// A number the engine can lay out with.
///
/// Implemented for `f32` and, behind the `f64` feature, for `f64`. Implementors must be plain
/// `Copy` values: the engine stores them inline in nodes and compares them by [`Scalar::key_bits`]
/// in cache keys.
pub trait Scalar:
    Copy
    + PartialOrd
    + Default
    + fmt::Debug
    + Send
    + Sync
    + 'static
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
{
    /// Zero.
    const ZERO: Self;
    /// One.
    const ONE: Self;
    /// A value larger than any layout size; stands in for "unbounded" available space.
    const MAX: Self;
    /// Comparison tolerance for "equal enough" layout results. Zero for exact types.
    const EPSILON: Self;
    /// The sentinel [`Opt`] uses for "undefined": NaN for floats, a reserved value for fixed point.
    const NONE: Self;

    /// True for the [`Scalar::NONE`] sentinel.
    fn is_none(self) -> bool;
    /// Conversion from style literals and factors.
    fn from_f32(v: f32) -> Self;
    /// Conversion for host interop and diagnostics.
    fn to_f32(self) -> f32;
    /// `self * num / den` with a wide intermediate where the type needs one.
    fn mul_div(self, num: Self, den: Self) -> Self;
    /// Largest integer not greater than `self`.
    fn floor(self) -> Self;
    /// Smallest integer not less than `self`.
    fn ceil(self) -> Self;
    /// Nearest integer, halves toward positive infinity (`-0.5` rounds to `0`), as Yoga's pixel
    /// grid, Taffy and JavaScript's `Math.round` do. One implementation for every target.
    fn round(self) -> Self;
    /// Absolute value.
    fn abs(self) -> Self;
    /// Bits used in cache keys. Keys compare, they never compute, so this may be lossy for
    /// types wider than 32 bits.
    fn key_bits(self) -> u32;
    /// The value behind [`Scalar::key_bits`], when the encoding is lossless. Types that lose
    /// information return `None`, which disables the relaxed cache-hit rules for them.
    fn from_key_bits(bits: u32) -> Option<Self>;

    /// The larger of two values.
    #[inline]
    fn max(self, other: Self) -> Self {
        if other > self {
            other
        } else {
            self
        }
    }
    /// The smaller of two values.
    #[inline]
    fn min(self, other: Self) -> Self {
        if other < self {
            other
        } else {
            self
        }
    }
    /// `self` clamped to `[lo, hi]`, where either bound may be undefined.
    #[inline]
    fn clamp_opt(self, lo: Opt<Self>, hi: Opt<Self>) -> Self {
        let v = match hi.get() {
            Some(h) => self.min(h),
            None => self,
        };
        match lo.get() {
            Some(l) => v.max(l),
            None => v,
        }
    }
    /// True when the two values differ by at most [`Scalar::EPSILON`].
    #[inline]
    fn approx_eq(self, other: Self) -> bool {
        (self - other).abs() <= Self::EPSILON
    }
}

/// Truncation toward zero for `f32`, exact for every finite input (values at or beyond 2^23
/// are already integers). Written without `std` so every target rounds identically.
#[inline]
#[allow(clippy::neg_cmp_op_on_partial_ord)] // the negated comparison deliberately routes NaN to the early return
fn trunc_f32(x: f32) -> f32 {
    if !(x.abs_bits() < 8_388_608.0) {
        return x; // NaN, infinities and |x| >= 2^23 are their own truncation
    }
    (x as i32) as f32
}

/// Private helper so `abs` does not depend on `std`.
trait AbsBits {
    fn abs_bits(self) -> Self;
}
impl AbsBits for f32 {
    #[inline]
    fn abs_bits(self) -> f32 {
        f32::from_bits(self.to_bits() & 0x7FFF_FFFF)
    }
}
#[cfg(feature = "f64")]
impl AbsBits for f64 {
    #[inline]
    fn abs_bits(self) -> f64 {
        f64::from_bits(self.to_bits() & 0x7FFF_FFFF_FFFF_FFFF)
    }
}

impl Scalar for f32 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    const MAX: Self = f32::INFINITY;
    const EPSILON: Self = 1e-4;
    const NONE: Self = f32::NAN;

    #[inline]
    fn is_none(self) -> bool {
        self.is_nan()
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        v
    }
    #[inline]
    fn to_f32(self) -> f32 {
        self
    }
    #[inline]
    fn mul_div(self, num: Self, den: Self) -> Self {
        self * num / den
    }
    #[inline]
    fn floor(self) -> Self {
        let t = trunc_f32(self);
        if self < t {
            t - 1.0
        } else {
            t
        }
    }
    #[inline]
    fn ceil(self) -> Self {
        let t = trunc_f32(self);
        if self > t {
            t + 1.0
        } else {
            t
        }
    }
    #[inline]
    fn round(self) -> Self {
        let f = Scalar::floor(self);
        if self - f >= 0.5 { f + 1.0 } else { f }
    }
    #[inline]
    fn abs(self) -> Self {
        self.abs_bits()
    }
    #[inline]
    fn key_bits(self) -> u32 {
        self.to_bits()
    }
    #[inline]
    fn from_key_bits(bits: u32) -> Option<Self> {
        Some(f32::from_bits(bits))
    }
}

#[cfg(feature = "f64")]
#[inline]
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn trunc_f64(x: f64) -> f64 {
    if !(x.abs_bits() < 4_503_599_627_370_496.0) {
        return x;
    }
    (x as i64) as f64
}

#[cfg(feature = "f64")]
impl Scalar for f64 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    const MAX: Self = f64::INFINITY;
    const EPSILON: Self = 1e-6;
    const NONE: Self = f64::NAN;

    #[inline]
    fn is_none(self) -> bool {
        self.is_nan()
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        v as f64
    }
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32
    }
    #[inline]
    fn mul_div(self, num: Self, den: Self) -> Self {
        self * num / den
    }
    #[inline]
    fn floor(self) -> Self {
        let t = trunc_f64(self);
        if self < t {
            t - 1.0
        } else {
            t
        }
    }
    #[inline]
    fn ceil(self) -> Self {
        let t = trunc_f64(self);
        if self > t {
            t + 1.0
        } else {
            t
        }
    }
    #[inline]
    fn round(self) -> Self {
        let f = Scalar::floor(self);
        if self - f >= 0.5 { f + 1.0 } else { f }
    }
    #[inline]
    fn abs(self) -> Self {
        self.abs_bits()
    }
    #[inline]
    fn key_bits(self) -> u32 {
        (self as f32).to_bits()
    }
    #[inline]
    fn from_key_bits(_: u32) -> Option<Self> {
        None
    }
}

/// An optional scalar in the space of one.
///
/// `Option<f32>` is 8 bytes because `f32` has no niche; `Opt<f32>` is 4, using the type's
/// [`Scalar::NONE`] sentinel. It converts to and from `Option` freely and compares `NONE`
/// equal to `NONE`, which `Option<f32>` with NaN would not.
#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct Opt<S: Scalar>(S);

impl<S: Scalar> Opt<S> {
    /// The undefined value.
    pub const NONE: Self = Opt(S::NONE);

    /// Wraps a defined value.
    #[inline]
    pub fn some(v: S) -> Self {
        Opt(v)
    }
    /// The value, if defined.
    #[inline]
    pub fn get(self) -> Option<S> {
        if self.0.is_none() {
            None
        } else {
            Some(self.0)
        }
    }
    /// True when defined.
    #[inline]
    pub fn is_some(self) -> bool {
        !self.0.is_none()
    }
    /// True when undefined.
    #[inline]
    pub fn is_none(self) -> bool {
        self.0.is_none()
    }
    /// The value, or `default` when undefined.
    #[inline]
    pub fn unwrap_or(self, default: S) -> S {
        if self.0.is_none() {
            default
        } else {
            self.0
        }
    }
    /// Applies `f` to a defined value.
    #[inline]
    pub fn map(self, f: impl FnOnce(S) -> S) -> Self {
        if self.0.is_none() {
            self
        } else {
            Opt(f(self.0))
        }
    }
    /// `self` if defined, otherwise `other`.
    #[inline]
    pub fn or(self, other: Self) -> Self {
        if self.0.is_none() {
            other
        } else {
            self
        }
    }
    /// Bits for cache keys; `NONE` has one canonical pattern.
    #[inline]
    pub fn key_bits(self) -> u32 {
        if self.0.is_none() {
            S::NONE.key_bits()
        } else {
            self.0.key_bits()
        }
    }
}

impl<S: Scalar> Default for Opt<S> {
    #[inline]
    fn default() -> Self {
        Self::NONE
    }
}
impl<S: Scalar> PartialEq for Opt<S> {
    #[inline]
    fn eq(&self, o: &Self) -> bool {
        self.key_bits() == o.key_bits()
    }
}
impl<S: Scalar> From<Option<S>> for Opt<S> {
    #[inline]
    fn from(o: Option<S>) -> Self {
        match o {
            Some(v) => Opt(v),
            None => Self::NONE,
        }
    }
}
impl<S: Scalar> From<Opt<S>> for Option<S> {
    #[inline]
    fn from(o: Opt<S>) -> Self {
        o.get()
    }
}
impl<S: Scalar> From<S> for Opt<S> {
    #[inline]
    fn from(v: S) -> Self {
        Opt(v)
    }
}
impl<S: Scalar> fmt::Debug for Opt<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.get() {
            Some(v) => write!(f, "Some({:?})", v),
            None => f.write_str("None"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_half_up_and_std_free() {
        assert_eq!(Scalar::round(2.5f32), 3.0);
        assert_eq!(Scalar::round(-2.5f32), -2.0);
        assert_eq!(Scalar::round(-0.5f32), 0.0);
        assert_eq!(Scalar::round(2.4f32), 2.0);
        assert_eq!(Scalar::round(-0.6f32), -1.0);
        assert_eq!(Scalar::round(-0.4f32), 0.0);
        assert_eq!((-0.5f32).floor(), -1.0);
        assert_eq!((-0.5f32).ceil(), 0.0);
        assert_eq!(1e9f32.floor(), 1e9);
        assert!(f32::NAN.round().is_nan());
    }

    #[test]
    fn opt_is_four_bytes_and_none_equals_none() {
        assert_eq!(core::mem::size_of::<Opt<f32>>(), 4);
        assert_eq!(Opt::<f32>::NONE, Opt::NONE);
        assert_ne!(Opt::some(1.0f32), Opt::NONE);
        assert_eq!(Opt::<f32>::from(Some(2.0)).get(), Some(2.0));
        assert_eq!(Option::<f32>::from(Opt::<f32>::NONE), None);
        assert_eq!(Opt::some(1.0f32).unwrap_or(5.0), 1.0);
        assert_eq!(Opt::<f32>::NONE.unwrap_or(5.0), 5.0);
    }
}
