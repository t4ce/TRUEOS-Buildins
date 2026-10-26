#![allow(dead_code, unused_imports)]
use core::mem::MaybeUninit;
#[derive(Debug)] pub struct Error;
mod util {
    use core::mem::MaybeUninit;
    pub fn uninit_slice_fill_zero(dest: &mut [MaybeUninit<u8>]) -> &mut [u8] {
        for byte in dest.iter_mut() { byte.write(0); }
        unsafe { core::slice::from_raw_parts_mut(dest.as_mut_ptr().cast(), dest.len()) }
    }
    pub fn inner_u32() {}
    pub fn inner_u64() {}
}
#[path="../vendor/getrandom-0.2.17/src/trueos.rs"] mod old;
#[path="../vendor/getrandom-0.3.4/src/backends/trueos.rs"] mod middle;
#[path="../vendor/getrandom-0.4.2/src/backends/trueos.rs"] mod newest;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
static WORD: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
#[unsafe(no_mangle)]
unsafe extern "C" fn sys_rand(out: *mut u32, count: usize) {
    assert!(!out.is_null()); assert!(count > 0 && count <= 64);
    assert_eq!((out as usize) % core::mem::align_of::<u32>(), 0);
    CALLS.fetch_add(1, SeqCst);
    for index in 0..count {
        let word = WORD.fetch_add(1, SeqCst) as u32;
        unsafe { out.add(index).write(word.wrapping_mul(0x04030201).wrapping_add(0x10203040)); }
    }
}
#[test]
fn all_adapters_preserve_word_abi_and_buffer_boundaries() {
    for fill in [old::getrandom_inner, middle::fill_inner, newest::fill_inner] {
        for length in [0, 1, 2, 3, 4, 5, 255, 256, 257, 511, 512, 513, 4097] {
            for offset in 0..4 {
                WORD.store(0, SeqCst); CALLS.store(0, SeqCst);
                let mut bytes = vec![MaybeUninit::new(0xa5u8); length + offset + 4];
                fill(&mut bytes[offset..offset + length]).unwrap();
                let bytes: Vec<_> = bytes.into_iter().map(|b| unsafe { b.assume_init() }).collect();
                assert!(bytes[..offset].iter().all(|&b| b == 0xa5));
                assert!(bytes[offset + length..].iter().all(|&b| b == 0xa5));
                let expected: Vec<_> = (0..((length + 3) / 4)).flat_map(|i| (i as u32).wrapping_mul(0x04030201).wrapping_add(0x10203040).to_le_bytes()).take(length).collect();
                assert_eq!(&bytes[offset..offset + length], &expected);
                assert_eq!(CALLS.load(SeqCst), (length + 255) / 256);
            }
        }
    }
}
