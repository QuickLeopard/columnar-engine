//! A counting global allocator.
//!
//! **It counts the bytes the process holds, and nothing else.** It does not
//! change what is allocated, and it does not change how much: the same layout
//! goes to the system allocator.
//!
//! **It is in an example, not in the engine.** Every crate here denies
//! `unsafe_code`, and a `GlobalAlloc` implementation needs `unsafe`. **An
//! example is its own crate**, so the counting allocator is the diagnostic's
//! and the engine's own memory behaviour is untouched by it.
//!
//! **And a time measured beside it is not the engine's time.** A counting
//! allocator counts **every** allocation, and that is not free -- so the
//! memory is measured with the allocator in it, and the time is measured
//! elsewhere, in a probe that has no allocator in it.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The bytes the process holds now.
pub(crate) static HELD: AtomicUsize = AtomicUsize::new(0);

/// The most bytes the process has held at once.
pub(crate) static PEAK: AtomicUsize = AtomicUsize::new(0);

/// The allocator that counts.
pub(crate) struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let held = HELD.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(held, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        HELD.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if !resized.is_null() {
            if new_size >= layout.size() {
                let grew = new_size - layout.size();
                let held = HELD.fetch_add(grew, Ordering::Relaxed) + grew;
                PEAK.fetch_max(held, Ordering::Relaxed);
            } else {
                HELD.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        resized
    }
}

impl Counting {
    /// The bytes the process holds now.
    #[must_use]
    pub(crate) fn held() -> usize {
        HELD.load(Ordering::Relaxed)
    }

    /// The most bytes the process has held at once.
    #[must_use]
    pub(crate) fn peak() -> usize {
        PEAK.load(Ordering::Relaxed)
    }

    /// Starts the peak again, from `baseline`.
    pub(crate) fn reset(baseline: usize) {
        PEAK.store(baseline, Ordering::Relaxed);
    }
}
