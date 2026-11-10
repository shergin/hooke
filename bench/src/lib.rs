//! `hooke-bench`: benchmarks comparing hooke with Taffy, Yoga and Cephal on workloads whose
//! output is first checked to be equal across engines.
//!
//! - [`scene`]: the neutral tree description every adapter builds from.
//! - [`engine`]: the adapter contract.
//! - [`measure`]: the shared text model.
//! - [`engines`]: one adapter per engine.
//! - [`workloads`]: seeded, deterministic scene generators at three sizes.
//! - [`gate`]: the equal-output check that runs before anything is timed.
//! - [`alloc`]: the counting global allocator for the memory workload.
//!
//! See `bench/README.md` for how to run the suite and read the report.

pub mod alloc;
pub mod engine;
pub mod engines;
pub mod gate;
pub mod measure;
pub mod scene;
pub mod workloads;
