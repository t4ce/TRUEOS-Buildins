#[allow(unused_imports)]
use crate::alloc_prelude::*;
macro_rules! if_loom {
    ($($t:tt)*) => {{
        #[cfg(loom)]
        {
            $($t)*
        }
    }}
}
