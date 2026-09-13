use std::alloc::{GlobalAlloc, Layout};

/// Mimalloc's ordinary entry points already provide word alignment. Avoid
/// its general aligned-allocation path for the engine's strings and vectors.
pub struct EngineAllocator;

// SAFETY: mimalloc blocks are at least word-aligned, including small blocks.
// Larger alignments use its aligned API. Both paths share mi_free, and the
// realloc variants preserve the corresponding allocation's alignment.
unsafe impl GlobalAlloc for EngineAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= std::mem::size_of::<usize>() {
            libmimalloc_sys::mi_malloc(layout.size()).cast()
        } else {
            libmimalloc_sys::mi_malloc_aligned(layout.size(), layout.align()).cast()
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= std::mem::size_of::<usize>() {
            libmimalloc_sys::mi_zalloc(layout.size()).cast()
        } else {
            libmimalloc_sys::mi_zalloc_aligned(layout.size(), layout.align()).cast()
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _: Layout) {
        libmimalloc_sys::mi_free(pointer.cast());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if layout.align() <= std::mem::size_of::<usize>() {
            libmimalloc_sys::mi_realloc(pointer.cast(), size).cast()
        } else {
            libmimalloc_sys::mi_realloc_aligned(pointer.cast(), size, layout.align()).cast()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocations_preserve_alignment_zeroing_and_reallocated_contents() {
        for alignment in [1, 2, 4, 8, 16, 32, 64, 4096] {
            for size in [1, 7, 16, 65, 1024, 65537] {
                let layout = Layout::from_size_align(size, alignment).unwrap();
                unsafe {
                    let pointer = EngineAllocator.alloc_zeroed(layout);
                    assert!(!pointer.is_null());
                    assert_eq!(pointer as usize % alignment, 0);
                    assert!(std::slice::from_raw_parts(pointer, size)
                        .iter()
                        .all(|&v| v == 0));
                    pointer.write_bytes(0xa5, size);
                    let grown = EngineAllocator.realloc(pointer, layout, size * 2);
                    assert!(!grown.is_null());
                    assert_eq!(grown as usize % alignment, 0);
                    assert!(std::slice::from_raw_parts(grown, size)
                        .iter()
                        .all(|&v| v == 0xa5));
                    let larger = Layout::from_size_align(size * 2, alignment).unwrap();
                    let shrunk = EngineAllocator.realloc(grown, larger, 1);
                    assert!(!shrunk.is_null());
                    assert_eq!(shrunk as usize % alignment, 0);
                    assert_eq!(*shrunk, 0xa5);
                    EngineAllocator.dealloc(shrunk, Layout::from_size_align(1, alignment).unwrap());
                    let ordinary = EngineAllocator.alloc(layout);
                    assert!(!ordinary.is_null());
                    assert_eq!(ordinary as usize % alignment, 0);
                    EngineAllocator.dealloc(ordinary, layout);
                }
            }
        }
    }
}
