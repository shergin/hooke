//! The equal-output gate, run before anything is timed.
//!
//! Every engine builds the scene and lays it out cold; its frames are compared with hooke's
//! node by node, absolute tolerance [`TOLERANCE`] on x, y, width and height, unrounded. The
//! comparison is repeated after the workload's text edit and after its resize, each applied to
//! the same engine instance after the cold pass, because engines can diverge in their
//! incremental paths too. An engine that panics anywhere (an unimplemented adapter, a bug) did
//! not run; its panic message is the reason.
//!
//! The first cold layout is timed. An engine whose cold layout takes longer than
//! [`slow_secs`] (`HOOKE_BENCH_SLOW_SECS`, default 2 s; hooke lays out 100k nodes in about
//! 0.1 s) is [`Verdict::TooSlow`]: its edit and resize phases are skipped, it is excluded
//! from every timing family for that workload size, and the gate's one layout is its only
//! measurement.

use crate::engine::Engine;
use crate::engines::cephal::CephalEngine;
use crate::engines::hooke::HookeEngine;
use crate::engines::taffy::TaffyEngine;
use crate::scene::{Frame, NodeId, Scene, Viewport};
use crate::workloads::{Edit, Workload};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

/// The cold-layout time above which an engine is not sampled: `HOOKE_BENCH_SLOW_SECS`,
/// default 2.0 seconds.
pub fn slow_secs() -> f64 {
    std::env::var("HOOKE_BENCH_SLOW_SECS").ok().and_then(|v| v.parse().ok()).filter(|v: &f64| *v > 0.0).unwrap_or(2.0)
}

/// Absolute tolerance in layout units.
pub const TOLERANCE: f32 = 0.01;

/// Seconds for tables: one decimal from 1 s, three below.
pub fn secs(v: f64) -> String {
    if v >= 1.0 { format!("{v:.1}") } else { format!("{v:.3}") }
}

/// The outcome of one comparison.
#[derive(Clone, PartialEq, Debug)]
pub enum Verdict {
    Agrees,
    /// The first node, in pre-order, whose frame differs.
    Differs { node: NodeId, hooke: Frame, other: Frame },
    DidNotRun(String),
    /// The gate's cold layout took `seconds`, more than [`slow_secs`]; nothing else is run.
    TooSlow { seconds: f64 },
}

impl Verdict {
    pub fn agrees(&self) -> bool {
        matches!(self, Verdict::Agrees)
    }
    /// A short cell for tables.
    pub fn cell(&self) -> String {
        match self {
            Verdict::Agrees => "agrees".into(),
            Verdict::Differs { node, hooke, other } => format!(
                "differs at #{node}: hooke ({}, {}, {}x{}) vs ({}, {}, {}x{})",
                hooke.x, hooke.y, hooke.width, hooke.height, other.x, other.y, other.width, other.height
            ),
            Verdict::DidNotRun(r) => format!("did not run: {r}"),
            Verdict::TooSlow { seconds } => format!("too slow: {} s per layout, not sampled", secs(*seconds)),
        }
    }
}

/// One engine's verdicts on one workload.
#[derive(Clone, PartialEq, Debug)]
pub struct Row {
    pub engine: &'static str,
    pub cold: Verdict,
    /// `None` when the workload has no text to edit.
    pub edit: Option<Verdict>,
    pub resize: Verdict,
}

impl Row {
    /// May this engine be timed on the workload's cold-layout family?
    pub fn cold_ok(&self) -> bool {
        self.cold.agrees()
    }
    /// May this engine be timed on the edit family?
    pub fn edit_ok(&self) -> bool {
        self.cold.agrees() && self.edit.as_ref().is_some_and(Verdict::agrees)
    }
    /// May this engine be timed on the resize family?
    pub fn resize_ok(&self) -> bool {
        self.cold.agrees() && self.resize.agrees()
    }
    /// Why the engine is excluded from `family`, if it is.
    pub fn exclusion(&self, family: &str) -> Option<String> {
        let first_bad = |vs: &[Option<&Verdict>]| vs.iter().flatten().find(|v| !v.agrees()).map(|v| v.cell());
        match family {
            "edit" | "candidates" => first_bad(&[Some(&self.cold), self.edit.as_ref()])
                .or_else(|| self.edit.is_none().then(|| "no text leaf to edit".to_string())),
            "resize" => first_bad(&[Some(&self.cold), Some(&self.resize)]),
            _ => first_bad(&[Some(&self.cold)]),
        }
    }
}

/// Frames after cold layout, after the edit and after the resize.
#[derive(Clone, Debug)]
pub struct Run {
    pub cold: Vec<Frame>,
    pub edit: Option<Vec<Frame>>,
    pub resize: Vec<Frame>,
    /// Seconds of the first, cold layout.
    pub cold_secs: f64,
    /// The cold layout exceeded [`slow_secs`]; `edit` is `None` and `resize` empty.
    pub too_slow: bool,
}

/// Builds `E`, lays it out cold, applies the edit, then the resize, recording frames after
/// each. Panics propagate.
pub fn run<E: Engine>(scene: &Scene, edit: Option<&Edit>, resized: Viewport) -> Run {
    let mut e = E::build(scene);
    let start = Instant::now();
    e.layout();
    let cold_secs = start.elapsed().as_secs_f64();
    let mut cold = Vec::new();
    e.frames(&mut cold);
    if cold_secs > slow_secs() {
        return Run { cold, edit: None, resize: Vec::new(), cold_secs, too_slow: true };
    }
    let edit = edit.map(|ed| {
        e.set_text(ed.node, &ed.after);
        e.layout();
        let mut f = Vec::new();
        e.frames(&mut f);
        f
    });
    e.resize(resized);
    e.layout();
    let mut resize = Vec::new();
    e.frames(&mut resize);
    Run { cold, edit, resize, cold_secs, too_slow: false }
}

/// [`run`] with panics caught and turned into a reason.
pub fn try_run<E: Engine>(scene: &Scene, edit: Option<&Edit>, resized: Viewport) -> Result<Run, String> {
    catch_unwind(AssertUnwindSafe(|| run::<E>(scene, edit, resized))).map_err(|p| {
        p.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panicked".into())
    })
}

/// Compares two frame lists node by node.
pub fn compare(hooke: &[Frame], other: &[Frame]) -> Verdict {
    if hooke.len() != other.len() {
        return Verdict::DidNotRun(format!("{} frames, hooke has {}", other.len(), hooke.len()));
    }
    let close = |a: f32, b: f32| (a - b).abs() <= TOLERANCE;
    for (node, (h, o)) in hooke.iter().zip(other).enumerate() {
        if !(close(h.x, o.x) && close(h.y, o.y) && close(h.width, o.width) && close(h.height, o.height)) {
            return Verdict::Differs { node, hooke: *h, other: *o };
        }
    }
    Verdict::Agrees
}

fn row(engine: &'static str, reference: &Result<Run, String>, other: Result<Run, String>) -> Row {
    match (reference, other) {
        (Err(r), _) => {
            let v = Verdict::DidNotRun(format!("hooke did not run: {r}"));
            Row { engine, cold: v.clone(), edit: Some(v.clone()), resize: v }
        }
        (Ok(_), Err(r)) => {
            let v = Verdict::DidNotRun(r);
            Row { engine, cold: v.clone(), edit: Some(v.clone()), resize: v }
        }
        (Ok(h), _) if h.too_slow && engine != HookeEngine::NAME => {
            let v = Verdict::DidNotRun(format!("hooke too slow ({} s per layout)", secs(h.cold_secs)));
            Row { engine, cold: v.clone(), edit: Some(v.clone()), resize: v }
        }
        (Ok(_), Ok(o)) if o.too_slow => {
            let v = Verdict::TooSlow { seconds: o.cold_secs };
            Row { engine, cold: v.clone(), edit: Some(v.clone()), resize: v }
        }
        (Ok(h), Ok(o)) => Row {
            engine,
            cold: compare(&h.cold, &o.cold),
            edit: match (&h.edit, &o.edit) {
                (Some(a), Some(b)) => Some(compare(a, b)),
                _ => None,
            },
            resize: compare(&h.resize, &o.resize),
        },
    }
}

/// The gate for one engine against an already computed hooke run.
pub fn check<E: Engine>(w: &Workload, hooke: &Result<Run, String>) -> Row {
    row(E::NAME, hooke, try_run::<E>(&w.scene, w.edit.as_ref(), w.resized))
}

/// The gate for every engine compiled in, hooke first (which agrees with itself by
/// definition when it runs). Runs on a thread with a large stack ([`with_big_stack`]).
pub fn gate(w: &Workload) -> Vec<Row> {
    with_big_stack(|| gate_here(w))
}

fn gate_here(w: &Workload) -> Vec<Row> {
    let hooke = try_run::<HookeEngine>(&w.scene, w.edit.as_ref(), w.resized);
    let mut rows = vec![row(HookeEngine::NAME, &hooke, hooke.clone())];
    rows.push(check::<TaffyEngine>(w, &hooke));
    #[cfg(feature = "yoga")]
    rows.push(check::<crate::engines::yoga::YogaEngine>(w, &hooke));
    rows.push(check::<CephalEngine>(w, &hooke));
    rows
}

/// Runs `f` on a thread with a large stack: the deep workloads recurse once per level in
/// every engine.
pub fn with_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(1 << 30)
            .spawn_scoped(s, f)
            .expect("spawn")
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Align, Avail, Dim, Edges, FlexDirection, Justify, Lp, Node, Style};
    use crate::workloads::{self, Workload};

    fn scene(nodes: Vec<Node>, w: f32, h: f32) -> Scene {
        Scene { nodes, viewport: Viewport { width: Avail::Definite(w), height: Avail::Definite(h) } }
    }
    fn container(style: Style, children: Vec<usize>) -> Node {
        Node { style, children, text: None }
    }
    fn text(style: Style, t: &str) -> Node {
        Node { style, children: Vec::new(), text: Some(t.into()) }
    }
    fn root() -> Style {
        Style { width: Dim::Percent(1.0), height: Dim::Percent(1.0), ..Style::default() }
    }
    fn workload(scene: Scene) -> Workload {
        let leaf = scene.text_leaves().next();
        Workload {
            name: "test",
            size: "test",
            edit: leaf.map(|node| workloads::Edit {
                node,
                before: scene.nodes[node].text.clone().unwrap(),
                after: "HHHHHHHHHH\u{200B}HHHHHHHHHHHHHHHHHHHH\u{200B}HH".into(),
            }),
            resized: Viewport { width: Avail::Definite(330.0), height: Avail::Definite(200.0) },
            scene,
        }
    }
    fn assert_hooke_taffy_agree(w: &Workload) {
        let rows = gate(w);
        let taffy = rows.iter().find(|r| r.engine == "taffy").unwrap();
        assert_eq!(taffy.cold, Verdict::Agrees, "cold");
        if let Some(e) = &taffy.edit {
            assert_eq!(*e, Verdict::Agrees, "edit");
        }
        assert_eq!(taffy.resize, Verdict::Agrees, "resize");
    }

    #[test]
    fn a_slow_engine_is_too_slow_and_excluded_everywhere() {
        let fast = Run { cold: vec![Frame::default()], edit: None, resize: vec![Frame::default()], cold_secs: 0.001, too_slow: false };
        let slow = Run { cold: vec![Frame::default()], edit: None, resize: Vec::new(), cold_secs: 3.5, too_slow: true };
        let r = row("taffy", &Ok(fast), Ok(slow));
        assert_eq!(r.cold, Verdict::TooSlow { seconds: 3.5 });
        assert_eq!(r.cold.cell(), "too slow: 3.5 s per layout, not sampled");
        for family in ["cold", "deep", "text", "edit", "resize", "candidates"] {
            assert!(r.exclusion(family).is_some_and(|e| e.starts_with("too slow")), "{family}");
        }
    }

    #[test]
    fn compare_finds_the_first_differing_node() {
        let a = vec![Frame::default(), Frame { x: 1.0, ..Frame::default() }, Frame { y: 2.0, ..Frame::default() }];
        assert_eq!(compare(&a, &a), Verdict::Agrees);
        let mut b = a.clone();
        b[1].x += 0.005;
        assert_eq!(compare(&a, &b), Verdict::Agrees, "within tolerance");
        b[1].x += 0.1;
        b[2].y += 1.0;
        assert!(matches!(compare(&a, &b), Verdict::Differs { node: 1, .. }));
        assert!(matches!(compare(&a, &b[..2]), Verdict::DidNotRun(_)));
    }

    #[test]
    fn a_panicking_engine_did_not_run() {
        let w = workload(scene(vec![container(root(), vec![])], 100.0, 100.0));
        let rows = gate(&w);
        assert_eq!(rows[0].engine, "hooke");
        assert_eq!(rows[0].cold, Verdict::Agrees);
        let cephal = rows.iter().find(|r| r.engine == "cephal").unwrap();
        // Until the cephal adapter exists its `build` is `todo!()`; afterwards it must agree.
        assert!(matches!(cephal.cold, Verdict::DidNotRun(_) | Verdict::Agrees));
        if !cephal.cold.agrees() {
            assert!(cephal.exclusion("cold").is_some());
        }
    }

    #[test]
    fn row_with_grow_padding_gap_and_text() {
        let nodes = vec![
            container(
                Style { padding: Edges::all(Lp::Px(10.0)), column_gap: 5.0, align_items: Align::FlexStart, ..root() },
                vec![1, 2, 3],
            ),
            text(Style { grow: 1.0, ..Style::default() }, "HHH\u{200B}HH\u{200B}HHHH"),
            text(Style { grow: 2.0, margin: Edges::all(Dim::Px(3.0)), ..Style::default() }, "HHHHHH\u{200B}HHHHHHHHHHHHHH"),
            container(Style { width: Dim::Px(40.0), height: Dim::Px(25.0), border: Edges::all(2.0), ..Style::default() }, vec![]),
        ];
        assert_hooke_taffy_agree(&workload(scene(nodes, 300.0, 200.0)));
    }

    #[test]
    fn column_of_wrapping_paragraphs() {
        let nodes = vec![
            container(Style { direction: FlexDirection::Row, ..root() }, vec![1]),
            container(
                Style { direction: FlexDirection::Column, width: Dim::Percent(0.5), padding: Edges::all(Lp::Px(5.0)), row_gap: 4.0, ..Style::default() },
                vec![2, 3, 4],
            ),
            text(Style { shrink: 0.0, ..Style::default() }, "HHH\u{200B}HHHHH\u{200B}HH\u{200B}HHHHHHHH\u{200B}H"),
            text(Style { shrink: 0.0, ..Style::default() }, "HHHHHHHH\u{200B}HHHHHHHH\u{200B}HHHHHHHH"),
            text(Style { shrink: 0.0, justify: Justify::Center, ..Style::default() }, "HH"),
        ];
        assert_hooke_taffy_agree(&workload(scene(nodes, 300.0, 200.0)));
    }

    #[test]
    fn small_generated_workloads_agree() {
        for name in ["text", "random", "deep_chain", "fit_nesting", "wide_wrapping"] {
            let size = workloads::sizes(name)[0].0;
            let w = workloads::make(name, size);
            assert_hooke_taffy_agree(&w);
        }
    }
}
