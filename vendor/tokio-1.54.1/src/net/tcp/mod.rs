//! TCP utility types.

#[allow(unused_imports)]
use crate::alloc_prelude::*;
pub(crate) mod listener;

cfg_not_wasip1! {
    #[cfg_attr(feature="native",path="socket_native.rs")]
    pub(crate) mod socket;
}

mod split;
pub use split::{ReadHalf, WriteHalf};

mod split_owned;
pub use split_owned::{OwnedReadHalf, OwnedWriteHalf, ReuniteError};

pub(crate) mod stream;
pub(crate) use stream::TcpStream;
