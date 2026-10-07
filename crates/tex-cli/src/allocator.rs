//! The engine's global allocator: dlmalloc behind a spin lock.
//!
//! `dlmalloc::GlobalDlmalloc` guards its single heap with a `pthread_mutex_t`,
//! whose lock and unlock calls were a visible share of a TeX run (the engine
//! allocates constantly, nearly always from one thread). The critical sections
//! here are a few dozen instructions, so an atomic flag is cheaper; a thread
//! that finds it taken backs off to the scheduler instead of spinning on.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, Ordering};
use dlmalloc::Dlmalloc;

static LOCKED: AtomicBool = AtomicBool::new(false);
static mut HEAP: Dlmalloc = Dlmalloc::new();

pub struct EngineAllocator;

/// Holds the heap lock while alive.
struct Guard;

impl Guard {
    #[inline]
    fn acquire() -> Guard {
        if LOCKED
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            Self::acquire_contended();
        }
        Guard
    }

    #[cold]
    #[inline(never)]
    fn acquire_contended() {
        let mut spins = 0u32;
        loop {
            while LOCKED.load(Ordering::Relaxed) {
                if spins < 64 {
                    spins += 1;
                    core::hint::spin_loop();
                } else {
                    std::thread::yield_now();
                }
            }
            if LOCKED
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }
}

impl Drop for Guard {
    #[inline]
    fn drop(&mut self) {
        LOCKED.store(false, Ordering::Release);
    }
}

// SAFETY: every access to `HEAP` happens while holding `LOCKED`, which gives
// the exclusive access `Dlmalloc` requires.
unsafe impl GlobalAlloc for EngineAllocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _guard = Guard::acquire();
        (*addr_of_mut!(HEAP)).malloc(layout.size(), layout.align())
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _guard = Guard::acquire();
        (*addr_of_mut!(HEAP)).free(ptr, layout.size(), layout.align())
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _guard = Guard::acquire();
        (*addr_of_mut!(HEAP)).calloc(layout.size(), layout.align())
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _guard = Guard::acquire();
        (*addr_of_mut!(HEAP)).realloc(ptr, layout.size(), layout.align(), new_size)
    }
}
