//! Loops one hooke operation for a wall-clock budget, so a sampling profiler (Instruments'
//! Time Profiler through `xctrace`, `perf`, `samply`) sees nothing else.
//!
//! ```text
//! profile <op> <workload> <size> [seconds=5] [--split]
//! ```
//!
//! `op` is `edit`, `cold`, `resize`, `fork`, `candidates8` or `build`; `workload` and `size` are
//! as in [`workloads::make`]. Each op repeats what the matching criterion bench times. `--split`
//! (with `edit`) also reports `set_text` and `layout` separately. No counting allocator is
//! installed, so the profile shows hooke's own cost. See `tools/profile/README.md`.

use hooke_bench::engine::Engine;
use hooke_bench::engines::hooke::HookeEngine;
use hooke_bench::gate;
use hooke_bench::scene::NodeId;
use hooke_bench::workloads::{self, Workload};
use std::hint::black_box;
use std::time::{Duration, Instant};

const OPS: &str = "edit, cold, resize, fork, candidates8, build";
const WARM_UP: u32 = 3;

fn usage() -> ! {
    eprintln!("usage: profile <op> <workload> <size> [seconds=5] [--split]\n  op: {OPS}");
    std::process::exit(2)
}

fn main() {
    let mut split = false;
    let mut pos: Vec<String> = Vec::new();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--split" => split = true,
            "-h" | "--help" => usage(),
            _ => pos.push(a),
        }
    }
    if !(3..=4).contains(&pos.len()) {
        usage();
    }
    let secs: f64 = pos.get(3).map_or(5.0, |s| s.parse().unwrap_or_else(|_| usage()));
    let op: &'static str = pos[0].clone().leak();
    let name: &'static str = pos[1].clone().leak();
    let size: &'static str = pos[2].clone().leak();
    if !OPS.split(", ").any(|o| o == op) {
        usage();
    }
    let budget = Duration::from_secs_f64(secs);
    gate::with_big_stack(move || {
        let w = workloads::make(name, size);
        eprintln!("profile: {op} {name} {size}: {} nodes, {secs} s", w.scene.len());
        run(op, &w, budget, split);
    });
}

/// Runs `step` three times untimed, then until `budget` is spent; returns iterations and the
/// total time.
fn timed(budget: Duration, mut step: impl FnMut()) -> (u64, Duration) {
    for _ in 0..WARM_UP {
        step();
    }
    let start = Instant::now();
    let mut iters = 0u64;
    while start.elapsed() < budget {
        step();
        iters += 1;
    }
    (iters, start.elapsed())
}

fn us(d: Duration, iters: u64) -> f64 {
    d.as_secs_f64() * 1e6 / iters.max(1) as f64
}

fn run(op: &str, w: &Workload, budget: Duration, split: bool) {
    let needs_edit = || {
        w.edit
            .as_ref()
            .unwrap_or_else(|| panic!("workload {} has no text leaves", w.name))
    };
    let mut e = HookeEngine::build(&w.scene);
    e.layout();
    let (mut t_set, mut t_layout) = (Duration::ZERO, Duration::ZERO);
    let (iters, total) = match op {
        "edit" => {
            let edit = needs_edit();
            let mut flip = false;
            timed(budget, || {
                flip = !flip;
                let text = if flip { &edit.after } else { &edit.before };
                if split {
                    let t0 = Instant::now();
                    e.set_text(edit.node, text);
                    let t1 = Instant::now();
                    e.layout();
                    let t2 = Instant::now();
                    t_set += t1 - t0;
                    t_layout += t2 - t1;
                } else {
                    e.set_text(edit.node, text);
                    e.layout();
                }
            })
        }
        "cold" => timed(budget, || {
            e.invalidate_all();
            e.layout();
            black_box(&e);
        }),
        "resize" => {
            let mut flip = false;
            timed(budget, || {
                flip = !flip;
                e.resize(if flip { w.resized } else { w.scene.viewport });
                e.layout();
            })
        }
        "fork" => {
            let edit = needs_edit();
            timed(budget, || drop(black_box(e.fork_edit_layout(edit.node, &edit.after))))
        }
        "candidates8" => {
            let edits: Vec<(NodeId, String)> = workloads::spread_leaves(&w.scene, 8)
                .into_iter()
                .map(|n| {
                    (
                        n,
                        format!("{}\u{200B}HHHHH", w.scene.nodes[n].text.as_deref().unwrap_or("")),
                    )
                })
                .collect();
            timed(budget, || drop(black_box(e.candidates(&edits, false))))
        }
        "build" => timed(budget, || drop(black_box(HookeEngine::build(&w.scene)))),
        _ => unreachable!(),
    };
    println!(
        "op={op} workload={} size={} iterations={iters} total_s={:.3} mean_us={:.3}",
        w.name,
        w.size,
        total.as_secs_f64(),
        us(total, iters)
    );
    if split && op == "edit" {
        // The split sums include the warm-up iterations.
        let n = iters + u64::from(WARM_UP);
        println!(
            "split: set_text_mean_us={:.3} layout_mean_us={:.3}",
            us(t_set, n),
            us(t_layout, n)
        );
    } else if split {
        eprintln!("profile: --split applies to edit only");
    }
}
