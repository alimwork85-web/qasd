//! The daemon's allocator: mimalloc. On macOS fsearch kept per-thread free
//! lists on top of malloc's space-efficient mode; mimalloc already has fast
//! thread-local heaps and returns idle memory to the OS, so `drain` only
//! asks it to collect now.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

pub struct Alloc;

static KEPT: AtomicBool = AtomicBool::new(false);

unsafe impl GlobalAlloc for Alloc {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { mimalloc::MiMalloc.alloc(l) }
    }
    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        unsafe { mimalloc::MiMalloc.alloc_zeroed(l) }
    }
    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { mimalloc::MiMalloc.dealloc(p, l) }
    }
    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        unsafe { mimalloc::MiMalloc.realloc(p, l, n) }
    }
}

/// A thread that searches (kept for API parity: mimalloc's heaps are
/// per-thread already). Marks that there is something to `drain` later.
pub fn keep_on_this_thread() {
    KEPT.store(true, Relaxed);
}

/// Give this thread's freed memory back to the OS.
pub fn drain() {
    unsafe { libmimalloc_sys::mi_collect(false) };
}

/// Whether some thread kept memory since the last call.
pub fn kept() -> bool {
    KEPT.swap(false, Relaxed)
}

/// Return freed memory process-wide (after big transient work). Each
/// thread's heap keeps the pages its allocations came from, even once
/// another thread freed them, until that thread collects: so every thread
/// of rayon's global pool (where the MFT records are parsed) collects too,
/// or ~600 MB stays committed after a build.
pub fn release() {
    unsafe { libmimalloc_sys::mi_collect(true) };
    rayon::broadcast(|_| unsafe { libmimalloc_sys::mi_collect(true) });
}
