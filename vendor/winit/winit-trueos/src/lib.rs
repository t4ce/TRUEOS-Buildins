//! TRUEOS UI4 backend.
//!
//! A TRUEOS window has a triple-buffered scene behind a retained double-buffered
//! UI layer. Released UI buffers can be scanned directly; UI4 synchronizes their
//! producer/display ownership.
mod abi;
mod device_input;
mod event_loop;
mod input;
mod input_ext;
pub use abi::{KeyboardOutputEvent, PanEvent, PointerEvent};
pub use input_ext::{InputBatch, InputDevice, InputEvent, device_source};
/// Native HID discovery, independent sequence readers, combo bindings, and virtual devices.
#[cfg(target_os = "trueos")]
pub use v::vinput as hid;
mod window;
pub use event_loop::{
    ActiveEventLoop, EventLoop, EventLoopProxy, PlatformSpecificEventLoopAttributes,
};
pub use input::{physicalkey_to_scancode, scancode_to_physicalkey};
pub use window::Window;
use winit_core::window::Window as CoreWindow;
/// Content coordinates within a full-sized UI4 allocation. Empty margins need no scene draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentViewport {
    pub position: dpi::PhysicalPosition<u32>,
    pub size: dpi::PhysicalSize<u32>,
}
pub trait WindowExtTrueOS {
    fn trueos_window_id(&self) -> u32;
    /// App-owned content policy. The backing frame and SurfaceResized remain full-sized.
    /// None restores unrestricted resizing. Both ratio dimensions must be nonzero.
    fn trueos_content_viewport(&self) -> ContentViewport;
    fn trueos_set_resize_aspect_ratio(
        &self,
        ratio: Option<dpi::PhysicalSize<u32>>,
    ) -> Result<(), winit_core::error::RequestError>;
    /// Enable a bounded copy of routed UI4 input alongside standard Winit events.
    /// Disabled by default. Disabling clears the queue and loss counter.
    fn trueos_capture_input(&self, enabled: bool);
    /// Drain captured input. Call from `about_to_wait` or a window event callback.
    /// This never consumes input from another window or from standard Winit callbacks.
    fn trueos_take_input(&self) -> InputBatch;
}
impl WindowExtTrueOS for dyn CoreWindow + '_ {
    fn trueos_content_viewport(&self) -> ContentViewport {
        self.cast_ref::<Window>().expect("non-TRUEOS window on TRUEOS").trueos_content_viewport()
    }
    fn trueos_set_resize_aspect_ratio(
        &self,
        ratio: Option<dpi::PhysicalSize<u32>>,
    ) -> Result<(), winit_core::error::RequestError> {
        self.cast_ref::<Window>()
            .expect("non-TRUEOS window on TRUEOS")
            .trueos_set_resize_aspect_ratio(ratio)
    }

    fn trueos_capture_input(&self, enabled: bool) {
        self.cast_ref::<Window>()
            .expect("non-TRUEOS window on TRUEOS")
            .trueos_capture_input(enabled);
    }
    fn trueos_take_input(&self) -> InputBatch {
        self.cast_ref::<Window>().expect("non-TRUEOS window on TRUEOS").trueos_take_input()
    }
    fn trueos_window_id(&self) -> u32 {
        self.cast_ref::<Window>().expect("non-TRUEOS window on TRUEOS").trueos_window_id()
    }
}

impl WindowExtTrueOS for Window {
    fn trueos_content_viewport(&self) -> ContentViewport {
        let surface = *self.inner.size.lock().unwrap();
        let size = window::fit_resize(surface, *self.inner.resize_aspect_ratio.lock().unwrap());
        ContentViewport {
            position: dpi::PhysicalPosition::new(
                (surface.width - size.width) / 2,
                (surface.height - size.height) / 2,
            ),
            size,
        }
    }

    fn trueos_set_resize_aspect_ratio(
        &self,
        ratio: Option<dpi::PhysicalSize<u32>>,
    ) -> Result<(), winit_core::error::RequestError> {
        if ratio.is_some_and(|ratio| ratio.width == 0 || ratio.height == 0) {
            return Err(winit_core::error::NotSupportedError::new(
                "resize aspect dimensions must be nonzero",
            )
            .into());
        }
        *self.inner.resize_aspect_ratio.lock().unwrap() = ratio;
        Ok(())
    }

    fn trueos_window_id(&self) -> u32 {
        self.trueos_window_id()
    }
    fn trueos_capture_input(&self, enabled: bool) {
        self.inner.input.lock().unwrap().enable(enabled);
    }
    fn trueos_take_input(&self) -> InputBatch {
        self.inner.input.lock().unwrap().take()
    }
}

/// Configure the scene cadence of a window with independent scene and UI layers.
/// Standard `create_window` uses the layered contract at 60 Hz.
pub trait ActiveEventLoopExtTrueOS {
    /// Create one Streaming/Triple-buffered GPU frame with no paired UI layer.
    /// The caller renders directly to `trueos_window_id()` on demand.
    fn create_streaming_window(
        &self, attributes: winit_core::window::WindowAttributes,
    ) -> Result<Box<dyn CoreWindow>, winit_core::error::RequestError>;

    fn create_layered_window(
        &self,
        attributes: winit_core::window::WindowAttributes,
        background_hz: u32,
    ) -> Result<Box<dyn CoreWindow>, winit_core::error::RequestError>;
    /// Scale only the scene backing; UI, window size and input stay physical.
    fn create_layered_window_with_scene_divisor(
        &self, attributes: winit_core::window::WindowAttributes,
        background_hz: u32, scene_divisor: u32,
    ) -> Result<Box<dyn CoreWindow>, winit_core::error::RequestError>;

}
impl ActiveEventLoopExtTrueOS for dyn winit_core::event_loop::ActiveEventLoop + '_ {
    fn create_streaming_window(
        &self, attributes: winit_core::window::WindowAttributes,
    ) -> Result<Box<dyn CoreWindow>, winit_core::error::RequestError> {
        let event_loop = self.cast_ref::<ActiveEventLoop>().expect("non-TRUEOS event loop on TRUEOS");
        Window::new_streaming(event_loop, attributes)
            .map(|window| Box::new(window) as Box<dyn CoreWindow>)
    }

    fn create_layered_window(
        &self,
        attributes: winit_core::window::WindowAttributes,
        background_hz: u32,
    ) -> Result<Box<dyn CoreWindow>, winit_core::error::RequestError> {
        let event_loop =
            self.cast_ref::<ActiveEventLoop>().expect("non-TRUEOS event loop on TRUEOS");
        Window::new_with_layers(event_loop, attributes, Some(background_hz))
            .map(|window| Box::new(window) as Box<dyn CoreWindow>)
    }
    fn create_layered_window_with_scene_divisor(
        &self, attributes: winit_core::window::WindowAttributes,
        background_hz: u32, scene_divisor: u32,
    ) -> Result<Box<dyn CoreWindow>, winit_core::error::RequestError> {
        let event_loop = self.cast_ref::<ActiveEventLoop>().expect("non-TRUEOS event loop on TRUEOS");
        Window::new_with_scaled_scene(event_loop, attributes, Some(background_hz), scene_divisor)
            .map(|window| Box::new(window) as Box<dyn CoreWindow>)
    }

}
