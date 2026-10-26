//! TrueOS kernel random source, with the getrandom 0.2 API adapter.
use crate::Error;
use core::mem::MaybeUninit;

#[path = "../../getrandom-trueos.rs"]
mod trueos;

pub fn getrandom_inner(dest: &mut [MaybeUninit<u8>]) -> Result<(), Error> {
    trueos::fill(crate::util::uninit_slice_fill_zero(dest));
    Ok(())
}
