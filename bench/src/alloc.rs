//! The counting global allocator for the memory workload.
//!
//! [`Counting`] wraps [`std::alloc::System`] and keeps three relaxed atomics: bytes currently
//! allocated, the number of allocations (a `realloc` counts as one) and the total bytes ever
//! requested. Install it in a binary with
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: hooke_bench::alloc::Counting = hooke_bench::alloc::Counting;
//! ```
//!
//! and read it with [`snapshot`]. Without it installed the counters stay at zero, which
//! [`installed`] detects.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};

static CURRENT: AtomicIsize = AtomicIsize::new(0);
static COUNT: AtomicU64 = AtomicU64::new(0);
static TOTAL: AtomicU64 = AtomicU64::new(0);

/// A [`System`] wrapper that counts.
pub struct Counting;

// SAFETY: every method forwards to `System` with the caller's arguments unchanged and only
// updates counters afterwards.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            CURRENT.fetch_add(layout.size() as isize, Ordering::Relaxed);
            COUNT.fetch_add(1, Ordering::Relaxed);
            TOTAL.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            CURRENT.fetch_add(layout.size() as isize, Ordering::Relaxed);
            COUNT.fetch_add(1, Ordering::Relaxed);
            TOTAL.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        CURRENT.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            CURRENT.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
            COUNT.fetch_add(1, Ordering::Relaxed);
            TOTAL.fetch_add(new_size as u64, Ordering::Relaxed);
        }
        p
    }
}

/// The counters at one instant.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Snapshot {
    /// Bytes currently allocated.
    pub current: isize,
    /// Allocations so far (a `realloc` counts as one).
    pub count: u64,
    /// Bytes requested so far.
    pub total: u64,
}

impl Snapshot {
    /// The change from `before` to `self`.
    pub fn since(self, before: Snapshot) -> Snapshot {
        Snapshot {
            current: self.current - before.current,
            count: self.count - before.count,
            total: self.total - before.total,
        }
    }
}

/// Reads the counters.
pub fn snapshot() -> Snapshot {
    Snapshot {
        current: CURRENT.load(Ordering::Relaxed),
        count: COUNT.load(Ordering::Relaxed),
        total: TOTAL.load(Ordering::Relaxed),
    }
}

/// True if [`Counting`] is the global allocator of this binary.
pub fn installed() -> bool {
    let before = snapshot().count;
    let b = std::hint::black_box(Box::new(0u64));
    drop(b);
    snapshot().count > before
}
