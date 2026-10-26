#[cfg(any(x11_platform, wayland_platform))]
mod linux;
#[cfg(trueos_platform)]
pub(crate) use winit_trueos as platform;

#[cfg(any(x11_platform, wayland_platform))]
use self::linux as platform;
#[allow(unused_imports)]
pub use self::platform::*;

#[cfg(not(any(x11_platform, wayland_platform, trueos_platform)))]
compile_error!("This winit fork supports TrueOS and Linux with X11 or Wayland");
