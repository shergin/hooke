//! The Chrome fixture corpus: every in-scope flexbox fixture from Taffy v0.15.1's Chrome-generated
//! test suite (and a few from cephal), laid out by hooke and compared frame by frame with Chrome.
//!
//! One test runs the whole corpus. It fails when a fixture fails that is not listed in
//! `tests/fixtures/expected_failures.txt`, or when a listed fixture passes or no longer exists.
//!
//! - `HOOKE_FIXTURE=<substring>` runs only the matching fixtures and prints, for each, the fixture
//!   source, the hooke style tree and every node's expected and actual frame (add
//!   `-- --nocapture` to see it).
//! - `HOOKE_FIXTURES_UPDATE=1` rewrites `expected_failures.txt` from this run, keeping the
//!   reasons of entries that still fail and writing `unexplained` for new ones, and passes.
//!
//! See `tests/fixtures/README.md` for the format, the source and the rounding model.

mod support;

use hooke::{Direction, Display, Handle, LayoutCx, Opt, Point, Position, RootConstraints, Scratch, Size};
use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use support::fixtures::{self, Fixture, FixtureNode};
use support::{Node, Shaper};

/// Tolerance for fixtures with rounding (all but a handful): frames are whole pixels.
const TOLERANCE: f32 = 0.01;
/// Tolerance for the few fixtures recorded without rounding: Chrome's unrounded values are
/// LayoutUnits (multiples of 1/64 px), so they carry up to a few 1/64ths of quantisation error.
/// This is the tolerance Taffy's own XML tests use.
const UNROUNDED_TOLERANCE: f32 = 0.1;
const DIRS: [&str; 2] = ["flex", "chrome_truth"];

/// One compared node: its pre-order path, and expected and actual frames relative to the
/// parent's rounded (or, without rounding, unrounded) origin.
struct Row {
    path: String,
    props: String,
    expected: [f32; 4],
    actual: [f32; 4],
    /// Only the size is compared (a `display: none` node).
    size_only: bool,
    ok: bool,
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

/// `name<TAB>reason` lines; blank lines and `#` comments are ignored.
fn read_list(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else { return BTreeMap::new() };
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| match l.split_once('\t') {
            Some((n, r)) => (n.trim().to_string(), r.trim().to_string()),
            None => (l.trim().to_string(), String::new()),
        })
        .collect()
}

/// Lays the fixture out under Taffy's root constraints: the viewport as available space, no
/// imposed size, the root's own direction.
fn lay_out(fixture: &Fixture) -> Handle<Node> {
    let mut root = fixtures::build(&fixture.root, &mut 0);
    let direction = match fixture.root.style.direction() {
        Direction::Rtl => Direction::Rtl,
        _ => Direction::Ltr,
    };
    let constraints =
        RootConstraints { available: fixture.available, known: Size::new(Opt::NONE, Opt::NONE), direction };
    let mut scratch = Scratch::new();
    let mut cx = LayoutCx::new(&mut scratch);
    let mut shaper = Shaper { calls: 0 };
    hooke::layout(&mut root, constraints, &mut cx, &mut shaper).expect("not cancelled");
    root
}

/// Walks both trees in pre-order and compares absolute frames.
///
/// Expected absolute origins follow Chrome's containing blocks: a node's `x y` is relative to its
/// parent, or for `position: absolute` to its nearest non-static ancestor (the root if none).
/// Actual frames are hooke's unrounded absolute origins (sums of parent-relative offsets)
/// snapped with `hooke::snap`, which rounds absolute edges and takes sizes as differences of
/// rounded edges, as Taffy's `round_layout` does and as the expectations were recorded. The
/// root has no offset in hooke, so it is placed at Chrome's root origin.
fn compare(fixture: &Fixture, root: &Handle<Node>) -> Vec<Row> {
    struct Anc {
        position: Position,
        expected_abs: [f32; 2],
    }
    #[allow(clippy::too_many_arguments)]
    fn walk(
        f: &FixtureNode,
        n: &Handle<Node>,
        path: String,
        ancestors: &mut Vec<Anc>,
        actual_parent: (Point<f32>, Point<f32>), // (unrounded, rounded) absolute origin of the parent
        offset: Point<f32>,
        rounding: bool,
        out: &mut Vec<Row>,
    ) {
        let [ex, ey, ew, eh] = f.expected;
        let expected_abs = if ancestors.is_empty() {
            [ex, ey]
        } else {
            let cb = if f.style.position() == Position::Absolute {
                ancestors.iter().rev().find(|a| a.position != Position::Static).unwrap_or(&ancestors[0])
            } else {
                ancestors.last().unwrap()
            };
            [cb.expected_abs[0] + ex, cb.expected_abs[1] + ey]
        };
        let parent_expected_abs = ancestors.last().map_or([0.0, 0.0], |a| a.expected_abs);
        let size = n.layout.size();
        let (unrounded_parent, rounded_parent) = actual_parent;
        let unrounded = Point::new(unrounded_parent.x + offset.x, unrounded_parent.y + offset.y);
        let actual_abs = if rounding {
            hooke::snap(unrounded_parent, offset, size, 1.0)
        } else {
            hooke::Rect::new(unrounded, size)
        };
        let expected = [expected_abs[0] - parent_expected_abs[0], expected_abs[1] - parent_expected_abs[1], ew, eh];
        let actual =
            [actual_abs.x - rounded_parent.x, actual_abs.y - rounded_parent.y, actual_abs.width, actual_abs.height];
        let hidden = f.style.display() == Display::None;
        let tolerance = if rounding { TOLERANCE } else { UNROUNDED_TOLERANCE };
        let close = |i: usize| (expected[i] - actual[i]).abs() <= tolerance;
        let ok = if hidden { close(2) && close(3) } else { (0..4).all(close) };
        out.push(Row { path: path.clone(), props: f.props.clone(), expected, actual, size_only: hidden, ok });
        if hidden {
            // Nothing inside a `display: none` subtree has a box.
            return;
        }
        if n.children.len() != f.children.len() {
            out.push(Row {
                path: format!("{path} (child count {} vs {})", f.children.len(), n.children.len()),
                props: String::new(),
                expected,
                actual,
                size_only: false,
                ok: false,
            });
            return;
        }
        ancestors.push(Anc { position: f.style.position(), expected_abs });
        for (i, (fc, slot)) in f.children.iter().zip(n.children.iter()).enumerate() {
            walk(
                fc,
                &slot.node,
                format!("{path}.{i}"),
                ancestors,
                (unrounded, actual_abs.origin()),
                slot.offset,
                rounding,
                out,
            );
        }
        ancestors.pop();
    }
    let mut out = Vec::new();
    let origin = Point::new(fixture.root.expected[0], fixture.root.expected[1]);
    walk(&fixture.root, root, "root".into(), &mut Vec::new(), (Point::ZERO, Point::ZERO), origin, fixture.rounding, &mut out);
    out
}

fn frame(v: [f32; 4]) -> String {
    format!("({}, {}, {}x{})", v[0], v[1], v[2], v[3])
}

fn first_difference(rows: &[Row]) -> String {
    match rows.iter().find(|r| !r.ok) {
        Some(r) => format!(
            "{}: expected {} actual {}{}",
            r.path,
            frame(r.expected),
            frame(r.actual),
            if r.size_only { " (size only)" } else { "" }
        ),
        None => "no difference".into(),
    }
}

enum Outcome {
    Pass,
    Fail(String),
}

fn run_one(name: &str, src: &str, verbose: bool) -> Outcome {
    let fixture = match fixtures::parse(src) {
        Ok(f) => f,
        Err(e) => panic!("{name}: cannot parse fixture: {e}"),
    };
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let root = lay_out(&fixture);
        let rows = compare(&fixture, &root);
        (root, rows)
    }));
    let (root, rows) = match result {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".into());
            if verbose {
                eprintln!("== {name}\n{src}\nPANICKED: {msg}\n");
            }
            return Outcome::Fail(format!("panicked: {msg}"));
        }
    };
    let ok = rows.iter().all(|r| r.ok);
    if verbose {
        eprintln!("== {name} [{}]\n{}", if ok { "pass" } else { "FAIL" }, fixture.source);
        eprintln!("-- fixture\n{}", src.lines().filter(|l| l.trim_start().starts_with("node")).collect::<Vec<_>>().join("\n"));
        eprintln!("-- hooke style tree");
        support::dump_tree(&root, 0);
        eprintln!("-- frames relative to the parent (x, y, w x h)");
        for r in &rows {
            eprintln!(
                "{} {:<28} expected {:<28} actual {:<28} {}",
                if r.ok { "  " } else { "!!" },
                r.path,
                frame(r.expected),
                frame(r.actual),
                if r.size_only { "(display: none, size only)" } else { r.props.as_str() }
            );
        }
        eprintln!();
    }
    if ok { Outcome::Pass } else { Outcome::Fail(first_difference(&rows)) }
}

#[test]
fn chrome_fixtures() {
    let dir = fixtures_dir();
    let filter = std::env::var("HOOKE_FIXTURE").ok().filter(|s| !s.is_empty());
    let update = std::env::var("HOOKE_FIXTURES_UPDATE").is_ok_and(|v| v == "1");
    let matches = |name: &str| filter.as_ref().is_none_or(|f| name.contains(f.as_str()));

    let mut files = Vec::new();
    for sub in DIRS {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else { continue };
        for e in entries {
            let path = e.unwrap().path();
            if path.extension().is_some_and(|x| x == "txt") {
                let name = format!("{sub}/{}", path.file_stem().unwrap().to_string_lossy());
                files.push((name, path));
            }
        }
    }
    files.sort();
    assert!(!files.is_empty(), "no fixtures in {}", dir.display());

    let expected_failures_path = dir.join("expected_failures.txt");
    let expected_failures = read_list(&expected_failures_path);
    let skipped = read_list(&dir.join("skipped.txt")).into_keys().filter(|n| matches(n)).count();

    // Silence the default panic message of fixtures that panic inside the engine; the panic is
    // reported as that fixture's failure.
    let hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let mut results: BTreeMap<String, Outcome> = BTreeMap::new();
    for (name, path) in &files {
        if !matches(name) {
            continue;
        }
        let src = std::fs::read_to_string(path).unwrap();
        results.insert(name.clone(), run_one(name, &src, filter.is_some()));
    }
    panic::set_hook(hook);

    let passed = results.values().filter(|o| matches!(o, Outcome::Pass)).count();
    let failed = results.len() - passed;
    let expected_count =
        results.iter().filter(|(n, o)| matches!(o, Outcome::Fail(_)) && expected_failures.contains_key(*n)).count();
    let rate = if results.is_empty() { 0.0 } else { 100.0 * passed as f64 / results.len() as f64 };
    println!(
        "chrome fixtures: {passed} passed, {failed} failed ({expected_count} expected), {skipped} skipped; \
         pass rate {rate:.1}% of in-scope"
    );

    if update {
        // Entries outside the filter are kept as they are.
        let mut list: BTreeMap<String, String> =
            expected_failures.iter().filter(|(n, _)| !matches(n)).map(|(n, r)| (n.clone(), r.clone())).collect();
        for (name, outcome) in &results {
            if let Outcome::Fail(_) = outcome {
                let reason = expected_failures.get(name).cloned().unwrap_or_else(|| "unexplained".into());
                list.insert(name.clone(), reason);
            }
        }
        let mut text = String::from(
            "# Fixtures that currently fail, one per line: name<TAB>reason. Maintained by\n\
             # HOOKE_FIXTURES_UPDATE=1 cargo test --test chrome_fixtures; see tests/fixtures/README.md.\n",
        );
        for (n, r) in &list {
            text.push_str(&format!("{n}\t{r}\n"));
        }
        std::fs::write(&expected_failures_path, text).unwrap();
        println!("rewrote {} with {} entries", expected_failures_path.display(), list.len());
        return;
    }

    let mut problems = Vec::new();
    for (name, outcome) in &results {
        match (outcome, expected_failures.contains_key(name)) {
            (Outcome::Fail(diff), false) => problems.push(format!("FAILED (not expected) {name}: {diff}")),
            (Outcome::Pass, true) => {
                problems.push(format!("PASSED (listed in expected_failures.txt; remove it) {name}"))
            }
            _ => {}
        }
    }
    let known: std::collections::BTreeSet<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    for name in expected_failures.keys() {
        if matches(name) && !known.contains(name.as_str()) {
            problems.push(format!("STALE (listed in expected_failures.txt but no such fixture) {name}"));
        }
    }
    if !problems.is_empty() {
        for p in &problems {
            println!("{p}");
        }
        panic!(
            "{} fixture(s) disagree with tests/fixtures/expected_failures.txt; \
             run with HOOKE_FIXTURE=<name> -- --nocapture for details, \
             or HOOKE_FIXTURES_UPDATE=1 to accept the current results",
            problems.len()
        );
    }
}
