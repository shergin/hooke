//! Writes `notes/benchmarks/<YYYY-MM-DD>.md`: the equal-output gate for every workload and size,
//! the memory workload, criterion's results and the machine they ran on.
//!
//! ```text
//! CARGO_TARGET_DIR=target/benchA cargo bench -p hooke-bench        # timings (optional)
//! CARGO_TARGET_DIR=target/benchA cargo run -p hooke-bench --release --bin report
//! ```
//!
//! Criterion's results are read from `$CARGO_TARGET_DIR/criterion` (default `target/criterion`
//! at the workspace root); a benchmark whose directory is absent is reported as "not run".

use hooke_bench::alloc::{self, Snapshot};
use hooke_bench::engine::Engine;
use hooke_bench::engines::cephal::CephalEngine;
use hooke_bench::engines::hooke::HookeEngine;
use hooke_bench::engines::taffy::TaffyEngine;
use hooke_bench::gate::{self, Row, Verdict};
use hooke_bench::workloads::{self, Group, Workload};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;

#[global_allocator]
static ALLOC: alloc::Counting = alloc::Counting;

#[cfg(feature = "yoga")]
const ENGINES: [&str; 4] = ["hooke", "taffy", "yoga", "cephal"];
#[cfg(not(feature = "yoga"))]
const ENGINES: [&str; 4] = ["hooke", "taffy", "cephal", "yoga"];

const CANDIDATE_IDS: [&str; 14] = [
    "hooke-fork",
    "hooke-x1",
    "hooke-x1-cache",
    "hooke-x2",
    "hooke-x2-cache",
    "hooke-x4",
    "hooke-x4-cache",
    "hooke-x8",
    "hooke-x8-cache",
    "taffy-clone",
    "taffy-x1",
    "taffy-x2",
    "taffy-x4",
    "taffy-x8",
];

fn sh(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".into())
}

/// Today's UTC date as `YYYY-MM-DD` (civil-from-days, Howard Hinnant).
fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("bench is inside the workspace").to_path_buf()
}

/// `$CARGO_TARGET_DIR/criterion`, default `target/criterion` at the workspace root. Cargo runs a
/// bench binary from its package directory, so criterion resolves a relative `CARGO_TARGET_DIR`
/// against `bench/`; such a directory is used when the workspace-relative one does not exist.
fn criterion_dir() -> PathBuf {
    let Some(target) = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) else {
        return workspace().join("target").join("criterion");
    };
    if target.is_absolute() {
        return target.join("criterion");
    }
    let at_root = workspace().join(&target).join("criterion");
    let at_bench = Path::new(env!("CARGO_MANIFEST_DIR")).join(&target).join("criterion");
    if !at_root.exists() && at_bench.exists() { at_bench } else { at_root }
}

/// The mean point estimate, in nanoseconds per iteration, of one benchmark.
fn estimate(group: &str, id: &str, size: &str) -> Option<f64> {
    let path = criterion_dir().join(group).join(id).join(size).join("new").join("estimates.json");
    let text = std::fs::read_to_string(path).ok()?;
    let mean = &text[text.find("\"mean\"")?..];
    let pe = &mean[mean.find("\"point_estimate\"")? + "\"point_estimate\"".len()..];
    let pe = pe.trim_start().strip_prefix(':')?.trim_start();
    let end = pe.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '-' | '+'))).unwrap_or(pe.len());
    pe[..end].parse().ok()
}

fn fmt_ns(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

fn fmt_us(ns: f64) -> String {
    let us = ns / 1000.0;
    if us >= 100.0 { format!("{us:.0}") } else if us >= 1.0 { format!("{us:.1}") } else { format!("{us:.3}") }
}

fn short(v: &Verdict) -> String {
    match v {
        Verdict::Agrees => "agrees".into(),
        Verdict::Differs { node, .. } => format!("differs at #{node}"),
        Verdict::DidNotRun(_) => "did not run".into(),
        Verdict::TooSlow { .. } => v.cell(),
    }
}

/// The gate's verdict that `engine` was too slow on `key`, if it was.
fn too_slow(gated: &BTreeMap<(&'static str, &'static str), Gated>, key: (&'static str, &'static str), engine: &str) -> Option<f64> {
    gated.get(&key)?.rows.iter().find(|r| r.engine == engine).and_then(|r| match r.cold {
        Verdict::TooSlow { seconds } => Some(seconds),
        _ => None,
    })
}

/// One gate cell: cold, edit, resize.
fn gate_cell(r: &Row) -> String {
    match r.cold {
        Verdict::DidNotRun(_) => return "did not run".into(),
        Verdict::TooSlow { .. } => return r.cold.cell(),
        _ => {}
    }
    let e = r.edit.as_ref().map(short).unwrap_or_else(|| "n/a".into());
    if r.cold.agrees() && r.edit.as_ref().is_none_or(Verdict::agrees) && r.resize.agrees() {
        return "agrees".into();
    }
    format!("cold {}; edit {e}; resize {}", short(&r.cold), short(&r.resize))
}

struct Gated {
    w: Workload,
    rows: Vec<Row>,
}

/// Memory of one engine on one workload.
struct Mem {
    bytes_per_node: f64,
    warm_allocs: u64,
    edit_allocs: u64,
    edit_bytes: u64,
}

/// The memory workload for one engine. Bounded like the gate: if the cold layout takes longer
/// than [`gate::slow_secs`], nothing else is run and the error is "too slow".
fn memory<E: Engine>(w: &Workload) -> Result<Mem, String> {
    catch_unwind(AssertUnwindSafe(|| {
        let before = alloc::snapshot();
        let mut e = E::build(&w.scene);
        let start = std::time::Instant::now();
        e.layout();
        let secs = start.elapsed().as_secs_f64();
        let built = alloc::snapshot().since(before);
        if secs > gate::slow_secs() {
            return Err(format!("too slow: {} s per layout, not sampled", gate::secs(secs)));
        }
        let before = alloc::snapshot();
        e.layout();
        let warm = alloc::snapshot().since(before);
        let edit: Snapshot = match &w.edit {
            Some(ed) => {
                let before = alloc::snapshot();
                e.set_text(ed.node, &ed.after);
                e.layout();
                alloc::snapshot().since(before)
            }
            None => Snapshot::default(),
        };
        drop(e);
        Ok(Mem {
            bytes_per_node: built.current as f64 / w.scene.len() as f64,
            warm_allocs: warm.count,
            edit_allocs: edit.count,
            edit_bytes: edit.total,
        })
    }))
    .unwrap_or_else(|p| {
        Err(p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default())
    })
}

/// [`memory`], unless the gate already found the engine too slow on this workload size.
fn memory_if<E: Engine>(gated: &BTreeMap<(&'static str, &'static str), Gated>, w: &Workload) -> Result<Mem, String> {
    match too_slow(gated, (w.name, w.size), E::NAME) {
        Some(secs) => Err(format!("too slow: {} s per layout, not sampled", gate::secs(secs))),
        None => memory::<E>(w),
    }
}

fn measure_calls<E: Engine>(w: &Workload) -> Option<u64> {
    catch_unwind(AssertUnwindSafe(|| {
        let mut e = E::build(&w.scene);
        e.reset_measure_calls();
        e.layout();
        e.measure_calls()
    }))
    .ok()
}

fn main() {
    // Local date where `date` exists, else UTC.
    let date = Some(sh("date", &["+%F"])).filter(|d| d.len() == 10).unwrap_or_else(today);
    let mut out = String::new();
    let quick = std::env::var("HOOKE_BENCH_QUICK").is_ok_and(|v| v != "0" && !v.is_empty());

    // ---- gate ------------------------------------------------------------------------------
    eprintln!("running the gate ...");
    let mut gated: BTreeMap<(&'static str, &'static str), Gated> = BTreeMap::new();
    let mut order: Vec<(&'static str, &'static str)> = Vec::new();
    for name in workloads::workload_names() {
        let sizes = workloads::sizes(name);
        let sizes = if quick { &sizes[..2] } else { sizes };
        for &(size, _) in sizes {
            eprintln!("  {name} {size}");
            let w = workloads::make(name, size);
            let rows = gate::gate(&w);
            order.push((name, size));
            gated.insert((name, size), Gated { w, rows });
        }
    }

    let hooke_ran = gated.values().any(|g| g.rows[0].cold.agrees());
    let ran: Vec<&str> = ENGINES
        .iter()
        .copied()
        .filter(|e| gated.values().any(|g| g.rows.iter().any(|r| r.engine == *e && !matches!(r.cold, Verdict::DidNotRun(_)))))
        .collect();

    writeln!(out, "# hooke benchmarks, {date}\n").unwrap();
    writeln!(
        out,
        "Generated by `cargo run -p hooke-bench --release --bin report`. Criterion results are read from `{}`.\n",
        criterion_dir().display()
    )
    .unwrap();
    writeln!(out, "Engines that ran: {}.\n", ran.join(", ")).unwrap();
    if !hooke_ran {
        writeln!(out, "**hooke did not run; every comparison below is void.**\n").unwrap();
    }

    writeln!(out, "## Equal-output gate\n").unwrap();
    writeln!(
        out,
        "Every engine lays out the scene cold, then after the workload's text edit, then after its 10% resize; each \
         frame is compared with hooke's, node by node, absolute tolerance {} on x, y, width and height, unrounded. A \
         cell reads `agrees` when all three agree. Nodes are numbered in pre-order, root 0. An engine whose first cold \
         layout takes longer than {} s (`HOOKE_BENCH_SLOW_SECS`) is `too slow`: its edit and resize are skipped and it \
         is not sampled on that workload size; the time shown is the gate's one layout.\n",
        gate::TOLERANCE,
        gate::slow_secs()
    )
    .unwrap();
    let others: Vec<&str> = ENGINES[1..].to_vec();
    writeln!(out, "| workload | size | nodes | {} |", others.join(" | ")).unwrap();
    writeln!(out, "|---|---|---:|{}", "---|".repeat(others.len())).unwrap();
    let mut details = Vec::new();
    let mut did_not_run: BTreeMap<&str, String> = BTreeMap::new();
    for key in &order {
        let g = &gated[key];
        let cells: Vec<String> = others
            .iter()
            .map(|e| match g.rows.iter().find(|r| r.engine == *e) {
                Some(r) => {
                    if let Verdict::DidNotRun(reason) = &r.cold {
                        did_not_run.entry(r.engine).or_insert_with(|| reason.clone());
                    }
                    for (phase, v) in [("cold", Some(&r.cold)), ("edit", r.edit.as_ref()), ("resize", Some(&r.resize))] {
                        if let Some(v @ Verdict::Differs { .. }) = v {
                            details.push(format!("- {} {} {}, {phase}: {}", key.0, key.1, r.engine, v.cell()));
                        }
                    }
                    gate_cell(r)
                }
                None => "not compiled".into(),
            })
            .collect();
        writeln!(out, "| {} | {} | {} | {} |", key.0, key.1, g.w.scene.len(), cells.join(" | ")).unwrap();
    }
    if !details.is_empty() {
        writeln!(out, "\nDifferences (first differing node; hooke's frame, then the other engine's, as x, y, width x height):\n").unwrap();
        for d in &details {
            writeln!(out, "{d}").unwrap();
        }
    }
    writeln!(out).unwrap();

    // ---- timings ---------------------------------------------------------------------------
    let groups = workloads::groups();
    let families = [
        ("cold", "Cold layout", "One cold layout of the whole tree per iteration (hooke and Taffy invalidate every node between iterations, untimed; other engines are rebuilt). ns per node."),
        ("deep", "Deep auto-sized nesting", "Cold layout of auto-sized nesting: `deep_chain` (a chain alternating row and column, one text leaf) and `deep_branch` (fan-out 2 to 3, text leaves). ns per node."),
        ("text", "Text-heavy tree", "Cold layout of ten columns of wrapping paragraphs. ns per node, and measure callbacks per text leaf during one cold layout."),
        ("edit", "Incremental relayout after one text edit", "One `set_text` of a leaf about halfway through pre-order plus `layout`, against warm caches. hooke forks the committed revision and path-copies; Taffy marks dirty; Cephal and Yoga use their own invalidation. ns per node of the whole tree, and microseconds per edit."),
        ("resize", "Root resize", "Viewport width changed by 10% (alternating), then `layout` with warm caches. ns per node, and microseconds per resize."),
    ];
    let mut excluded: Vec<String> = Vec::new();
    for (family, title, blurb) in families {
        writeln!(out, "## {title}\n\n{blurb}\n").unwrap();
        let per_op = matches!(family, "edit" | "resize");
        for g in groups.iter().filter(|g| g.family == family) {
            write_group(&mut out, g, &gated, per_op, family == "text", &mut excluded);
        }
    }

    // candidates
    writeln!(out, "## Candidates (hooke-only operations)\n").unwrap();
    writeln!(
        out,
        "On the text tree. `hooke-fork`: clone the committed root handle, path-copy one text edit, lay the fork out \
         (the committed revision stays intact). `hooke-xN`: N forks from the same base, each editing a different leaf, \
         laid out on N scoped threads (thread spawn included), each with a fresh scratch; `-cache` shares one \
         `SharedMeasureCache` between them. `taffy-clone`: `TaffyTree::clone` plus the edit plus `compute_layout`; \
         `taffy-xN`: N of those, one after another on one thread, because `TaffyTree` 0.15 is neither `Send` nor `Sync`. Microseconds per operation (all N candidates), and ns per node of one tree.\n"
    )
    .unwrap();
    for g in groups.iter().filter(|g| g.family == "candidates") {
        let sizes: Vec<(&str, usize)> = order.iter().filter(|k| k.0 == g.workload).map(|k| (k.1, gated[k].w.scene.len())).collect();
        writeln!(out, "### {}\n", g.group).unwrap();
        writeln!(out, "| operation | {} |", sizes.iter().map(|s| format!("{} ({} nodes)", s.0, s.1)).collect::<Vec<_>>().join(" | ")).unwrap();
        writeln!(out, "|---|{}", "---:|".repeat(sizes.len())).unwrap();
        for id in CANDIDATE_IDS {
            let cells: Vec<String> = sizes
                .iter()
                .map(|(size, nodes)| match estimate(g.group, id, size) {
                    Some(ns) => format!("{} us ({} ns/node)", fmt_us(ns), fmt_ns(ns / *nodes as f64)),
                    None => "not run".into(),
                })
                .collect();
            writeln!(out, "| {id} | {} |", cells.join(" | ")).unwrap();
        }
        writeln!(out).unwrap();
    }

    // ---- memory ----------------------------------------------------------------------------
    writeln!(out, "## Memory\n").unwrap();
    if alloc::installed() {
        writeln!(
            out,
            "Through the counting global allocator. *Bytes/node*: heap bytes still allocated after build plus one cold \
             layout, divided by the node count (adapter bookkeeping included, the scene itself excluded). *Warm allocs*: \
             allocations during a second, unchanged `layout`. *Edit allocs* / *edit bytes*: allocations and bytes \
             requested during one `set_text` plus `layout`.\n"
        )
        .unwrap();
        writeln!(out, "| workload | size | engine | bytes/node | warm allocs | edit allocs | edit bytes |").unwrap();
        writeln!(out, "|---|---|---|---:|---:|---:|---:|").unwrap();
        let mem_set: [(&'static str, &'static str); 5] =
            [("fit_nesting", "10k"), ("random", "10k"), ("text", "10k"), ("random", "100k"), ("text", "100k")];
        for (name, size) in mem_set {
            if quick && size == "100k" {
                continue;
            }
            let w = workloads::make(name, size);
            let mut res: Vec<(&str, Result<Mem, String>)> =
                vec![("hooke", memory_if::<HookeEngine>(&gated, &w)), ("taffy", memory_if::<TaffyEngine>(&gated, &w))];
            #[cfg(feature = "yoga")]
            res.push(("yoga", memory_if::<hooke_bench::engines::yoga::YogaEngine>(&gated, &w)));
            res.push(("cephal", memory_if::<CephalEngine>(&gated, &w)));
            for (engine, r) in res {
                match r {
                    Ok(m) => writeln!(
                        out,
                        "| {name} | {size} | {engine} | {:.0} | {} | {} | {} |",
                        m.bytes_per_node,
                        m.warm_allocs,
                        if w.edit.is_some() { m.edit_allocs.to_string() } else { "n/a".into() },
                        if w.edit.is_some() { m.edit_bytes.to_string() } else { "n/a".into() },
                    )
                    .unwrap(),
                    Err(e) if e.starts_with("too slow") => writeln!(out, "| {name} | {size} | {engine} | {e} | | | |").unwrap(),
                    Err(_) => writeln!(out, "| {name} | {size} | {engine} | did not run | | | |").unwrap(),
                }
            }
        }
        writeln!(out).unwrap();
    } else {
        writeln!(out, "The counting allocator is not installed in this binary; no memory numbers.\n").unwrap();
    }

    // ---- machine ---------------------------------------------------------------------------
    writeln!(out, "## Machine\n").unwrap();
    writeln!(out, "- `uname -a`: {}", sh("uname", &["-a"])).unwrap();
    writeln!(out, "- CPU: {}", sh("sysctl", &["-n", "machdep.cpu.brand_string"])).unwrap();
    writeln!(out, "- logical CPUs: {}", sh("sysctl", &["-n", "hw.ncpu"])).unwrap();
    let mem = sh("sysctl", &["-n", "hw.memsize"]);
    let mem_gib = mem.parse::<f64>().map(|b| format!(" ({:.0} GiB)", b / (1u64 << 30) as f64)).unwrap_or_default();
    writeln!(out, "- memory: {mem} bytes{mem_gib}").unwrap();
    writeln!(out, "- toolchain: {}", sh("rustc", &["-V"])).unwrap();
    writeln!(
        out,
        "- engines: hooke {} (workspace, commit {}), taffy 0.15.1, cephal 0.2.0, yoga 0.5.0 ({})",
        env!("CARGO_PKG_VERSION"),
        sh("git", &["-C", &workspace().display().to_string(), "rev-parse", "--short", "HEAD"]),
        if cfg!(feature = "yoga") { "compiled in" } else { "not compiled: build with `--features yoga`" }
    )
    .unwrap();
    writeln!(out, "- profile: release; rounding disabled in every engine; the counting allocator is installed in the bench binary too\n").unwrap();

    // ---- omissions -------------------------------------------------------------------------
    writeln!(out, "## Omissions\n").unwrap();
    writeln!(
        out,
        "- No instruction counts: iai-callgrind needs Valgrind, which does not run on macOS on Apple Silicon. All timings \
         are wall-clock (criterion)."
    )
    .unwrap();
    writeln!(out, "- arewelayoutyet's `percentage_and_ratio` is not ported: the scene's style subset has no `aspect-ratio`.").unwrap();
    if !cfg!(feature = "yoga") {
        writeln!(out, "- Yoga is not compiled into this build (`--features yoga` needs a C++ toolchain and libclang).").unwrap();
    }
    for (engine, reason) in &did_not_run {
        writeln!(out, "- {engine} did not run: `{}`.", reason.lines().next().unwrap_or("")).unwrap();
    }
    for e in &excluded {
        writeln!(out, "- {e}").unwrap();
    }
    writeln!(out).unwrap();

    // ---- reading ---------------------------------------------------------------------------
    writeln!(out, "## What the numbers show and do not show\n").unwrap();
    writeln!(out, "{}", reading(&groups, &gated)).unwrap();

    let dir = workspace().join("notes").join("benchmarks");
    std::fs::create_dir_all(&dir).expect("create notes/benchmarks");
    let path = dir.join(format!("{date}.md"));
    std::fs::write(&path, out).expect("write report");
    println!("{}", path.display());
}

fn write_group(
    out: &mut String,
    g: &Group,
    gated: &BTreeMap<(&'static str, &'static str), Gated>,
    per_op: bool,
    text: bool,
    excluded: &mut Vec<String>,
) {
    let sizes: Vec<&'static str> =
        workloads::sizes(g.workload).iter().map(|s| s.0).filter(|s| gated.contains_key(&(g.workload, *s))).collect();
    writeln!(out, "### {}\n", g.group).unwrap();
    let mut header = vec!["engine".to_string()];
    for s in &sizes {
        header.push(format!("{s} ({} nodes)", gated[&(g.workload, *s)].w.scene.len()));
    }
    writeln!(out, "| {} |", header.join(" | ")).unwrap();
    writeln!(out, "|---|{}", "---:|".repeat(sizes.len())).unwrap();
    for engine in ENGINES {
        if engine == "yoga" && !cfg!(feature = "yoga") {
            continue;
        }
        let cells: Vec<String> = sizes
            .iter()
            .map(|size| {
                let gw = &gated[&(g.workload, *size)];
                let nodes = gw.w.scene.len() as f64;
                let row = gw.rows.iter().find(|r| r.engine == engine);
                if let Some(reason) = row.and_then(|r| r.exclusion(g.family)) {
                    if reason.starts_with("too slow") {
                        excluded.push(format!("{engine} not sampled in {}/{size}: {reason}", g.group));
                        return "not sampled (too slow)".into();
                    }
                    if !reason.starts_with("did not run") {
                        excluded.push(format!("{engine} excluded from {}/{size}: {reason}", g.group));
                    }
                    return if reason.starts_with("did not run") { "did not run".into() } else { "excluded (gate)".into() };
                }
                match estimate(g.group, engine, size) {
                    Some(ns) if per_op => format!("{} ns/node, {} us", fmt_ns(ns / nodes), fmt_us(ns)),
                    Some(ns) => fmt_ns(ns / nodes),
                    None => "not run".into(),
                }
            })
            .collect();
        writeln!(out, "| {engine} | {} |", cells.join(" | ")).unwrap();
    }
    if text {
        writeln!(out, "\nMeasure callbacks per text leaf, one cold layout:\n").unwrap();
        writeln!(out, "| engine | {} |", sizes.join(" | ")).unwrap();
        writeln!(out, "|---|{}", "---:|".repeat(sizes.len())).unwrap();
        let mut line = |engine: &str, f: &dyn Fn(&Workload) -> Option<u64>| {
            let cells: Vec<String> = sizes
                .iter()
                .map(|s| {
                    let w = &gated[&(g.workload, *s)].w;
                    let leaves = w.scene.text_leaves().count().max(1) as f64;
                    if too_slow(gated, (g.workload, *s), engine).is_some() {
                        return "not sampled (too slow)".into();
                    }
                    f(w).map(|c| format!("{:.2}", c as f64 / leaves)).unwrap_or_else(|| "did not run".into())
                })
                .collect();
            writeln!(out, "| {engine} | {} |", cells.join(" | ")).unwrap();
        };
        line("hooke", &measure_calls::<HookeEngine>);
        line("taffy", &measure_calls::<TaffyEngine>);
        #[cfg(feature = "yoga")]
        line("yoga", &measure_calls::<hooke_bench::engines::yoga::YogaEngine>);
        line("cephal", &measure_calls::<CephalEngine>);
    }
    writeln!(out).unwrap();
}

/// A first draft of the plain-language reading, computed from the numbers.
fn reading(groups: &[Group], gated: &BTreeMap<(&'static str, &'static str), Gated>) -> String {
    let mut s = String::new();
    let ratio = |group: &str, size: &str| -> Option<f64> { Some(estimate(group, "hooke", size)? / estimate(group, "taffy", size)?) };
    let mut cold = Vec::new();
    for g in groups.iter().filter(|g| matches!(g.family, "cold" | "deep" | "text")) {
        for &(size, _) in workloads::sizes(g.workload) {
            if let Some(r) = ratio(g.group, size) {
                cold.push((g.group, size, r));
            }
        }
    }
    if cold.is_empty() {
        s.push_str("No timings were found; run `cargo bench -p hooke-bench` first. The gate and memory sections stand on their own.\n");
        return s;
    }
    let geo = (cold.iter().map(|c| c.2.ln()).sum::<f64>() / cold.len() as f64).exp();
    let (best, worst) = {
        let mut v = cold.clone();
        v.sort_by(|a, b| a.2.total_cmp(&b.2));
        (v[0], v[v.len() - 1])
    };
    let _ = writeln!(
        s,
        "- **Cold layout.** Over {} cold-layout benchmarks where both ran, hooke's time is {:.2}x Taffy's (geometric \
         mean; below 1 means hooke is faster). Best for hooke: {} {} at {:.2}x; worst: {} {} at {:.2}x. Cold layout is \
         the case hooke is not designed around, and arewelayoutyet's table measures only this.",
        cold.len(),
        geo,
        best.0,
        best.1,
        best.2,
        worst.0,
        worst.1,
        worst.2
    );
    for (group, label) in [("edit_text", "text tree"), ("edit_random", "random tree")] {
        let parts: Vec<String> = workloads::SIZES
            .iter()
            .filter_map(|&(size, _)| {
                let h = estimate(group, "hooke", size)?;
                let t = estimate(group, "taffy", size)?;
                Some(format!("{size}: hooke {} us, Taffy {} us", fmt_us(h), fmt_us(t)))
            })
            .collect();
        if !parts.is_empty() {
            let _ = writeln!(s, "- **One text edit, {label}.** {}.", parts.join("; "));
        }
    }
    for (group, label) in [("resize_text", "text tree"), ("resize_random", "random tree")] {
        let parts: Vec<String> = workloads::SIZES
            .iter()
            .filter_map(|&(size, _)| {
                let h = estimate(group, "hooke", size)?;
                let t = estimate(group, "taffy", size)?;
                Some(format!("{size}: hooke {} us, Taffy {} us", fmt_us(h), fmt_us(t)))
            })
            .collect();
        if !parts.is_empty() {
            let _ = writeln!(s, "- **Resize, {label}.** {}.", parts.join("; "));
        }
    }
    let parts: Vec<String> = workloads::SIZES
        .iter()
        .filter_map(|&(size, _)| {
            let h = estimate("candidates_text", "hooke-fork", size)?;
            let t = estimate("candidates_text", "taffy-clone", size)?;
            Some(format!("{size}: fork {} us vs clone {} us", fmt_us(h), fmt_us(t)))
        })
        .collect();
    if !parts.is_empty() {
        let _ = writeln!(
            s,
            "- **A candidate revision.** hooke's fork, path copy and layout against Taffy's deep clone, edit and layout: {}. \
             Taffy has no cheaper way to keep the committed tree intact while laying out a candidate.",
            parts.join("; ")
        );
    }
    let differs: usize = gated.values().filter(|g| g.rows.iter().any(|r| matches!(r.cold, Verdict::Differs { .. }))).count();
    let _ = writeln!(
        s,
        "- **Equal output.** {differs} of {} workload sizes had at least one engine differ from hooke on the cold layout; \
         an engine is timed only where it agrees, so every timing above compares the same layout.",
        gated.len()
    );
    s.push_str(
        "- **Not shown.** Instruction counts (no Valgrind on this machine); real text shaping (every engine uses the same \
         10px Ahem model, so the text numbers measure how often and under what constraints engines ask, not shaping \
         cost); rounding; multi-process or GPU effects; trees outside the style subset that every engine supports. \
         The per-node numbers divide by the scene's node count, not by nodes an engine visited.\n",
    );
    s
}
