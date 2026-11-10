//! One adapter per engine. Each maps the scene's style subset onto the engine's own types,
//! routes the engine's measure callback through [`crate::measure::measure_text`], and reads
//! frames back in scene node order.

pub mod cephal;
pub mod hooke;
pub mod taffy;
#[cfg(feature = "yoga")]
pub mod yoga;
