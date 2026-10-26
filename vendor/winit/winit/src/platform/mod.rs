//! Platform implementations available in this fork: TrueOS, Linux Wayland and Linux X11.
//!
//! Only the modules corresponding to the platform being compiled are available.
//! `scancode` provides physical-key conversion; `startup_notify` is available on Linux.

#[cfg(trueos_platform)]
pub use winit_trueos as trueos;
#[cfg(wayland_platform)]
pub use winit_wayland as wayland;
#[cfg(x11_platform)]
pub use winit_x11 as x11;

#[cfg(any(x11_platform, wayland_platform, trueos_platform, docsrs))]
pub mod scancode;
#[cfg(any(x11_platform, wayland_platform, docsrs))]
pub mod startup_notify;
