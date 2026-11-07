use core::{ffi::{c_char, c_int, c_void}, ptr};

unsafe extern "C" {
    fn hull_quickjs_native_memchr(bytes: *const c_void, value: c_int, length: usize) -> *mut c_void;
    fn hull_quickjs_native_strchr(text: *const c_char, value: c_int) -> *mut c_char;
    fn hull_quickjs_native_strrchr(text: *const c_char, value: c_int) -> *mut c_char;
    fn hull_quickjs_native_strcmp(left: *const c_char, right: *const c_char) -> c_int;
}

#[test]
fn binary_search_honors_length_zero_and_every_unsigned_byte() {
    let bytes: Vec<u8> = (0..=255).collect();
    unsafe {
        assert!(hull_quickjs_native_memchr(ptr::null(), 0, 0).is_null());
        for (index, byte) in bytes.iter().enumerate() {
            assert!(hull_quickjs_native_memchr(bytes.as_ptr().cast(), i32::from(*byte), index).is_null());
            assert_eq!(hull_quickjs_native_memchr(bytes.as_ptr().cast(), i32::from(*byte), index + 1), bytes.as_ptr().add(index).cast_mut().cast());
        }
        assert_eq!(hull_quickjs_native_memchr(bytes.as_ptr().cast(), 511, bytes.len()), bytes.as_ptr().add(255).cast_mut().cast());
    }
}

#[test]
fn string_search_stops_at_nul_and_can_find_the_terminator() {
    let text = b"a\x80a\0a\x80\0";
    unsafe {
        let base = text.as_ptr().cast::<c_char>();
        assert_eq!(hull_quickjs_native_strchr(base, 128), base.add(1).cast_mut());
        assert_eq!(hull_quickjs_native_strchr(base, i32::from(b'a')), base.cast_mut());
        assert_eq!(hull_quickjs_native_strrchr(base, i32::from(b'a')), base.add(2).cast_mut());
        assert_eq!(hull_quickjs_native_strchr(base, 0), base.add(3).cast_mut());
        assert_eq!(hull_quickjs_native_strrchr(base, 0), base.add(3).cast_mut());
        assert!(hull_quickjs_native_strchr(base, i32::from(b'z')).is_null());
        assert!(hull_quickjs_native_strrchr(base, i32::from(b'z')).is_null());
        let empty = c"".as_ptr();
        assert_eq!(hull_quickjs_native_strchr(empty, 0), empty.cast_mut());
        assert_eq!(hull_quickjs_native_strrchr(empty, 0), empty.cast_mut());
    }
}

#[test]
fn comparison_is_unsigned_lexicographic_and_ignores_bytes_after_nul() {
    unsafe {
        assert!(hull_quickjs_native_strcmp(c"\x7f".as_ptr(), c"\x80".as_ptr()) < 0);
        assert!(hull_quickjs_native_strcmp(c"\xff".as_ptr(), c"\x80".as_ptr()) > 0);
        assert!(hull_quickjs_native_strcmp(c"a".as_ptr(), c"aa".as_ptr()) < 0);
        assert!(hull_quickjs_native_strcmp(c"aa".as_ptr(), c"a".as_ptr()) > 0);
        assert_eq!(hull_quickjs_native_strcmp(c"".as_ptr(), c"".as_ptr()), 0);
        assert_eq!(hull_quickjs_native_strcmp(b"same\0x\0".as_ptr().cast(), b"same\0y\0".as_ptr().cast()), 0);
    }
}
