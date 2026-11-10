//! Criterion benchmarks: one group per workload of each family, `BenchmarkId::new(engine,
//! size)`, `Throughput::Elements(node count)` so criterion reports time per node.
//!
//! Before an engine is timed on a workload, the equal-output gate (`hooke_bench::gate`) lays
//! it out in every engine and compares frames with hooke's; an engine that differs, or does
//! not run, is skipped for that workload and the reason is printed.
//!
//! - `cold_*`, `deep_*`, `text_columns`: one cold layout per iteration. Between iterations,
//!   untimed, hooke and Taffy invalidate every node of the existing tree (Taffy's
//!   `mark_dirty` on every node, as arewelayoutyet does; hooke clears every node's caches);
//!   engines without such an operation in their adapter are rebuilt.
//! - `edit_*`: one `set_text` plus `layout` per iteration, alternating between two texts of
//!   the same leaf, so every iteration is exactly one edit against warm caches.
//! - `resize_*`: one `resize` plus `layout` per iteration, alternating between the viewport and
//!   one 10% wider.
//! - `candidates_text`: hooke forks (`hooke-fork`; `hooke-xN` on N threads, `-cache` with a
//!   shared measure cache) against Taffy's deep clone (`taffy-clone`; `taffy-xN` sequentially, as
//!   `TaffyTree` is not `Send`).
//!
//! `HOOKE_BENCH_QUICK=1` or `--quick` runs only the two smaller sizes with fewer samples.

use criterion::{BenchmarkGroup, BenchmarkId, Criterion, Throughput, measurement::WallTime};
use hooke_bench::engine::Engine;
use hooke_bench::engines::cephal::CephalEngine;
use hooke_bench::engines::hooke::HookeEngine;
use hooke_bench::engines::taffy::TaffyEngine;
use hooke_bench::gate::{self, Row};
use hooke_bench::scene::Scene;
use hooke_bench::workloads::{self, Group, Workload};
use std::collections::HashMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOC: hooke_bench::alloc::Counting = hooke_bench::alloc::Counting;

/// How the cold workloads make the next layout cold without timing it.
trait ColdReset: Engine + Sized {
    fn reset(engine: &mut Self, scene: &Scene) {
        *engine = Self::build(scene);
    }
}
impl ColdReset for HookeEngine {
    fn reset(engine: &mut Self, _: &Scene) {
        engine.invalidate_all();
    }
}
impl ColdReset for TaffyEngine {
    fn reset(engine: &mut Self, _: &Scene) {
        engine.invalidate_all();
    }
}
impl ColdReset for CephalEngine {}
#[cfg(feature = "yoga")]
impl ColdReset for hooke_bench::engines::yoga::YogaEngine {}

fn quick() -> bool {
    std::env::var("HOOKE_BENCH_QUICK").is_ok_and(|v| v != "0" && !v.is_empty()) || std::env::args().any(|a| a == "--quick")
}

/// Sample size, warm-up and measurement time by node count.
fn configure(group: &mut BenchmarkGroup<'_, WallTime>, nodes: usize) {
    let (samples, warm, measure) = if quick() {
        (10, 0.2, 0.5)
    } else if nodes <= 2_000 {
        (30, 0.5, 1.5)
    } else if nodes <= 20_000 {
        (20, 0.5, 2.0)
    } else {
        (10, 0.5, 3.0)
    };
    group.sample_size(samples);
    group.warm_up_time(Duration::from_secs_f64(warm));
    group.measurement_time(Duration::from_secs_f64(measure));
}

fn row<'a>(rows: &'a [Row], engine: &str) -> Option<&'a Row> {
    rows.iter().find(|r| r.engine == engine)
}

/// May `E` be timed on `family`? Prints the reason when not.
fn allowed<E: Engine>(rows: &[Row], g: &Group, size: &str) -> bool {
    match row(rows, E::NAME).map(|r| r.exclusion(g.family)) {
        Some(None) => true,
        Some(Some(reason)) => {
            eprintln!("skipping {} in {}/{size}: {reason}", E::NAME, g.group);
            false
        }
        None => false,
    }
}

fn bench_cold<E: ColdReset>(group: &mut BenchmarkGroup<'_, WallTime>, w: &Workload) {
    let mut e = E::build(&w.scene);
    group.bench_function(BenchmarkId::new(E::NAME, w.size), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                E::reset(&mut e, &w.scene);
                let start = Instant::now();
                e.layout();
                total += start.elapsed();
                black_box(&e);
            }
            total
        })
    });
}

fn bench_edit<E: Engine>(group: &mut BenchmarkGroup<'_, WallTime>, w: &Workload) {
    let edit = w.edit.as_ref().expect("edit workloads have text");
    let mut e = E::build(&w.scene);
    e.layout();
    let mut flip = false;
    group.bench_function(BenchmarkId::new(E::NAME, w.size), |b| {
        b.iter(|| {
            flip = !flip;
            e.set_text(edit.node, if flip { &edit.after } else { &edit.before });
            e.layout();
        })
    });
}

fn bench_resize<E: Engine>(group: &mut BenchmarkGroup<'_, WallTime>, w: &Workload) {
    let mut e = E::build(&w.scene);
    e.layout();
    let mut flip = false;
    group.bench_function(BenchmarkId::new(E::NAME, w.size), |b| {
        b.iter(|| {
            flip = !flip;
            e.resize(if flip { w.resized } else { w.scene.viewport });
            e.layout();
        })
    });
}

fn bench_candidates(group: &mut BenchmarkGroup<'_, WallTime>, w: &Workload, rows: &[Row], g: &Group) {
    let edit = w.edit.as_ref().expect("candidate workloads have text");
    let edits: Vec<(usize, String)> =
        workloads::spread_leaves(&w.scene, 8).into_iter().map(|n| (n, format!("{}\u{200B}HHHHH", w.scene.nodes[n].text.as_deref().unwrap_or("")))).collect();
    if allowed::<HookeEngine>(rows, g, w.size) {
        let mut h = HookeEngine::build(&w.scene);
        h.layout();
        group.bench_function(BenchmarkId::new("hooke-fork", w.size), |b| {
            b.iter_with_large_drop(|| h.fork_edit_layout(edit.node, &edit.after))
        });
        for n in [1, 2, 4, 8] {
            for cache in [false, true] {
                let id = format!("hooke-x{n}{}", if cache { "-cache" } else { "" });
                group.bench_function(BenchmarkId::new(id, w.size), |b| {
                    b.iter_with_large_drop(|| h.candidates(&edits[..n], cache))
                });
            }
        }
    }
    if allowed::<TaffyEngine>(rows, g, w.size) {
        let mut t = TaffyEngine::build(&w.scene);
        t.layout();
        group.bench_function(BenchmarkId::new("taffy-clone", w.size), |b| {
            b.iter_with_large_drop(|| t.clone_edit_layout(edit.node, &edit.after))
        });
        for n in [1, 2, 4, 8] {
            group.bench_function(BenchmarkId::new(format!("taffy-x{n}"), w.size), |b| {
                b.iter_with_large_drop(|| t.candidates(&edits[..n]))
            });
        }
    }
}

fn bench_engine<E: ColdReset>(group: &mut BenchmarkGroup<'_, WallTime>, g: &Group, w: &Workload, rows: &[Row]) {
    if !allowed::<E>(rows, g, w.size) {
        return;
    }
    match g.family {
        "edit" => bench_edit::<E>(group, w),
        "resize" => bench_resize::<E>(group, w),
        _ => bench_cold::<E>(group, w),
    }
}

fn benches(c: &mut Criterion) {
    let mut gates: HashMap<(&str, &str), Vec<Row>> = HashMap::new();
    for g in workloads::groups() {
        let mut group = c.benchmark_group(g.group);
        let sizes = workloads::sizes(g.workload);
        let sizes = if quick() { &sizes[..2] } else { sizes };
        for &(size, _) in sizes {
            let w = workloads::make(g.workload, size);
            let rows = gates.entry((g.workload, size)).or_insert_with(|| gate::gate(&w)).clone();
            group.throughput(Throughput::Elements(w.scene.len() as u64));
            configure(&mut group, w.scene.len());
            if g.family == "candidates" {
                bench_candidates(&mut group, &w, &rows, &g);
                continue;
            }
            bench_engine::<HookeEngine>(&mut group, &g, &w, &rows);
            bench_engine::<TaffyEngine>(&mut group, &g, &w, &rows);
            #[cfg(feature = "yoga")]
            bench_engine::<hooke_bench::engines::yoga::YogaEngine>(&mut group, &g, &w, &rows);
            bench_engine::<CephalEngine>(&mut group, &g, &w, &rows);
        }
        group.finish();
    }
}

fn main() {
    // The deep workloads recurse once per level in every engine.
    gate::with_big_stack(|| {
        let mut c = Criterion::default().configure_from_args();
        benches(&mut c);
        c.final_summary();
    });
}
