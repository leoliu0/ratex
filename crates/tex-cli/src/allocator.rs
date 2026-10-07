//! Global allocator of the command-line tools: dlmalloc behind one lock for
//! large blocks, and a lock-free per-thread cache for small ones.
//!
//! The engine allocates and frees millions of small token lists, macro bodies
//! and frames. A locked allocator spends more time in the lock than in the
//! allocator, so blocks of up to `MAX_SMALL` bytes come from per-thread size
//! classes (16-byte steps). Each class keeps a free list; an empty list is
//! refilled by carving the thread's current chunk, itself taken from dlmalloc
//! in `CHUNK` pieces. A block freed by another thread than the allocating one
//! joins the freeing thread's list: every block of a class has the same size
//! and alignment, so it serves any later request of that class. Small blocks
//! are never returned to dlmalloc.

use dlmalloc::Dlmalloc;
use std::alloc::{GlobalAlloc, Layout};
use std::cell::UnsafeCell;
use std::ptr;
use std::sync::Mutex;

const GRAIN: usize = 16;
const MAX_SMALL: usize = 1024;
const CLASSES: usize = MAX_SMALL / GRAIN;
const CHUNK: usize = 128 * 1024;

static LARGE: Mutex<Dlmalloc> = Mutex::new(Dlmalloc::new());

struct Cache {
    /// Head of the free list of each size class (class `c` holds blocks of
    /// `(c + 1) * GRAIN` bytes); a free block stores the next one in its
    /// first word.
    free: [*mut u8; CLASSES],
    bump: *mut u8,
    end: *mut u8,
}

thread_local! {
    static CACHE: UnsafeCell<Cache> = const {
        UnsafeCell::new(Cache {
            free: [ptr::null_mut(); CLASSES],
            bump: ptr::null_mut(),
            end: ptr::null_mut(),
        })
    };
}

pub struct EngineAllocator;

/// Size class of a small layout, or `None` for the locked large path.
#[inline(always)]
fn small_class(layout: &Layout) -> Option<usize> {
    if layout.size() <= MAX_SMALL && layout.align() <= GRAIN {
        Some(layout.size().saturating_sub(1) / GRAIN)
    } else {
        None
    }
}

#[inline(always)]
fn large() -> std::sync::MutexGuard<'static, Dlmalloc> {
    // A poisoned lock only means another thread panicked inside dlmalloc;
    // the process aborts on panic, so it cannot be observed.
    LARGE.lock().unwrap_or_else(|e| e.into_inner())
}

#[cold]
#[inline(never)]
unsafe fn refill(cache: &mut Cache, block: usize) -> *mut u8 {
    let chunk = large().malloc(CHUNK, GRAIN);
    if chunk.is_null() {
        return chunk;
    }
    cache.bump = chunk.add(block);
    cache.end = chunk.add(CHUNK);
    chunk
}

#[inline(always)]
unsafe fn alloc_small(class: usize) -> *mut u8 {
    CACHE.with(|cell| {
        let cache = &mut *cell.get();
        let head = cache.free[class];
        if !head.is_null() {
            cache.free[class] = *(head as *mut *mut u8);
            return head;
        }
        let block = (class + 1) * GRAIN;
        if (cache.end as usize) - (cache.bump as usize) >= block {
            let p = cache.bump;
            cache.bump = p.add(block);
            return p;
        }
        refill(cache, block)
    })
}

#[inline(always)]
unsafe fn free_small(p: *mut u8, class: usize) {
    CACHE.with(|cell| {
        let cache = &mut *cell.get();
        *(p as *mut *mut u8) = cache.free[class];
        cache.free[class] = p;
    })
}

unsafe impl GlobalAlloc for EngineAllocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match small_class(&layout) {
            Some(class) => alloc_small(class),
            None => large().malloc(layout.size(), layout.align()),
        }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        match small_class(&layout) {
            Some(class) => free_small(p, class),
            None => large().free(p, layout.size(), layout.align()),
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        match small_class(&layout) {
            Some(class) => {
                let p = alloc_small(class);
                if !p.is_null() {
                    ptr::write_bytes(p, 0, layout.size());
                }
                p
            }
            None => large().calloc(layout.size(), layout.align()),
        }
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        match (small_class(&layout), small_class(&new_layout)) {
            (None, None) => large().realloc(p, layout.size(), layout.align(), new_size),
            (Some(old), Some(new)) if old == new => p,
            _ => {
                let q = self.alloc(new_layout);
                if !q.is_null() {
                    ptr::copy_nonoverlapping(p, q, layout.size().min(new_size));
                    self.dealloc(p, layout);
                }
                q
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn blocks_of_every_class_keep_their_contents_across_realloc() {
        let a = EngineAllocator;
        unsafe {
            let mut size = 1;
            let mut p = a.alloc(layout(size, 8));
            *p = 0xA5;
            while size < 4 * MAX_SMALL {
                let grown = size + size / 2 + 1;
                p = a.realloc(p, layout(size, 8), grown);
                assert!(!p.is_null());
                assert_eq!(*p, 0xA5);
                assert_eq!(p as usize % 8, 0);
                p.add(grown - 1).write(0x5A);
                size = grown;
            }
            while size > 1 {
                let shrunk = size / 2;
                p = a.realloc(p, layout(size, 8), shrunk);
                assert_eq!(*p, 0xA5);
                size = shrunk;
            }
            a.dealloc(p, layout(size, 8));
        }
    }

    #[test]
    fn small_blocks_are_distinct_aligned_and_zeroed_on_request() {
        let a = EngineAllocator;
        unsafe {
            let dirty = a.alloc(layout(48, 16));
            dirty.write_bytes(0xFF, 48);
            a.dealloc(dirty, layout(48, 16));
            // The freed block is the next one of its class.
            let zero = a.alloc_zeroed(layout(48, 16));
            assert_eq!(zero, dirty);
            assert!((0..48).all(|i| *zero.add(i) == 0));
            let other = a.alloc(layout(48, 16));
            assert_ne!(other, zero);
            assert_eq!(other as usize % 16, 0);
            a.dealloc(other, layout(48, 16));
            a.dealloc(zero, layout(48, 16));
            // Over-aligned and large requests take the locked path.
            let wide = a.alloc(layout(64, 64));
            assert_eq!(wide as usize % 64, 0);
            a.dealloc(wide, layout(64, 64));
            let big = a.alloc_zeroed(layout(3 * MAX_SMALL, 8));
            assert!((0..3 * MAX_SMALL).all(|i| *big.add(i) == 0));
            a.dealloc(big, layout(3 * MAX_SMALL, 8));
        }
    }

    #[test]
    fn a_block_freed_by_another_thread_is_reusable() {
        let a = EngineAllocator;
        let p = unsafe { a.alloc(layout(200, 8)) } as usize;
        std::thread::spawn(move || unsafe {
            EngineAllocator.dealloc(p as *mut u8, layout(200, 8));
            let q = EngineAllocator.alloc(layout(200, 8));
            assert_eq!(q as usize, p);
            EngineAllocator.dealloc(q, layout(200, 8));
        })
        .join()
        .unwrap();
    }
}
