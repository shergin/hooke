//! Two cost guarantees: a deep chain of auto-sized containers is laid out in linear time with a
//! bounded number of measurer calls, and a warmed-up pass allocates nothing beyond what
//! copy-on-write demands.

mod support;

use hooke::{
    AlignItems, Available, Edges, FlexDirection, Handle, JustifyContent, LayoutCx, Length, Opt, Position,
    RootConstraints, Scratch, Size, Stats, Style,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use support::*;

// ---------------------------------------------------------------------------------------------
// Allocation counting
// ---------------------------------------------------------------------------------------------

/// Counts allocations made by the thread that armed it. All counting state is thread-local,
/// so tests running concurrently on other threads neither add to nor reset the count.
struct Counting;

const SIZES_KEPT: usize = 16;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    /// The sizes of the first counted allocations, for diagnosing a failure.
    static SIZES: [Cell<usize>; SIZES_KEPT] = const { [const { Cell::new(0) }; SIZES_KEPT] };
}

fn note_allocation(layout: Layout) {
    if !ARMED.try_with(Cell::get).unwrap_or(false) {
        return;
    }
    let n = ALLOCATIONS.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    });
    SIZES.with(|sizes| {
        if let Some(s) = sizes.get(n) {
            s.set(layout.size());
        }
    });
}

/// The sizes of the first (up to 16) allocations of the current thread's last counted window.
fn allocation_sizes() -> Vec<usize> {
    let n = ALLOCATIONS.with(Cell::get).min(SIZES_KEPT);
    SIZES.with(|sizes| sizes[..n].iter().map(Cell::get).collect())
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation(layout);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation(layout);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller guarantees `new_size` is valid for `layout.align()`.
        note_allocation(unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) });
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Runs `f` and returns its result with the number of allocations (and reallocations) the
/// current thread made inside it. `f` must not allocate for its own bookkeeping.
fn count_allocations<R>(f: impl FnOnce() -> R) -> (R, usize) {
    ALLOCATIONS.with(|c| c.set(0));
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (r, ALLOCATIONS.with(Cell::get))
}

// ---------------------------------------------------------------------------------------------
// Deep chains
// ---------------------------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
enum Chain {
    Rows,
    Columns,
    Alternating,
    AlternatingFlexStart,
    AlternatingPadded,
}

const CHAINS: [Chain; 5] = [Chain::Rows, Chain::Columns, Chain::Alternating, Chain::AlternatingFlexStart, Chain::AlternatingPadded];
const DEPTHS: [usize; 4] = [10, 25, 50, 100];

impl Chain {
    fn style(self, level: usize) -> Style {
        let alternate = if level.is_multiple_of(2) { FlexDirection::Row } else { FlexDirection::Column };
        match self {
            Chain::Rows => Style::web(),
            Chain::Columns => Style::web().with_flex_direction(FlexDirection::Column),
            Chain::Alternating => Style::web().with_flex_direction(alternate),
            Chain::AlternatingFlexStart => {
                Style::web().with_flex_direction(alternate).with_align_items(AlignItems::FlexStart)
            }
            // Every level adds one pixel on each side, so no two levels have the same size.
            Chain::AlternatingPadded => {
                Style::web().with_flex_direction(alternate).with_padding(Edges::all(Length::Px(1.0)))
            }
        }
    }
}

/// `depth` nested containers, one child each, with a text leaf at the bottom. The leaf's path
/// is `[0; depth]`.
fn chain(kind: Chain, depth: usize) -> Handle<Node> {
    let mut node = text(depth as u32, Style::web(), 120.0, 16.0);
    for level in (0..depth).rev() {
        node = view(level as u32, kind.style(level), vec![node]);
    }
    node
}

fn max_content() -> RootConstraints<f32> {
    RootConstraints {
        available: Size::new(Available::MaxContent, Available::MaxContent),
        known: Size::new(Opt::NONE, Opt::NONE),
        direction: hooke::Direction::Ltr,
    }
}

fn constraint_sets() -> [(&'static str, RootConstraints<f32>); 2] {
    [("viewport", RootConstraints::viewport(800.0, 600.0)), ("max-content", max_content())]
}

/// One pass with a fresh scratch: the root's size, the stats and the measurer calls.
fn pass(root: &mut Handle<Node>, constraints: RootConstraints<f32>) -> (Size<f32>, Stats, u32) {
    let mut scratch = Scratch::new();
    let mut cx = LayoutCx::new(&mut scratch);
    let mut shaper = Shaper { calls: 0 };
    let size = hooke::layout(root, constraints, &mut cx, &mut shaper).expect("not cancelled");
    (size, cx.stats, shaper.calls)
}

/// The most measurer calls any chain needs, whatever its depth: observed, not a loose bound.
/// The leaf is asked once (at max-content, or at its stretched size) and every later question
/// is answered by its slots, the per-pass table or an ancestor's memo.
const MAX_CALLS: u32 = 1;
/// Nodes visited per level of the chain, at most.
const VISITS_PER_LEVEL: u32 = 20;

#[test]
fn deep_chains_measure_the_leaf_a_bounded_number_of_times_and_visit_linearly() {
    for kind in CHAINS {
        for (label, constraints) in constraint_sets() {
            for depth in DEPTHS {
                let mut root = chain(kind, depth);
                let (size, cold, cold_calls) = pass(&mut root, constraints);
                let visit_bound = VISITS_PER_LEVEL * (depth as u32 + 1);
                eprintln!(
                    "{kind:?} {label} depth {depth}: cold calls {cold_calls}, visited {}, stats {cold:?}, size {size:?}",
                    cold.visited
                );
                assert!(cold_calls <= MAX_CALLS, "{kind:?} {label} depth {depth}: {cold_calls} measurer calls");
                assert!(cold.visited <= visit_bound, "{kind:?} {label} depth {depth}: visited {}", cold.visited);
                assert_eq!(cold.nodes_cloned, 0);

                // The same tree again, no edits: everything comes from the memos.
                let (again_size, warm, warm_calls) = pass(&mut root, constraints);
                eprintln!("{kind:?} {label} depth {depth}: warm calls {warm_calls}, stats {warm:?}");
                assert_eq!(again_size, size);
                assert_eq!(warm_calls, 0, "{kind:?} {label} depth {depth}: an unchanged tree is not re-measured");
                assert_eq!(warm.nodes_cloned, 0);

                // Edit the leaf through a host path copy.
                let path = vec![0; depth];
                {
                    let leaf = node_at_mut(&mut root, &path);
                    leaf.kind = Kind::Text { intrinsic: Size::new(70.0, 30.0) };
                    leaf.layout.set_content_id(fresh_id());
                }
                let (_, edited, edit_calls) = pass(&mut root, constraints);
                eprintln!(
                    "{kind:?} {label} depth {depth}: edit calls {edit_calls}, visited {}, stats {edited:?}",
                    edited.visited
                );
                assert!(edit_calls <= MAX_CALLS, "{kind:?} {label} depth {depth}: {edit_calls} calls after the edit");
                assert!(edited.visited <= visit_bound, "{kind:?} {label} depth {depth}: visited {}", edited.visited);
                assert_eq!(edited.nodes_cloned, 0, "the host's path copy already made the path unique");

                // Incremental equals cold.
                let mut fresh = chain(kind, depth);
                {
                    let leaf = node_at_mut(&mut fresh, &path);
                    leaf.kind = Kind::Text { intrinsic: Size::new(70.0, 30.0) };
                }
                pass(&mut fresh, constraints);
                assert_eq!(frames(&root), frames(&fresh), "{kind:?} {label} depth {depth}: incremental equals cold");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// No allocation after warm-up
// ---------------------------------------------------------------------------------------------

/// A random tree of views and texts with mixed styles, remembering the path to every leaf.
fn random_tree(rng: &mut Rng, depth: u32, tag: &mut u32, paths: &mut Vec<Vec<usize>>, path: &mut Vec<usize>) -> Handle<Node> {
    *tag += 1;
    let t = *tag;
    let mut style = Style::web()
        .with_flex_direction(rng.pick(&[
            FlexDirection::Row,
            FlexDirection::Column,
            FlexDirection::RowReverse,
            FlexDirection::ColumnReverse,
        ]))
        .with_justify_content(rng.pick(&[
            JustifyContent::FlexStart,
            JustifyContent::Center,
            JustifyContent::SpaceBetween,
            JustifyContent::FlexEnd,
        ]))
        .with_align_items(rng.pick(&[AlignItems::Stretch, AlignItems::FlexStart, AlignItems::Center, AlignItems::FlexEnd]))
        .with_padding(Edges::all(Length::Px(rng.below(6) as f32)))
        .with_margin(Edges::all(Length::Px(rng.below(4) as f32)))
        .with_gap(Length::Px(rng.below(5) as f32), Length::Px(rng.below(5) as f32))
        .with_flex_grow(rng.below(3) as f32)
        .with_flex_shrink(rng.below(2) as f32)
        .with_position(Position::Relative);
    if rng.chance(25) {
        style = style.with_size(Length::Px(40.0 + rng.below(200) as f32), Length::Auto);
    }
    if rng.chance(15) {
        style = style.with_size(Length::Percent(0.5), Length::Auto);
    }
    if rng.chance(10) {
        style = style.with_max_size(Length::Px(60.0 + rng.below(100) as f32), Length::None);
    }
    if depth == 0 || rng.chance(30) {
        paths.push(path.clone());
        return text(t, style, 10.0 + rng.below(120) as f32, 8.0 + rng.below(20) as f32);
    }
    let n = 1 + rng.below(5) as usize;
    let mut kids = Vec::with_capacity(n);
    for i in 0..n {
        path.push(i);
        kids.push(random_tree(rng, depth - 1, tag, paths, path));
        path.pop();
    }
    view(t, style, kids)
}

fn count_nodes(node: &Handle<Node>) -> usize {
    1 + node.children.iter().map(|s| count_nodes(&s.node)).sum::<usize>()
}

/// A random tree of a few hundred nodes and the paths to its leaves.
fn big_random_tree(seed: u64) -> (Handle<Node>, Vec<Vec<usize>>) {
    let mut rng = Rng(seed);
    loop {
        let mut tag = 0;
        let mut paths = Vec::new();
        let root = random_tree(&mut rng, 6, &mut tag, &mut paths, &mut Vec::new());
        if count_nodes(&root) >= 200 {
            return (root, paths);
        }
    }
}

/// One pass over `root` with `scratch`, counting the allocations it makes.
fn counted_pass(root: &mut Handle<Node>, scratch: &mut Scratch<f32>, constraints: RootConstraints<f32>) -> (Stats, u32, usize) {
    let mut shaper = Shaper { calls: 0 };
    let mut cx = LayoutCx::new(scratch);
    let (result, allocations) = count_allocations(|| hooke::layout(root, constraints, &mut cx, &mut shaper));
    result.expect("not cancelled");
    (cx.stats, shaper.calls, allocations)
}

fn edit_leaf(root: &mut Handle<Node>, path: &[usize], width: f32) {
    let leaf = node_at_mut(root, path);
    leaf.kind = Kind::Text { intrinsic: Size::new(width, 13.0) };
    leaf.layout.set_content_id(fresh_id());
}

const SEEDS: [u64; 4] = [0x5eed, 7919, 104_729, 31_337];

#[test]
fn a_warmed_up_pass_allocates_nothing() {
    for seed in SEEDS {
        for (label, constraints) in constraint_sets() {
            let (mut root, paths) = big_random_tree(seed);
            let nodes = count_nodes(&root);
            let mut scratch = Scratch::new();
            let (_, cold_calls, warmup) = counted_pass(&mut root, &mut scratch, constraints);
            let (stats, calls, unchanged) = counted_pass(&mut root, &mut scratch, constraints);
            eprintln!("seed {seed} {label} ({nodes} nodes): warm-up {warmup} allocations and {cold_calls} calls, unchanged {unchanged}, calls {calls}, {stats:?}");
            assert_eq!(unchanged, 0, "seed {seed} {label}: an unchanged tree laid out again allocated ({:?})", allocation_sizes());
            assert_eq!(calls, 0);

            // A leaf edited through the host's path copy, outside the counted window.
            let path = paths[paths.len() / 2].clone();
            edit_leaf(&mut root, &path, 77.0);
            let (stats, calls, edited) = counted_pass(&mut root, &mut scratch, constraints);
            eprintln!("seed {seed} {label}: path-copied edit {edited} allocations, calls {calls}, {stats:?}");
            assert_eq!(stats.nodes_cloned, 0);
            assert_eq!(stats.lists_cloned, 0);
            assert_eq!(
                edited,
                0,
                "seed {seed} {label}: a pass over a path-copied edit allocated (sizes {:?})",
                allocation_sizes()
            );
        }
    }
}

#[test]
fn a_forked_pass_allocates_only_for_copy_on_write() {
    for seed in SEEDS {
        for (label, constraints) in constraint_sets() {
            let (mut base, paths) = big_random_tree(seed);
            let mut scratch = Scratch::new();
            counted_pass(&mut base, &mut scratch, constraints);
            counted_pass(&mut base, &mut scratch, constraints);
            let before = frames(&base);

            // Fork, edit a leaf in the fork, lay the fork out. Siblings of the edited path that
            // move are shared with the base, so the pass may clone lists (offsets) and nodes.
            let mut fork = base.clone();
            let path = paths[paths.len() / 2].clone();
            edit_leaf(&mut fork, &path, 141.0);
            let (stats, calls, allocations) = counted_pass(&mut fork, &mut scratch, constraints);
            let clones = (stats.nodes_cloned + stats.lists_cloned) as usize;
            eprintln!(
                "seed {seed} {label}: fork {allocations} allocations, nodes_cloned {}, lists_cloned {}, calls {calls}",
                stats.nodes_cloned, stats.lists_cloned
            );
            assert!(
                allocations <= clones,
                "seed {seed} {label}: {allocations} allocations for {} node and {} list clones",
                stats.nodes_cloned,
                stats.lists_cloned
            );
            assert_eq!(frames(&base), before, "seed {seed} {label}: the base is untouched");
        }
    }
}
