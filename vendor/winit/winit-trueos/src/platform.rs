#[cfg(all(feature = "trueos-native", target_os = "trueos"))]
pub(crate) use trueos_runtime::{sync::{Condvar, Mutex}, time::Instant};

#[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
pub(crate) use std::{sync::{Condvar, Mutex}, time::Instant};
