//! The persistence contract: copy-on-write, invalidation, racing candidates and the oracles
//! nobody else has (incremental equals cold, concurrent equals sequential, base untouched).

mod support;

use hooke::{
    AlignItems, Children, Direction, Handle, JustifyContent, LayoutCx, Length, Position, RootConstraints, Scratch,
    SharedMeasureCache, Size, Style,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use support::*;

fn row() -> Style {
    Style::web()
}

/// root(row) -> [A(row) -> [t1, t2], B(row) -> [t3], P(paragraph with attachment V)]
fn fixture() -> Handle<Node> {
    let t1 = text(11, row(), 20.0, 10.0);
    let t2 = text(12, row(), 30.0, 10.0);
    let t3 = text(13, row(), 40.0, 10.0);
    let a = view(1, row(), vec![t1, t2]);
    let b = view(2, row(), vec![t3]);
    let v = view(31, row().with_size(Length::Px(7.0), Length::Px(7.0)), vec![]);
    let p = paragraph(3, row(), vec![v]);
    view(0, row(), vec![a, b, p])
}

const VIEWPORT: Size<f32> = Size {
    width: 800.0,
    height: 600.0,
};

#[test]
fn base_layout_and_sizes() {
    let mut root = fixture();
    let (size, stats, calls) = run(&mut root, VIEWPORT, None);
    // A = 20+30, B = 40, P = 50 + V(7) = 57, all 10/12 tall, stretched to the tallest (12).
    // The root's width is the sum of its items: a single-line container with an auto main size
    // does not fill definite available space (Taffy's rule, which Chrome fixtures confirm).
    assert_eq!(size, Size::new(147.0, 12.0));
    assert_eq!(root.children[0].node.layout.size(), Size::new(50.0, 12.0));
    assert_eq!(root.children[1].node.layout.size(), Size::new(40.0, 12.0));
    assert_eq!(root.children[2].node.layout.size(), Size::new(57.0, 12.0));
    assert_eq!(root.children[1].offset.x, 50.0);
    assert_eq!(root.children[2].offset.x, 90.0);
    assert_eq!(stats.nodes_cloned, 0, "a unique tree is never cloned");
    // Each text is measured once at max-content and once more when its cross size becomes
    // definite through stretching (as Yoga and Taffy do); the paragraph's answer is reused.
    assert_eq!(calls, 7);
    assert_eq!(
        core::mem::size_of::<hooke::LayoutState>(),
        64 + 32 * hooke::DEFAULT_SLOTS,
        "192 with four slots, 128 with two"
    );
    assert_eq!(core::mem::size_of::<Style>(), 144);
    assert_eq!(core::mem::size_of::<hooke::Slot<Node>>(), 16);
}

#[test]
fn single_edit_re_measures_one_leaf_and_clones_nothing_during_layout() {
    let mut base = fixture();
    run(&mut base, VIEWPORT, None);
    let before = frames(&base);

    let mut candidate = base.clone(); // O(1) fork
    {
        let t2 = node_at_mut(&mut candidate, &[0, 1]); // clones root, A, t2 and two lists
        t2.kind = Kind::Text {
            intrinsic: Size::new(60.0, 10.0),
        };
        t2.layout.set_content_id(fresh_id());
    }
    let (size, stats, calls) = run(&mut candidate, VIEWPORT, None);
    assert_eq!(size.width, 177.0);
    assert_eq!(candidate.children[0].node.layout.size().width, 80.0);
    assert_eq!(
        calls, 2,
        "only t2 is re-measured (max-content, then at its stretched height)"
    );
    assert_eq!(
        stats.nodes_cloned, 0,
        "the host's path copy already made the path unique"
    );
    assert_eq!(frames(&base), before, "the base revision is untouched");
    assert!(
        Handle::ptr_eq(&base.children[1].node, &candidate.children[1].node),
        "B shared"
    );
    assert!(
        Handle::ptr_eq(&base.children[2].node, &candidate.children[2].node),
        "P shared"
    );
    assert!(
        !Handle::ptr_eq(&base.children[0].node, &candidate.children[0].node),
        "A path-copied"
    );
    assert_eq!(candidate.children[1].offset.x, 80.0, "B moved in root's private list");
    assert!(
        !candidate.children.is_touched() && !candidate.children[0].node.children.is_touched(),
        "touched cleared"
    );
}

#[test]
fn a_shifted_sibling_stays_shared() {
    let mut base = fixture();
    run(&mut base, VIEWPORT, None);
    let mut candidate = base.clone();
    {
        let t1 = node_at_mut(&mut candidate, &[0, 0]);
        t1.kind = Kind::Text {
            intrinsic: Size::new(100.0, 10.0),
        };
        t1.layout.set_content_id(fresh_id());
    }
    run(&mut candidate, VIEWPORT, None);
    let a = &candidate.children[0].node;
    assert!(
        Handle::ptr_eq(&a.children[1].node, &base.children[0].node.children[1].node),
        "t2 shared although it moved"
    );
    assert_eq!(a.children[1].offset.x, 100.0);
    assert_eq!(base.children[0].node.children[1].offset.x, 20.0);
}

#[test]
fn style_change_is_detected_without_an_invalidate_call() {
    let mut base = fixture();
    run(&mut base, VIEWPORT, None);
    let mut candidate = base.clone();
    {
        let a = node_at_mut(&mut candidate, &[0]);
        a.props = Props::new(row().with_padding(hooke::Edges::all(Length::Px(5.0))));
    }
    run(&mut candidate, VIEWPORT, None);
    assert_eq!(candidate.children[0].node.layout.size(), Size::new(60.0, 20.0));
    assert_eq!(candidate.children[0].node.children[0].offset.x, 5.0);
}

#[test]
fn two_candidates_on_two_threads_leave_the_base_untouched_and_commit_by_cas() {
    let mut base = fixture();
    run(&mut base, VIEWPORT, None);
    let before = frames(&base);
    let cache = SharedMeasureCache::with_capacity(1024);

    let mut c1 = base.clone();
    let mut c2 = base.clone();
    let t3_content = fresh_id();
    {
        let t1 = node_at_mut(&mut c1, &[0, 0]);
        t1.kind = Kind::Text {
            intrinsic: Size::new(100.0, 10.0),
        };
        t1.layout.set_content_id(fresh_id());
    }
    {
        let t3 = node_at_mut(&mut c2, &[1, 0]);
        t3.kind = Kind::Text {
            intrinsic: Size::new(10.0, 10.0),
        };
        t3.layout.set_content_id(t3_content);
    }
    let (r1, r2) = std::thread::scope(|s| {
        let h1 = s.spawn(|| run(&mut c1, VIEWPORT, Some(&cache)));
        let h2 = s.spawn(|| run(&mut c2, VIEWPORT, Some(&cache)));
        (h1.join().unwrap(), h2.join().unwrap())
    });
    assert_eq!(frames(&base), before, "two concurrent passes never wrote the base");
    assert_eq!(r1.2, 2, "only t1, under its two constraints");
    assert_eq!(r2.2, 2, "only t3, under its two constraints");

    // Commit protocol: revision number compare-and-swap under a mutex, layout outside it.
    struct Revision {
        number: u64,
        root: Handle<Node>,
    }
    let tree = Mutex::new(Revision {
        number: 0,
        root: base.clone(),
    });
    let try_commit = |base_number: u64, root: Handle<Node>| -> Result<u64, u64> {
        let mut cur = tree.lock().unwrap();
        if cur.number != base_number {
            return Err(cur.number);
        }
        cur.number += 1;
        cur.root = root;
        Ok(cur.number)
    };
    let committed = try_commit(0, c1.clone()).expect("first commit succeeds");
    assert_eq!(
        try_commit(0, c2.clone()),
        Err(committed),
        "second commit rejected against a stale base"
    );

    // Rebase c2's edit onto the winner; its text measurement comes back from the shared cache.
    let mut rebased = tree.lock().unwrap().root.clone();
    {
        let t3 = node_at_mut(&mut rebased, &[1, 0]);
        t3.kind = Kind::Text {
            intrinsic: Size::new(10.0, 10.0),
        };
        t3.layout.set_content_id(t3_content);
    }
    let (_, stats, calls) = run(&mut rebased, VIEWPORT, Some(&cache));
    assert_eq!(
        calls, 0,
        "the losing candidate's measurements were donated through the shared cache"
    );
    assert_eq!(stats.shared_hits, 2);
    try_commit(committed, rebased).expect("rebased commit succeeds");
}

#[test]
fn cancellation_returns_err_and_leaves_the_base_alone() {
    let mut base = fixture();
    run(&mut base, VIEWPORT, None);
    let before = frames(&base);
    let mut candidate = base.clone();
    {
        let t2 = node_at_mut(&mut candidate, &[0, 1]);
        t2.kind = Kind::Text {
            intrinsic: Size::new(1.0, 1.0),
        };
        t2.layout.set_content_id(fresh_id());
    }
    let flag = AtomicBool::new(true);
    let mut scratch = Scratch::new();
    let mut cx = LayoutCx::new(&mut scratch).with_cancel(&flag);
    let mut shaper = Shaper { calls: 0 };
    let r = hooke::layout(
        &mut candidate,
        RootConstraints::viewport(800.0, 600.0),
        &mut cx,
        &mut shaper,
    );
    assert!(r.is_err());
    assert_eq!(frames(&base), before);
    flag.store(false, Ordering::Relaxed);
    let mut cx = LayoutCx::new(&mut scratch).with_cancel(&flag);
    assert!(
        hooke::layout(
            &mut candidate,
            RootConstraints::viewport(800.0, 600.0),
            &mut cx,
            &mut shaper
        )
        .is_ok()
    );
}

#[test]
fn hidden_subtrees_are_never_written() {
    let hidden_leaf = text(99, row(), 40.0, 40.0);
    let hidden = view(98, row().with_display(hooke::Display::None), vec![hidden_leaf]);
    let shown = text(1, row(), 10.0, 10.0);
    let mut root = view(0, row(), vec![hidden, shown]);
    let (size, _, calls) = run(&mut root, VIEWPORT, None);
    assert_eq!(size, Size::new(10.0, 10.0));
    assert_eq!(calls, 1, "the hidden text was not measured");
    assert_eq!(root.children[0].node.layout.size(), Size::ZERO);
    assert!(!root.children[0].node.layout.has_new_layout());
}

/// Builds a random tree of views and texts, remembering a path to a random leaf.
fn random_tree(
    rng: &mut Rng,
    depth: u32,
    tag: &mut u32,
    paths: &mut Vec<Vec<usize>>,
    path: &mut Vec<usize>,
) -> Handle<Node> {
    *tag += 1;
    let t = *tag;
    let mut style = Style::web()
        .with_flex_direction(rng.pick(&[
            hooke::FlexDirection::Row,
            hooke::FlexDirection::Column,
            hooke::FlexDirection::RowReverse,
        ]))
        .with_justify_content(rng.pick(&[
            JustifyContent::FlexStart,
            JustifyContent::Center,
            JustifyContent::SpaceBetween,
            JustifyContent::FlexEnd,
        ]))
        .with_align_items(rng.pick(&[AlignItems::Stretch, AlignItems::FlexStart, AlignItems::Center]))
        .with_padding(hooke::Edges::all(Length::Px(rng.below(6) as f32)))
        .with_gap(Length::Px(rng.below(5) as f32), Length::Px(rng.below(5) as f32))
        .with_flex_grow(rng.below(3) as f32)
        .with_position(Position::Relative);
    if rng.chance(30) {
        style = style.with_size(Length::Px(40.0 + rng.below(200) as f32), Length::Auto);
    }
    if depth == 0 || rng.chance(35) {
        paths.push(path.clone());
        return text(t, style, 10.0 + rng.below(80) as f32, 8.0 + rng.below(20) as f32);
    }
    let n = 1 + rng.below(4) as usize;
    let mut kids = Vec::with_capacity(n);
    for i in 0..n {
        path.push(i);
        kids.push(random_tree(rng, depth - 1, tag, paths, path));
        path.pop();
    }
    view(t, style, kids)
}

#[test]
fn incremental_equals_cold_after_random_edits() {
    for seed in 1..=40u64 {
        let mut rng = Rng(seed * 7919);
        let mut tag = 0;
        let mut paths = Vec::new();
        let mut root = random_tree(&mut rng, 4, &mut tag, &mut paths, &mut Vec::new());
        run(&mut root, VIEWPORT, None);
        for _ in 0..3 {
            let path = paths[rng.below(paths.len() as u32) as usize].clone();
            let (w, h) = (10.0 + rng.below(100) as f32, 8.0 + rng.below(30) as f32);
            {
                let leaf = node_at_mut(&mut root, &path);
                leaf.kind = Kind::Text {
                    intrinsic: Size::new(w, h),
                };
                leaf.layout.set_content_id(fresh_id());
            }
            run(&mut root, VIEWPORT, None);
        }
        let incremental = frames(&root);
        // Cold: rebuild the same tree from scratch by deep-copying structure with fresh state.
        let mut cold = deep_copy_fresh(&root);
        run(&mut cold, VIEWPORT, None);
        assert_eq!(
            incremental,
            frames(&cold),
            "seed {seed}: incremental layout must equal a cold layout"
        );
    }
}

fn deep_copy_fresh(node: &Handle<Node>) -> Handle<Node> {
    let children: Vec<_> = node.children.iter().map(|s| deep_copy_fresh(&s.node)).collect();
    let layout = match node.layout.kind() {
        hooke::NodeKind::Measured => hooke::LayoutState::measured(node.layout.content_id()),
        hooke::NodeKind::Container => hooke::LayoutState::container(),
        hooke::NodeKind::Contents => hooke::LayoutState::contents(),
    };
    Handle::new(Node {
        tag: node.tag,
        props: node.props.clone(),
        kind: node.kind.clone(),
        children: Children::from_nodes(children),
        layout,
    })
}

#[test]
fn concurrent_equals_sequential() {
    let mut rng = Rng(42);
    let mut tag = 0;
    let mut paths = Vec::new();
    let mut base = random_tree(&mut rng, 4, &mut tag, &mut paths, &mut Vec::new());
    run(&mut base, VIEWPORT, None);
    let base_frames = frames(&base);

    let edits: Vec<(Vec<usize>, f32)> = (0..4)
        .map(|i| (paths[i % paths.len()].clone(), 15.0 + 10.0 * i as f32))
        .collect();
    let mut sequential = Vec::new();
    for (path, w) in &edits {
        let mut c = base.clone();
        {
            let leaf = node_at_mut(&mut c, path);
            leaf.kind = Kind::Text {
                intrinsic: Size::new(*w, 10.0),
            };
            leaf.layout.set_content_id(fresh_id());
        }
        run(&mut c, VIEWPORT, None);
        sequential.push(frames(&c));
    }
    let concurrent: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = edits
            .iter()
            .map(|(path, w)| {
                let mut c = base.clone();
                let path = path.clone();
                let w = *w;
                s.spawn(move || {
                    {
                        let leaf = node_at_mut(&mut c, &path);
                        leaf.kind = Kind::Text {
                            intrinsic: Size::new(w, 10.0),
                        };
                        leaf.layout.set_content_id(fresh_id());
                    }
                    run(&mut c, VIEWPORT, None);
                    frames(&c)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(sequential, concurrent);
    assert_eq!(
        frames(&base),
        base_frames,
        "base byte-identical after four concurrent passes"
    );
}

#[test]
fn rtl_flips_the_row_and_is_part_of_the_cache_key() {
    let a = text(1, row(), 10.0, 10.0);
    let b = text(2, row(), 20.0, 10.0);
    let mut root = view(0, row().with_size(Length::Px(100.0), Length::Auto), vec![a, b]);
    let mut scratch = Scratch::new();
    let mut shaper = Shaper { calls: 0 };
    let mut cx = LayoutCx::new(&mut scratch);
    hooke::layout(&mut root, RootConstraints::viewport(800.0, 600.0), &mut cx, &mut shaper).unwrap();
    assert_eq!(root.children[0].offset.x, 0.0);
    assert_eq!(root.children[1].offset.x, 10.0);
    let rtl = RootConstraints::viewport(800.0, 600.0).with_direction(Direction::Rtl);
    let mut cx = LayoutCx::new(&mut scratch);
    hooke::layout(&mut root, rtl, &mut cx, &mut shaper).unwrap();
    assert_eq!(root.children[0].offset.x, 90.0, "first item at the inline end");
    assert_eq!(root.children[1].offset.x, 70.0);
}
