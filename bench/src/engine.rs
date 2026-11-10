//! The contract every engine adapter implements.
//!
//! An adapter owns one engine's tree for one [`Scene`] and exposes the operations the workloads
//! time. Each operation does whatever that engine needs natively (Taffy marks dirty, hooke
//! path-copies a fork, Cephal patches its retained tree); the suite times the call, not the
//! mechanism. Frames come back absolute and unrounded in scene node order so the equal-output
//! gate can compare engines node by node.

use crate::scene::{Frame, NodeId, Scene, Viewport};

pub trait Engine {
    /// Short name for tables: `hooke`, `taffy`, `yoga`, `cephal`.
    const NAME: &'static str;

    /// Builds the engine's tree for `scene`. Not timed, except by the memory workload.
    fn build(scene: &Scene) -> Self
    where
        Self: Sized;

    /// Lays the tree out under the current viewport. The first call is a cold layout; later
    /// calls are whatever the engine does with its caches, which is what the incremental and
    /// resize workloads measure.
    fn layout(&mut self);

    /// Every node's frame, in scene node order (`out` is cleared first). A `display: none` node
    /// and its descendants report zeroed frames.
    fn frames(&self, out: &mut Vec<Frame>);

    /// Changes the text of leaf `node` and marks whatever the engine needs for the next
    /// [`layout`](Self::layout) to see it.
    fn set_text(&mut self, node: NodeId, text: &str);

    /// Changes the root's available space for the next [`layout`](Self::layout).
    fn resize(&mut self, viewport: Viewport);

    /// Measure callbacks made since the last [`reset_measure_calls`](Self::reset_measure_calls).
    fn measure_calls(&self) -> u64;
    fn reset_measure_calls(&mut self);
}
