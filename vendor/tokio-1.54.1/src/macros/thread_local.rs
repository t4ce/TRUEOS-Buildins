#[allow(unused_imports)]
use crate::alloc_prelude::*;
#[cfg(all(loom, test))]
macro_rules! tokio_thread_local {
    ($(#[$attrs:meta])* $vis:vis static $name:ident: $ty:ty = const { $expr:expr } $(;)?) => {
        loom::thread_local! {
            $(#[$attrs])*
            $vis static $name: $ty = $expr;
        }
    };

    ($($tts:tt)+) => { loom::thread_local!{ $($tts)+ } }
}

#[cfg(all(not(feature="native"),not(all(loom, test))))]
macro_rules! tokio_thread_local {
    ($($tts:tt)+) => {
        ::std::thread_local!{ $($tts)+ }
    }
}

#[cfg(feature="native")]
macro_rules! tokio_thread_local {
    ($(#[$attrs:meta])* $vis:vis static $name:ident: $ty:ty = const { $expr:expr } $(;)?) => {
        $(#[$attrs])* $vis static $name: trueos_runtime::thread::LocalKey<$ty> =
            trueos_runtime::thread::LocalKey::new(|| $expr);
    };
    ($(#[$attrs:meta])* $vis:vis static $name:ident: $ty:ty = $expr:expr $(;)?) => {
        $(#[$attrs])* $vis static $name: trueos_runtime::thread::LocalKey<$ty> =
            trueos_runtime::thread::LocalKey::new(|| $expr);
    };
}
