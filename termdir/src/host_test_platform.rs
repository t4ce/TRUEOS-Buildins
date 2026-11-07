//! Native operations are outside the host UI model tests. Trap any accidental call.
//! Definitions let the linker retain unexecuted archive and filesystem branches.
macro_rules! unavailable {
    ($name:ident($($arg:ident : $ty:ty),*) -> $ret:ty) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: $ty),*) -> $ret {
            $(let _ = $arg;)*
            panic!(concat!("host UI test unexpectedly called native operation: ", stringify!($name)))
        }
    };
}
unavailable!(trueos_cabi_async_fs_list_mounts_start() -> i32);
unavailable!(trueos_cabi_async_fs_status(id: u32) -> i32);
unavailable!(trueos_cabi_async_fs_result_len(id: u32) -> isize);
unavailable!(trueos_cabi_async_fs_result_read(id: u32, offset: usize, out: *mut u8, capacity: usize) -> isize);
unavailable!(trueos_cabi_async_fs_discard(id: u32) -> i32);
unavailable!(trueos_cabi_async_fs_stat_start(path: *const u8, len: usize) -> i32);
unavailable!(trueos_cabi_async_fs_typed_list_dir_start(path: *const u8, len: usize) -> i32);
unavailable!(trueos_cabi_archive_discard(id: u32) -> i32);
unavailable!(trueos_cabi_archive_pack_start(source: *const u8, source_len: usize, target: *const u8, target_len: usize) -> i32);
unavailable!(trueos_cabi_archive_unpack_start(source: *const u8, source_len: usize, target: *const u8, target_len: usize) -> i32);
unavailable!(trueos_cabi_archive_status(id: u32) -> i32);
unavailable!(trueos_cabi_archive_report(id: u32, out: *mut u8) -> i32);
unavailable!(trueos_cabi_poll_once() -> ());
