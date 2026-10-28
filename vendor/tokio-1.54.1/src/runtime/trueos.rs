//! TRUEOS placement hints and current execution activity.
//!
//! Tokio scheduler cores and blocking closures share the same thread pool.
//! Admission therefore identifies a pool thread, while a separate scoped
//! activity marker describes what that thread is currently executing.

use std::marker::PhantomData;

const TOKIO_POOL_ROLE: u32 = 2;
const ASYNC_SCHEDULER: u32 = 2;
const BLOCKING: u32 = 3;

#[cfg(not(test))]
unsafe extern "C" {
    fn trueos_cabi_thread_swap_child_role_v1(role: u32) -> u32;
    fn trueos_cabi_thread_swap_activity_v1(activity: u32) -> u32;
}

// Neither guard may move to another logical thread: the platform stores these
// values per continuation, including while a synchronous ABI call suspends it.
pub(crate) struct ChildRoleGuard(u32, PhantomData<*mut ()>);

impl ChildRoleGuard {
    pub(crate) fn tokio_pool() -> Self {
        Self(
            unsafe { trueos_cabi_thread_swap_child_role_v1(TOKIO_POOL_ROLE) },
            PhantomData,
        )
    }
}

impl Drop for ChildRoleGuard {
    fn drop(&mut self) {
        if self.0 != u32::MAX {
            unsafe { trueos_cabi_thread_swap_child_role_v1(self.0) };
        }
    }
}

pub(crate) struct ActivityGuard(u32, PhantomData<*mut ()>);

impl ActivityGuard {
    pub(crate) fn async_scheduler() -> Self {
        Self::enter(ASYNC_SCHEDULER)
    }

    pub(crate) fn blocking() -> Self {
        Self::enter(BLOCKING)
    }

    fn enter(activity: u32) -> Self {
        Self(
            unsafe { trueos_cabi_thread_swap_activity_v1(activity) },
            PhantomData,
        )
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        if self.0 != u32::MAX {
            unsafe { trueos_cabi_thread_swap_activity_v1(self.0) };
        }
    }
}

#[cfg(test)]
std::thread_local! {
    static CHILD_ROLE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static ACTIVITY: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
unsafe fn trueos_cabi_thread_swap_child_role_v1(role: u32) -> u32 {
    CHILD_ROLE.with(|current| current.replace(role))
}

#[cfg(test)]
unsafe fn trueos_cabi_thread_swap_activity_v1(activity: u32) -> u32 {
    ACTIVITY.with(|current| current.replace(activity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_hint_restores_on_error_and_unwind() {
        CHILD_ROLE.with(|current| current.set(1));
        let spawn_error = || -> std::io::Result<()> {
            let _role = ChildRoleGuard::tokio_pool();
            assert_eq!(CHILD_ROLE.with(|current| current.get()), TOKIO_POOL_ROLE);
            Err(std::io::Error::from_raw_os_error(11))
        };
        assert!(spawn_error().is_err());
        assert_eq!(CHILD_ROLE.with(|current| current.get()), 1);
        assert!(std::panic::catch_unwind(|| {
            let _role = ChildRoleGuard::tokio_pool();
            panic!("spawn hook panicked");
        })
        .is_err());
        assert_eq!(CHILD_ROLE.with(|current| current.get()), 1);
    }

    #[test]
    fn scheduler_and_blocking_activity_nest_and_restore_on_unwind() {
        {
            let _blocking_task = ActivityGuard::blocking();
            assert_eq!(ACTIVITY.with(|current| current.get()), BLOCKING);
            {
                let _scheduler = ActivityGuard::async_scheduler();
                assert_eq!(ACTIVITY.with(|current| current.get()), ASYNC_SCHEDULER);
                assert!(std::panic::catch_unwind(|| {
                    let _block_in_place = ActivityGuard::blocking();
                    assert_eq!(ACTIVITY.with(|current| current.get()), BLOCKING);
                    panic!("blocking closure panicked");
                })
                .is_err());
                assert_eq!(ACTIVITY.with(|current| current.get()), ASYNC_SCHEDULER);
            }
            assert_eq!(ACTIVITY.with(|current| current.get()), BLOCKING);
        }
        assert_eq!(ACTIVITY.with(|current| current.get()), 0);
    }
}
