//! QuickJS-owned allocations use Rust's allocator, with their exact layout kept
//! beside the payload. C's layout-free deallocation stays local to this engine;
//! it never needs the kernel's global C pointer registry.
use alloc::alloc::{alloc, alloc_zeroed, dealloc, realloc};
use core::{alloc::Layout, ffi::c_void, mem::size_of, ptr};

// QuickJS's 16-byte full-pointer JSValue and long-double C ABI require this
// alignment. The header size is also the engine's MALLOC_OVERHEAD in quickjs.c.
#[repr(C, align(16))]
struct Header {
    requested: usize,
}
const PREFIX: usize = size_of::<Header>();
const _: () = assert!(PREFIX == 16);

fn layout(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.max(1).checked_add(PREFIX)?, 16).ok()
}

unsafe fn allocate(size: usize, zeroed: bool) -> *mut c_void {
    let Some(layout) = layout(size) else {
        return ptr::null_mut();
    };
    let base = unsafe {
        if zeroed {
            alloc_zeroed(layout)
        } else {
            alloc(layout)
        }
    };
    if base.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        base.cast::<Header>().write(Header { requested: size });
        base.add(PREFIX).cast()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hull_quickjs_native_alloc(size: usize) -> *mut c_void {
    unsafe { allocate(size, false) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hull_quickjs_native_calloc(count: usize, size: usize) -> *mut c_void {
    let Some(size) = count.checked_mul(size) else {
        return ptr::null_mut();
    };
    unsafe { allocate(size, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hull_quickjs_native_free(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }
    let base = unsafe { pointer.cast::<u8>().sub(PREFIX) };
    let size = unsafe { (*base.cast::<Header>()).requested };
    // SAFETY: only live allocations from this module cross this private C API.
    unsafe { dealloc(base, layout(size).unwrap_unchecked()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hull_quickjs_native_realloc(
    pointer: *mut c_void,
    size: usize,
) -> *mut c_void {
    if pointer.is_null() {
        return unsafe { allocate(size, false) };
    }
    if size == 0 {
        unsafe { hull_quickjs_native_free(pointer) };
        return ptr::null_mut();
    }
    let Some(new_layout) = layout(size) else {
        return ptr::null_mut();
    };
    let base = unsafe { pointer.cast::<u8>().sub(PREFIX) };
    let old_size = unsafe { (*base.cast::<Header>()).requested };
    let new_base = unsafe { realloc(base, layout(old_size).unwrap_unchecked(), new_layout.size()) };
    if new_base.is_null() {
        // realloc failure leaves both the old bytes and their header owned by C.
        return ptr::null_mut();
    }
    unsafe {
        new_base.cast::<Header>().write(Header { requested: size });
        new_base.add(PREFIX).cast()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hull_quickjs_native_alloc_size(pointer: *mut c_void) -> usize {
    if pointer.is_null() {
        return 0;
    }
    unsafe { (*pointer.cast::<u8>().sub(PREFIX).cast::<Header>()).requested }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_align_and_keep_the_requested_size() {
        for size in [0, 1, 3, 15, 16, 17, 8193, 1024 * 1024] {
            unsafe {
                let pointer = hull_quickjs_native_alloc(size);
                assert!(!pointer.is_null());
                assert_eq!((pointer as usize) % 16, 0);
                assert_eq!(hull_quickjs_native_alloc_size(pointer), size);
                if size != 0 {
                    pointer.cast::<u8>().write_bytes(0x5a, size);
                }
                hull_quickjs_native_free(pointer);
            }
        }
    }

    #[test]
    fn calloc_is_zero_and_rejects_overflow() {
        unsafe {
            let pointer = hull_quickjs_native_calloc(7, 33);
            assert!(!pointer.is_null());
            assert_eq!(hull_quickjs_native_alloc_size(pointer), 231);
            assert!(
                core::slice::from_raw_parts(pointer.cast::<u8>(), 231)
                    .iter()
                    .all(|b| *b == 0)
            );
            hull_quickjs_native_free(pointer);
            assert!(hull_quickjs_native_calloc(usize::MAX, 2).is_null());
            assert!(hull_quickjs_native_alloc(usize::MAX).is_null());
            assert!(hull_quickjs_native_alloc(isize::MAX as usize).is_null());
        }
    }

    #[test]
    fn realloc_preserves_payload_and_failure_ownership() {
        unsafe {
            let mut pointer = hull_quickjs_native_realloc(ptr::null_mut(), 101);
            assert!(!pointer.is_null());
            for i in 0..101 {
                pointer.cast::<u8>().add(i).write(i as u8);
            }
            assert!(hull_quickjs_native_realloc(pointer, usize::MAX).is_null());
            assert_eq!(hull_quickjs_native_alloc_size(pointer), 101);
            for size in [4097, 41, 8193] {
                pointer = hull_quickjs_native_realloc(pointer, size);
                assert!(!pointer.is_null());
                assert_eq!((pointer as usize) % 16, 0);
                assert_eq!(hull_quickjs_native_alloc_size(pointer), size);
                for i in 0..41 {
                    assert_eq!(pointer.cast::<u8>().add(i).read(), i as u8);
                }
            }
            assert!(hull_quickjs_native_realloc(pointer, 0).is_null());
            hull_quickjs_native_free(ptr::null_mut());
            assert_eq!(hull_quickjs_native_alloc_size(ptr::null_mut()), 0);
        }
    }
}
