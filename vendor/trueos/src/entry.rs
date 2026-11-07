//! Native completion for Blueprint entry points.

/// Finish a normally returned Blueprint main function.
///
/// Call this after application-owned values have been dropped. Release the
/// borrowed terminal before the lifecycle request stops the Blueprint. A caller
/// without a stoppable Blueprint context remains in the spin fallback.
pub fn finish() -> ! {
    crate::vshell::leave_terminal_handoff();
    let _ = crate::vshell::shutdown_current_blueprint("blueprint main returned");
    loop {
        core::hint::spin_loop();
    }
}
