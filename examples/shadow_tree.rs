//! A Fabric-like host embedding: a persistent shadow tree that is forked per update, laid out
//! off the main thread, committed by compare-and-swap and mounted by a pointer-equality diff.
//!
//! Run with `cargo run --example shadow_tree`.

use hooke::{
    Available, Cancelled, Children, Edges, FlexDirection, Handle, LayoutCx, LayoutNode, LayoutState, Length, Measure,
    MeasureInput, Measured, Parts, PartsMut, Point, RootConstraints, Scratch, SharedMeasureCache, Size, Stats, Style,
    snap,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ---- The host's node --------------------------------------------------------------------------
/// Identity across revisions (Fabric's `ShadowNodeFamily`), shared by every clone of a node.
struct Family {
    tag: u32,
    name: &'static str,
}

/// Immutable, versioned props. A style change is a new `Arc<Props>` with a new version.
struct Props {
    style: Style,
    version: u64,
}

#[derive(Clone)]
enum Kind {
    View,
    Text(Arc<str>),
}

/// `Clone` is shallow (handle bumps plus the inline layout state) and is the clone callback.
#[derive(Clone)]
struct ShadowNode {
    family: Arc<Family>,
    props: Arc<Props>,
    kind: Kind,
    children: Children<ShadowNode>, // engine type: child handles plus their offsets
    layout: LayoutState,            // engine type: size, memo and measurement slots
}

impl LayoutNode for ShadowNode {
    type Scalar = f32;
    fn parts(&self) -> Parts<'_, Self> {
        Parts {
            style: &self.props.style,
            state: &self.layout,
            children: &self.children,
        }
    }
    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut {
            style: &self.props.style,
            state: &mut self.layout,
            children: &mut self.children,
        }
    }
    fn style_key(&self) -> u64 {
        self.props.version // O(1): props are immutable and versioned
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);

fn node(name: &'static str, style: Style, kind: Kind, children: Vec<Handle<ShadowNode>>) -> Handle<ShadowNode> {
    let layout = match &kind {
        Kind::View => LayoutState::container(),
        Kind::Text(t) => LayoutState::measured(content_id(t)),
    };
    let family = Arc::new(Family {
        tag: NEXT.fetch_add(1, Ordering::Relaxed) as u32,
        name,
    });
    let props = Arc::new(Props {
        style,
        version: NEXT.fetch_add(1, Ordering::Relaxed),
    });
    Handle::new(ShadowNode {
        family,
        props,
        kind,
        children: Children::from_nodes(children),
        layout,
    })
}

fn text(name: &'static str, s: &str) -> Handle<ShadowNode> {
    node(name, Style::web(), Kind::Text(s.into()), vec![])
}

/// Names immutable text content (FNV-1a), so equal text hits the shared cache in any revision.
fn content_id(s: &str) -> u64 {
    s.bytes()
        .fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
        })
        .max(1)
}

// ---- Measuring text ---------------------------------------------------------------------------
const CHAR_W: f32 = 6.5;
const LINE_H: f32 = 15.5;

/// A toy shaper: fixed-advance glyphs, greedy word wrap. Returns (widest line, line count).
fn shape(text: &str, max_width: Option<f32>) -> (f32, u32) {
    let (mut widest, mut line, mut lines) = (0.0f32, 0.0f32, 1);
    for word in text.split(' ') {
        let w = word.chars().count() as f32 * CHAR_W;
        let joined = if line > 0.0 { line + CHAR_W + w } else { w };
        if line > 0.0 && max_width.is_some_and(|m| joined > m) {
            (widest, line, lines) = (widest.max(line), w, lines + 1);
        } else {
            line = joined;
        }
    }
    (widest.max(line), lines)
}

/// One per pass (per thread). `interrupt` simulates a higher-priority update arriving mid-pass.
#[derive(Default)]
struct TextShaper<'a> {
    interrupt: Option<&'a AtomicBool>,
}

impl Measure<ShadowNode> for TextShaper<'_> {
    fn measure(&mut self, node: &mut ShadowNode, input: MeasureInput<f32>, _: &mut LayoutCx<'_, f32>) -> Measured<f32> {
        if let Some(flag) = self.interrupt {
            flag.store(true, Ordering::Relaxed); // the scheduler cancels this candidate
        }
        let Kind::Text(text) = &node.kind else {
            return Measured::default();
        };
        let max = input.known.width.get().or(match input.available.width {
            Available::Definite(w) => Some(w),
            Available::MinContent => Some(0.0),
            Available::MaxContent => None,
        });
        let (width, lines) = shape(text, max);
        let width = input.known.width.get().unwrap_or(width);
        Measured::size(width, input.known.height.get().unwrap_or(lines as f32 * LINE_H))
    }
}

// ---- Editing, laying out, committing ----------------------------------------------------------
/// The path copy every edit goes through: `make_mut` at every level. The borrow path is the
/// dirty path, so the engine re-examines exactly these nodes and answers the rest from memos.
fn node_at_mut<'a>(root: &'a mut Handle<ShadowNode>, path: &[usize]) -> &'a mut ShadowNode {
    let mut node = Handle::make_mut(root); // clones the root if shared
    for &i in path {
        node = node.children.make_mut()[i].node_mut(); // clones list and child if shared; marks touched
    }
    node
}

fn set_text(root: &mut Handle<ShadowNode>, path: &[usize], s: &str) {
    let leaf = node_at_mut(root, path);
    leaf.kind = Kind::Text(s.into());
    leaf.layout.set_content_id(content_id(s)); // new content: drop the leaf's cached answers
}

type PassResult = (Result<Size<f32>, Cancelled>, Stats);

fn pass(
    root: &mut Handle<ShadowNode>,
    cache: &SharedMeasureCache<f32>,
    cancel: &AtomicBool,
    mut shaper: TextShaper<'_>,
) -> PassResult {
    let mut scratch = Scratch::new(); // a real host pools one per thread
    let mut cx = LayoutCx::new(&mut scratch).with_cache(cache).with_cancel(cancel);
    let result = hooke::layout(root, RootConstraints::viewport(360.0, 640.0), &mut cx, &mut shaper);
    (result, cx.stats)
}

struct Revision {
    number: u64,
    root: Handle<ShadowNode>,
}

struct ShadowTree {
    current: Mutex<Arc<Revision>>,
}

impl ShadowTree {
    fn current(&self) -> Arc<Revision> {
        self.current.lock().unwrap().clone()
    }
    /// Publishes `root` if the tree is still at `base`, else returns the revision to rebase onto.
    /// Layout happens outside the lock; the lock guards only the pointer swap.
    fn try_commit(&self, base: &Revision, root: Handle<ShadowNode>) -> Result<Arc<Revision>, Arc<Revision>> {
        let mut current = self.current.lock().unwrap();
        if current.number != base.number {
            return Err(current.clone());
        }
        *current = Arc::new(Revision {
            number: base.number + 1,
            root,
        });
        Ok(current.clone())
    }
}

// ---- Mounting ---------------------------------------------------------------------------------
const SCALE: f32 = 3.0; // device pixels per layout unit

/// Diffs `new` against the mounted node at the same position. Pointer equality means same size
/// and same descendants, so a shared subtree is skipped, or moved with one frame update. Frames
/// are snapped in absolute space and emitted parent-relative, in device pixels. (A shared subtree
/// moved by a fraction of a pixel could re-snap its descendants; a host that cares re-snaps it.)
fn diff(
    old: Option<(&Handle<ShadowNode>, Point<f32>)>,
    new: &Handle<ShadowNode>,
    offset: Point<f32>,
    origin: Point<f32>,
    depth: usize,
) {
    let old = old.filter(|(o, _)| o.family.tag == new.family.tag); // identity is the family
    let px = |v: f32| (v * SCALE).round() as i32;
    let f = snap(origin, offset, new.layout.size(), SCALE);
    let frame = format!(
        "({}, {}) {}x{}",
        px(f.x - origin.x),
        px(f.y - origin.y),
        px(f.width),
        px(f.height)
    );
    let (pad, name) = ("  ".repeat(depth + 1), new.family.name);
    if let Some((o, old_offset)) = old
        && Handle::ptr_eq(o, new)
    {
        if old_offset != offset {
            println!("{pad}move   {name:<7} -> {frame}  (shared subtree: one frame update)");
        } else {
            println!("{pad}skip   {name:<7}  (pointer-equal)");
        }
        return;
    }
    println!(
        "{pad}{} {name:<7} -> {frame}",
        if old.is_some() { "update" } else { "create" }
    );
    let old_children = old.map(|(o, _)| &o.children);
    if old_children.is_some_and(|c| c.ptr_eq(&new.children)) {
        return; // same list allocation: same children, same sizes, same offsets
    }
    for (i, slot) in new.children.iter().enumerate() {
        let old_child = old_children.and_then(|c| c.get(i)).map(|s| (&s.node, s.offset));
        diff(old_child, &slot.node, slot.offset, f.origin(), depth + 1);
    }
}

// ---- Reporting --------------------------------------------------------------------------------
fn show((result, s): &PassResult) -> String {
    let size = result
        .map(|s| format!("root {}x{}", s.width, s.height))
        .unwrap_or_else(|e| format!("{e:?}"));
    format!(
        "{size}; visited {}, memo hits {}, measured {}, shared-cache hits {}, nodes cloned {}, lists cloned {}",
        s.visited, s.memo_hits, s.measured, s.shared_hits, s.nodes_cloned, s.lists_cloned
    )
}

fn print_tree(node: &Handle<ShadowNode>, offset: Point<f32>, depth: usize) {
    let (s, pad) = (node.layout.size(), "  ".repeat(depth + 1));
    let label = if let Kind::Text(t) = &node.kind {
        format!("  {t:?}")
    } else {
        String::new()
    };
    println!(
        "{pad}{:<7} @({}, {}) {}x{}{label}",
        node.family.name, offset.x, offset.y, s.width, s.height
    );
    node.children
        .iter()
        .for_each(|slot| print_tree(&slot.node, slot.offset, depth + 1));
}

/// Sorts `new` into nodes that are copies (`out[0]`) and maximal subtrees shared with `old` (`out[1]`).
fn sharing(old: &Handle<ShadowNode>, new: &Handle<ShadowNode>, out: &mut [Vec<&'static str>; 2]) {
    let shared = Handle::ptr_eq(old, new);
    out[shared as usize].push(new.family.name);
    if !shared {
        old.children
            .iter()
            .zip(new.children.iter())
            .for_each(|(o, n)| sharing(&o.node, &n.node, out));
    }
}

fn print_sharing(old: &Handle<ShadowNode>, new: &Handle<ShadowNode>) {
    let mut out = [vec![], vec![]];
    sharing(old, new, &mut out);
    println!("  copied: {:?}, still shared: {:?}", out[0], out[1]);
}

fn main() {
    let column = Style::web().with_flex_direction(FlexDirection::Column);
    let gap = |v| Style::web().with_gap(Length::Px(v), Length::Px(v));
    let header = node(
        "header",
        gap(8.0),
        Kind::View,
        vec![
            node(
                "title",
                Style::web().with_flex_grow(1.0),
                Kind::Text("Inbox".into()),
                vec![],
            ),
            text("badge", "3"),
        ],
    );
    let list = node(
        "list",
        column.with_gap(Length::Px(4.0), Length::Px(4.0)),
        Kind::View,
        vec![
            text("msg1", "Ada: lunch at noon?"),
            text("msg2", "Grace: the build is green"),
        ],
    );
    let footer = node("footer", Style::web(), Kind::View, vec![text("compose", "Compose")]);
    let screen = column
        .with_size(Length::Px(336.0), Length::Auto)
        .with_padding(Edges::all(Length::Px(12.0)));
    let mut root = node(
        "screen",
        screen.with_gap(Length::Px(8.0), Length::Px(8.0)),
        Kind::View,
        vec![header, list, footer],
    );

    let cache = SharedMeasureCache::with_capacity(1024); // shared by every pass on every thread
    let never = AtomicBool::new(false);
    println!(
        "revision 0: {}",
        show(&pass(&mut root, &cache, &never, TextShaper::default()))
    );
    print_tree(&root, Point::ZERO, 0);
    let tree = ShadowTree {
        current: Mutex::new(Arc::new(Revision { number: 0, root })),
    };
    let mounted = tree.current(); // the UI thread has mounted revision 0

    println!("\n(a) fork revision 0 with root.clone() and path-copy msg1 at [1, 0]");
    let base = tree.current();
    let mut candidate = base.root.clone(); // O(1): one refcount bump
    set_text(
        &mut candidate,
        &[1, 0],
        "Ada: are we still on for lunch? The new place on Market opens at noon.",
    );
    print_sharing(&base.root, &candidate);

    println!("\n(b) lay out the candidate");
    let result = pass(&mut candidate, &cache, &never, TextShaper::default());
    println!("  {}", show(&result));
    assert_eq!(
        (result.1.nodes_cloned, result.1.lists_cloned),
        (0, 0),
        "the path copy made the path unique"
    );
    print_sharing(&base.root, &candidate); // unchanged: msg2 and footer moved, but only their offsets changed
    let footer_shared = Handle::ptr_eq(&base.root.children[2].node, &candidate.children[2].node);
    let (y0, y1) = (base.root.children[2].offset.y, candidate.children[2].offset.y);
    println!("  footer offset y {y0} -> {y1}, written into the root's list; footer node shared: {footer_shared}");
    print_tree(&candidate, Point::ZERO, 0);
    let Ok(rev) = tree.try_commit(&base, candidate) else {
        unreachable!("nobody else committed")
    };
    println!("  committed revision {}", rev.number);

    println!(
        "\n(c) two candidates forked from revision {} on two threads, one shared measure cache",
        rev.number
    );
    let base = tree.current();
    let (mut x, mut y) = (base.root.clone(), base.root.clone());
    set_text(&mut x, &[0, 1], "12"); // X: the badge
    set_text(&mut y, &[2, 0], "New message"); // Y: the compose button
    let (rx, ry) = std::thread::scope(|s| {
        let hx = s.spawn(|| pass(&mut x, &cache, &never, TextShaper::default()));
        let hy = s.spawn(|| pass(&mut y, &cache, &never, TextShaper::default()));
        (hx.join().unwrap(), hy.join().unwrap())
    });
    // X clones one node during layout: the title, shared with revision 1, whose width the wider
    // badge squeezed. Writing its new size needs a private copy.
    println!(
        "  X badge \"12\":          {}\n  Y \"New message\":       {}",
        show(&rx),
        show(&ry)
    );
    // Committed in a fixed order so the output is stable; normally the first to arrive wins.
    let Ok(won) = tree.try_commit(&base, x) else {
        unreachable!("X commits first")
    };
    println!("  X committed revision {}", won.number);
    let Err(newer) = tree.try_commit(&base, y) else {
        unreachable!("Y is stale")
    };
    println!(
        "  Y rejected: the tree is at revision {}, Y was forked from {}",
        newer.number, base.number
    );
    let mut y = newer.root.clone(); // rebase: replay Y's edit onto the winner
    set_text(&mut y, &[2, 0], "New message");
    let result = pass(&mut y, &cache, &never, TextShaper::default());
    println!("  Y rebased:             {}", show(&result));
    assert_eq!(result.1.measured, 0, "Y's shaping came back from the shared cache");
    let Ok(rev) = tree.try_commit(&newer, y) else {
        unreachable!("nothing committed since")
    };
    println!("  Y committed revision {}", rev.number);

    println!(
        "\n(d) mount: diff mounted revision {} against revision {} at {SCALE}x",
        mounted.number, rev.number
    );
    diff(
        Some((&mounted.root, Point::ZERO)),
        &rev.root,
        Point::ZERO,
        Point::ZERO,
        0,
    );
    println!("  and revision {} against revision {}:", won.number, rev.number);
    diff(Some((&won.root, Point::ZERO)), &rev.root, Point::ZERO, Point::ZERO, 0);

    println!("\n(e) cancellation");
    let base = tree.current();
    let mut candidate = base.root.clone();
    set_text(&mut candidate, &[1, 1], "Grace: the build is green again");
    let flag = AtomicBool::new(false);
    let result = pass(&mut candidate, &cache, &flag, TextShaper { interrupt: Some(&flag) });
    println!("  flag set during the first measurement: {}", show(&result));
    println!(
        "  revision {} still current and untouched: {}",
        base.number,
        Handle::ptr_eq(&tree.current().root, &base.root)
    );
    flag.store(false, Ordering::Relaxed);
    println!(
        "  retried, keeping the work done before the cancel: {}",
        show(&pass(&mut candidate, &cache, &flag, TextShaper::default()))
    );
}
