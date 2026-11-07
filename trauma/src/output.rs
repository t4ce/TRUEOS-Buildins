//! Existing console diagnostics without a hosted process/stdio dependency.
#[cfg(target_os = "trueos")]
macro_rules! println {
    ($($arg:tt)*) => { trueos::vsys::log_infof(format_args!("{}\n", format_args!($($arg)*))) };
}
#[cfg(target_os = "trueos")]
macro_rules! eprintln {
    ($($arg:tt)*) => { trueos::vsys::log_errorf(format_args!("{}\n", format_args!($($arg)*))) };
}
