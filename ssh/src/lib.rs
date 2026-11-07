//! Shared native/host app state machines, independent of the terminal backend.
#![cfg_attr(target_os = "trueos", no_std)]
extern crate alloc;
pub mod escape;
pub mod options;
pub mod platform_store;
pub mod queue;
pub mod session;
pub mod storage;
pub mod terminal;
pub mod wire;
use alloc::string::String;
pub type Result<T> = core::result::Result<T, String>;
#[cfg(target_os = "trueos")]
pub fn stage_log(stage: &str) {
    trueos::vsys::log_infof(format_args!("ssh: stage={stage}\n"));
}
#[cfg(not(target_os = "trueos"))]
pub fn stage_log(_: &str) {}
pub fn process_var(name: &str) -> Option<String> {
    #[cfg(target_os = "trueos")]
    {
        trueos::env::var(name).ok()
    }
    #[cfg(not(target_os = "trueos"))]
    {
        std::env::var(name).ok()
    }
}
