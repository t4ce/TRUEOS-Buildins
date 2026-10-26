// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(target_os = "trueos")]
use std::alloc::{Layout, alloc, dealloc};
#[cfg(not(target_os = "trueos"))]
use std::ffi::c_int;
use std::io;
use std::ptr::NonNull;
#[cfg(not(target_os = "trueos"))]
use std::ptr::{self, null_mut};

#[cfg(all(target_os = "trueos", target_pointer_width = "32"))]
const VIRTUAL_ALIGNMENT: usize = 32 * 1024;
#[cfg(all(target_os = "trueos", target_pointer_width = "64"))]
const VIRTUAL_ALIGNMENT: usize = 64 * 1024;

/// Reserves a virtual memory region of the given size.
/// To commit the memory, use `virtual_commit`.
/// To release the memory, use `virtual_release`.
///
/// # Safety
///
/// This function is unsafe because it uses raw pointers.
/// Don't forget to release the memory when you're done with it or you'll leak it.
#[cfg(not(target_os = "trueos"))]
pub unsafe fn virtual_reserve(size: usize) -> io::Result<NonNull<u8>> {
    unsafe {
        let ptr = libc::mmap(
            null_mut(),
            size,
            desired_mprotect(libc::PROT_READ | libc::PROT_WRITE),
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        if ptr.is_null() || ptr::eq(ptr, libc::MAP_FAILED) {
            Err(io::Error::last_os_error())
        } else {
            Ok(NonNull::new_unchecked(ptr.cast()))
        }
    }
}

#[cfg(target_os = "trueos")]
pub unsafe fn virtual_reserve(size: usize) -> io::Result<NonNull<u8>> {
    let layout = Layout::from_size_align(size, VIRTUAL_ALIGNMENT)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    NonNull::new(unsafe { alloc(layout) }).ok_or_else(|| io::Error::from_raw_os_error(libc::ENOMEM))
}

#[cfg(all(not(target_os = "trueos"), target_os = "netbsd"))]
const fn desired_mprotect(flags: c_int) -> c_int {
    // NetBSD allows an mmap(2) caller to specify what protection flags they
    // will use later via mprotect. It does not allow a caller to move from
    // PROT_NONE to PROT_READ | PROT_WRITE.
    //
    // see PROT_MPROTECT in man 2 mmap
    flags << 3
}

#[cfg(all(not(target_os = "trueos"), not(target_os = "netbsd")))]
const fn desired_mprotect(_: c_int) -> c_int {
    libc::PROT_NONE
}

/// Releases a virtual memory region of the given size.
///
/// # Safety
///
/// This function is unsafe because it uses raw pointers.
/// Make sure to only pass pointers acquired from `virtual_reserve`.
#[cfg(not(target_os = "trueos"))]
pub unsafe fn virtual_release(base: NonNull<u8>, size: usize) {
    unsafe {
        libc::munmap(base.cast().as_ptr(), size);
    }
}

#[cfg(target_os = "trueos")]
pub unsafe fn virtual_release(base: NonNull<u8>, size: usize) {
    if let Ok(layout) = Layout::from_size_align(size, VIRTUAL_ALIGNMENT) {
        unsafe { dealloc(base.as_ptr(), layout) };
    }
}

/// Commits a virtual memory region of the given size.
///
/// # Safety
///
/// This function is unsafe because it uses raw pointers.
/// Make sure to only pass pointers acquired from `virtual_reserve`
/// and to pass a size less than or equal to the size passed to `virtual_reserve`.
#[cfg(not(target_os = "trueos"))]
pub unsafe fn virtual_commit(base: NonNull<u8>, size: usize) -> io::Result<()> {
    unsafe {
        let status = libc::mprotect(base.cast().as_ptr(), size, libc::PROT_READ | libc::PROT_WRITE);
        if status != 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
}

#[cfg(target_os = "trueos")]
pub unsafe fn virtual_commit(_base: NonNull<u8>, _size: usize) -> io::Result<()> {
    Ok(())
}
