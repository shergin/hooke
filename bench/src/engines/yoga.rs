//! Adapter for yoga. Not implemented yet.

use crate::engine::Engine;
use crate::scene::{Frame, NodeId, Scene, Viewport};

pub struct YogaEngine;

impl Engine for YogaEngine {
    const NAME: &'static str = "yoga";
    fn build(_scene: &Scene) -> Self {
        todo!("yoga adapter")
    }
    fn layout(&mut self) {
        todo!()
    }
    fn frames(&self, _out: &mut Vec<Frame>) {
        todo!()
    }
    fn set_text(&mut self, _node: NodeId, _text: &str) {
        todo!()
    }
    fn resize(&mut self, _viewport: Viewport) {
        todo!()
    }
    fn measure_calls(&self) -> u64 {
        0
    }
    fn reset_measure_calls(&mut self) {}
}
