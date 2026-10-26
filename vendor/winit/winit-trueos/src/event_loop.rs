use core::num::NonZeroU64;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use winit_core::application::ApplicationHandler;
use winit_core::cursor::{CustomCursor, CustomCursorSource};
use winit_core::error::{EventLoopError, NotSupportedError, RequestError};
use winit_core::event::{StartCause, WindowEvent};
use winit_core::event_loop::{
    ActiveEventLoop as CoreActiveEventLoop, ControlFlow, DeviceEvents, EventLoopProvider,
    EventLoopProxy as CoreEventLoopProxy, EventLoopProxyProvider, OwnedDisplayHandle,
};
use winit_core::monitor::MonitorHandle;
use winit_core::window::{Theme, Window as CoreWindow, WindowAttributes, WindowId};

use crate::window::WindowInner;
use crate::{Window, abi, input};

/// Measure caller wall time, including transport waits and callback work.
/// Slow-path-only records keep normal input dispatch free of log traffic.
pub(crate) fn report_slow_stage(stage: &'static str, started: Instant) {
    let elapsed = started.elapsed();
    if elapsed >= Duration::from_millis(250) {
        tracing::warn!(target: "winit_trueos_latency", stage,
            elapsed_us = elapsed.as_micros() as u64, "Slow TRUEOS event-loop stage");
    }
}

#[derive(Debug)]
pub(crate) struct Ui4Connection(NonZeroU64);

impl Ui4Connection {
    fn open() -> Option<Self> {
        NonZeroU64::new(unsafe { abi::trueos_cabi_ui4_display_open_v1() }).map(Self)
    }

    pub(crate) fn raw(&self) -> NonZeroU64 {
        self.0
    }
}

impl Drop for Ui4Connection {
    fn drop(&mut self) {
        unsafe { abi::trueos_cabi_ui4_display_close_v1(self.0.get()) };
    }
}

impl rwh_06::HasDisplayHandle for Ui4Connection {
    fn display_handle(&self) -> Result<rwh_06::DisplayHandle<'_>, rwh_06::HandleError> {
        let raw = rwh_06::RawDisplayHandle::Trueos(rwh_06::TrueosDisplayHandle::new(self.0));
        // SAFETY: `raw` contains only an owned integer connection capability.
        Ok(unsafe { rwh_06::DisplayHandle::borrow_raw(raw) })
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PlatformSpecificEventLoopAttributes {}
#[derive(Debug)]
pub struct EventLoop {
    active: ActiveEventLoop,
}
#[derive(Debug)]
pub struct EventLoopProxy {
    woken: AtomicBool,
    sleeper: Arc<(Mutex<()>, Condvar)>,
}
#[derive(Debug)]
pub struct ActiveEventLoop {
    control: Mutex<ControlFlow>,
    device_events: Mutex<DeviceEvents>,
    device_input: Mutex<crate::device_input::DeviceInput>,
    exiting: AtomicBool,
    pub(crate) redraws: Arc<Mutex<VecDeque<WindowId>>>,
    pub(crate) pending_window_events: Arc<Mutex<VecDeque<(WindowId, WindowEvent)>>>,
    pub(crate) windows: Mutex<Vec<std::sync::Weak<WindowInner>>>,
    pub(crate) proxy: Arc<EventLoopProxy>,
    pub(crate) connection: Arc<Ui4Connection>,
}
impl EventLoop {
    fn drain_redraws(&self) -> Vec<WindowId> {
        self.active.redraws.lock().unwrap().drain(..).collect()
    }
    pub fn new(_: &PlatformSpecificEventLoopAttributes) -> Result<Self, EventLoopError> {
        static CREATED: AtomicBool = AtomicBool::new(false);
        if CREATED.swap(true, Ordering::Relaxed) {
            return Err(EventLoopError::RecreationAttempt);
        };
        let proxy = Arc::new(EventLoopProxy {
            woken: AtomicBool::new(false),
            sleeper: Arc::new((Mutex::new(()), Condvar::new())),
        });
        let Some(connection) = Ui4Connection::open() else {
            return Err(NotSupportedError::new("UI4 graphics connection unavailable").into());
        };
        Ok(Self {
            active: ActiveEventLoop {
                control: Mutex::new(ControlFlow::Wait),
                device_events: Mutex::new(DeviceEvents::default()),
                device_input: Mutex::new(crate::device_input::DeviceInput::default()),
                exiting: AtomicBool::new(false),
                redraws: Arc::new(Mutex::new(VecDeque::new())),
                pending_window_events: Arc::new(Mutex::new(VecDeque::new())),
                windows: Mutex::new(Vec::new()),
                proxy,
                connection: Arc::new(connection),
            },
        })
    }
    pub fn window_target(&self) -> &dyn CoreActiveEventLoop {
        &self.active
    }
}
impl EventLoop {
    fn iteration<A: ApplicationHandler>(&self, app: &mut A, cause: StartCause, first: bool) {
        let lifecycle_started = Instant::now();
        app.new_events(&self.active, cause);
        if first {
            app.can_create_surfaces(&self.active)
        }
        if self.active.proxy.woken.swap(false, Ordering::AcqRel) {
            app.proxy_wake_up(&self.active)
        }
        let pending_window_events =
            self.active.pending_window_events.lock().unwrap().drain(..).collect::<Vec<_>>();
        for (id, event) in pending_window_events {
            app.window_event(&self.active, id, event);
        }
        let windows = {
            let mut windows = self.active.windows.lock().unwrap();
            windows.retain(|window| window.strong_count() != 0);
            windows.iter().filter_map(std::sync::Weak::upgrade).collect::<Vec<_>>()
        };
        report_slow_stage("lifecycle and queued callbacks", lifecycle_started);
        for window in &windows {
            if window.closed.load(Ordering::Acquire) {
                continue;
            }
            let id = WindowId::from_raw(window.id as usize);
            let state_started = Instant::now();
            let mut xy = [0i32; 2];
            if unsafe { abi::trueos_cabi_ui4_scene_frame_get_position(window.id, xy.as_mut_ptr()) }
                == 0
            {
                let position = dpi::PhysicalPosition::new(xy[0], xy[1]);
                let mut previous = window.position.lock().unwrap();
                if *previous != position {
                    *previous = position;
                    drop(previous);
                    app.window_event(&self.active, id, WindowEvent::Moved(position));
                }
            }
            let pending_resize = {
                let mut pending = window.pending_resize.lock().unwrap();
                // A failed allocation must not pin an obsolete dock target.
                // The kernel coalesces its queue; always take a newer request.
                let mut raw = abi::ResizeEvent::default();
                if unsafe { abi::trueos_cabi_ui4_scene_resize_event_take(window.id, &mut raw) } == 0
                {
                    *pending = Some(raw);
                    *window.resize_failures.lock().unwrap() = 0;
                    tracing::info!(
                        window = window.id,
                        width = raw.width,
                        height = raw.height,
                        "UI4 resize received"
                    );
                }
                *pending
            };
            if let Some(raw) = pending_resize {
                let status = unsafe {
                    abi::trueos_cabi_ui4_scene_frame_resize(window.id, raw.width, raw.height)
                };
                if status == 0 {
                    *window.pending_resize.lock().unwrap() = None;
                    *window.resize_failures.lock().unwrap() = 0;
                    tracing::info!(
                        window = window.id,
                        width = raw.width,
                        height = raw.height,
                        "UI4 resize staged; delivering SurfaceResized"
                    );
                    let size = dpi::PhysicalSize::new(raw.width, raw.height);
                    *window.size.lock().unwrap() = size;
                    app.window_event(&self.active, id, WindowEvent::SurfaceResized(size));
                    window.redraws.lock().unwrap().push_back(id);
                } else {
                    let mut failures = window.resize_failures.lock().unwrap();
                    *failures = failures.saturating_add(1);
                    if *failures == 1 || failures.is_power_of_two() {
                        tracing::warn!(
                            window = window.id,
                            width = raw.width,
                            height = raw.height,
                            status,
                            attempts = *failures,
                            "UI4 resize not staged; app still reports previous size"
                        );
                    }
                }
            }
            if let Some(state) = {
                let mut state = abi::WindowStateV1::default();
                (unsafe { abi::trueos_cabi_ui4_scene_window_state_get_v1(window.id, &mut state) }
                    == 0
                    && state.version == abi::WINDOW_STATE_V1_VERSION)
                    .then_some(state)
            } {
                // A dock may change mode without changing pixel extent. Give
                // the app a callback in which to query fullscreen/maximize state.
                let maximized = state.reserved[0] != 0;
                let mut previous_maximized = window.maximized.lock().unwrap();
                if *previous_maximized != maximized {
                    *previous_maximized = maximized;
                    window.redraws.lock().unwrap().push_back(id);
                }
                drop(previous_maximized);
                let focused = state.focused != 0;
                let mut previous = window.focused.lock().unwrap();
                if previous.replace(focused) != Some(focused) {
                    drop(previous);
                    app.window_event(&self.active, id, WindowEvent::Focused(focused));
                }
            }
            report_slow_stage("window position, resize and focus", state_started);
            let keyboard_started = Instant::now();
            loop {
                let mut raw = abi::KeyboardOutputEvent::default();
                let result = unsafe {
                    abi::trueos_cabi_ui4_scene_keyboard_event_take_v1(
                        window.id,
                        (&mut raw as *mut abi::KeyboardOutputEvent).cast(),
                    )
                };
                if result != 0 {
                    break;
                }
                window.input.lock().unwrap().push(crate::InputEvent::Keyboard(raw));
                let events = window.keyboard.lock().unwrap().translate(&raw);
                for event in events {
                    app.window_event(&self.active, id, event);
                }
            }
            report_slow_stage("routed keyboard drain and callbacks", keyboard_started);
            let pointer_started = Instant::now();
            loop {
                let mut raw = abi::PointerEvent::default();
                let result = unsafe {
                    abi::trueos_cabi_ui4_scene_pointer_event_take(
                        window.id,
                        (&mut raw as *mut abi::PointerEvent).cast(),
                    )
                };
                if result != 0 {
                    break;
                }
                window.input.lock().unwrap().push(crate::InputEvent::Pointer(raw));
                for event in input::pointer_events(&raw) {
                    app.window_event(&self.active, id, event);
                }
            }
            report_slow_stage("routed pointer drain and callbacks", pointer_started);
            let pan_started = Instant::now();
            loop {
                let mut raw = abi::PanEvent::default();
                if unsafe { abi::trueos_cabi_ui4_scene_pan_event_take(window.id, &mut raw) } != 0 {
                    break;
                }
                window.input.lock().unwrap().push(crate::InputEvent::Pan(raw));
                if let Some(event) = input::pan_event(&raw) {
                    app.window_event(&self.active, id, event);
                }
            }
            report_slow_stage("routed pan drain and callbacks", pan_started);
        }
        let device_started = Instant::now();
        let focused = windows.iter().any(|window| {
            !window.closed.load(Ordering::Acquire) && *window.focused.lock().unwrap() == Some(true)
        });
        let enabled = match *self.active.device_events.lock().unwrap() {
            DeviceEvents::Always => true,
            DeviceEvents::WhenFocused => focused,
            DeviceEvents::Never => false,
        };
        let device_events = self.active.device_input.lock().unwrap().poll(enabled);
        for (id, event) in device_events {
            app.device_event(&self.active, Some(id), event);
        }
        report_slow_stage("raw device polling and callbacks", device_started);
        let redraw_started = Instant::now();
        // Drain before callbacks. A redraw requested during a callback belongs
        // to the next iteration and cannot deadlock on this queue's mutex.
        let live_window_ids = windows
            .iter()
            .filter(|window| !window.closed.load(Ordering::Acquire))
            .map(|window| WindowId::from_raw(window.id as usize))
            .collect::<HashSet<_>>();
        let redraws = self.drain_redraws();
        for id in redraws {
            if live_window_ids.contains(&id) {
                app.window_event(&self.active, id, WindowEvent::RedrawRequested)
            }
        }
        report_slow_stage("redraw callbacks", redraw_started);
        let app_started = Instant::now();
        app.about_to_wait(&self.active);
        report_slow_stage("application about_to_wait", app_started);
    }

    fn run_app_inner<A: ApplicationHandler>(&mut self, app: &mut A) {
        self.active.exiting.store(false, Ordering::Release);
        self.iteration(app, StartCause::Init, true);
        while !self.active.exiting() {
            let start = Instant::now();
            let flow = *self.active.control.lock().unwrap();
            let requested_resume = match flow {
                ControlFlow::WaitUntil(deadline) => Some(deadline),
                _ => None,
            };
            let timeout = match flow {
                ControlFlow::Poll => Duration::ZERO,
                ControlFlow::Wait => Duration::from_millis(8),
                ControlFlow::WaitUntil(deadline) => {
                    deadline.saturating_duration_since(start).min(Duration::from_millis(8))
                },
            };
            let was_woken = self.active.proxy.woken.load(Ordering::Acquire);
            if !was_woken && !timeout.is_zero() {
                let wait_started = Instant::now();
                let (lock, cvar) = &*self.active.proxy.sleeper;
                let _ = cvar.wait_timeout(lock.lock().unwrap(), timeout).unwrap();
                let elapsed = wait_started.elapsed();
                if elapsed >= Duration::from_millis(250) {
                    tracing::warn!(target: "winit_trueos_latency",
                        requested_us = timeout.as_micros() as u64,
                        elapsed_us = elapsed.as_micros() as u64, "Slow TRUEOS event-loop wait");
                }
            }
            let cause = match flow {
                ControlFlow::Poll => StartCause::Poll,
                ControlFlow::Wait => StartCause::WaitCancelled { start, requested_resume: None },
                ControlFlow::WaitUntil(deadline) => {
                    if Instant::now() >= deadline {
                        StartCause::ResumeTimeReached { start, requested_resume: deadline }
                    } else {
                        StartCause::WaitCancelled { start, requested_resume }
                    }
                },
            };
            self.iteration(app, cause, false)
        }
    }

    pub fn run_app_on_demand<A: ApplicationHandler>(
        &mut self,
        mut app: A,
    ) -> Result<(), EventLoopError> {
        self.run_app_inner(&mut app);
        Ok(())
    }
}
impl EventLoopProvider for EventLoop {
    fn run_app<A: ApplicationHandler + 'static>(
        mut self,
        mut app: A,
    ) -> Result<(), EventLoopError> {
        self.run_app_inner(&mut app);
        Ok(())
    }
    fn create_proxy(&self) -> CoreEventLoopProxy {
        self.active.create_proxy()
    }
    fn owned_display_handle(&self) -> OwnedDisplayHandle {
        self.active.owned_display_handle()
    }
    fn listen_device_events(&self, a: DeviceEvents) {
        self.active.listen_device_events(a)
    }
    fn set_control_flow(&self, c: ControlFlow) {
        self.active.set_control_flow(c)
    }
    fn create_custom_cursor(&self, c: CustomCursorSource) -> Result<CustomCursor, RequestError> {
        self.active.create_custom_cursor(c)
    }
}
impl EventLoopProxyProvider for EventLoopProxy {
    fn wake_up(&self) {
        self.woken.store(true, Ordering::Release);
        self.sleeper.1.notify_one();
    }
}
impl CoreActiveEventLoop for ActiveEventLoop {
    fn create_proxy(&self) -> CoreEventLoopProxy {
        CoreEventLoopProxy::new(self.proxy.clone())
    }
    fn create_window(&self, a: WindowAttributes) -> Result<Box<dyn CoreWindow>, RequestError> {
        Ok(Box::new(Window::new(self, a)?))
    }
    fn create_custom_cursor(&self, _: CustomCursorSource) -> Result<CustomCursor, RequestError> {
        Err(NotSupportedError::new("custom cursors are not supported by TRUEOS UI4").into())
    }
    fn available_monitors(&self) -> Box<dyn Iterator<Item = MonitorHandle>> {
        Box::new(std::iter::empty())
    }
    fn primary_monitor(&self) -> Option<MonitorHandle> {
        None
    }
    fn listen_device_events(&self, mode: DeviceEvents) {
        *self.device_events.lock().unwrap() = mode;
    }
    fn system_theme(&self) -> Option<Theme> {
        None
    }
    fn set_control_flow(&self, c: ControlFlow) {
        *self.control.lock().unwrap() = c
    }
    fn control_flow(&self) -> ControlFlow {
        *self.control.lock().unwrap()
    }
    fn exit(&self) {
        self.exiting.store(true, Ordering::Release)
    }
    fn exiting(&self) -> bool {
        self.exiting.load(Ordering::Acquire)
    }
    fn owned_display_handle(&self) -> OwnedDisplayHandle {
        OwnedDisplayHandle::new(self.connection.clone())
    }
    fn rwh_06_handle(&self) -> &dyn rwh_06::HasDisplayHandle {
        self
    }
}
impl rwh_06::HasDisplayHandle for ActiveEventLoop {
    fn display_handle(&self) -> Result<rwh_06::DisplayHandle<'_>, rwh_06::HandleError> {
        self.connection.display_handle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redraws_are_coalesced_and_drained_before_callbacks() {
        let loop_ = EventLoop::new(&PlatformSpecificEventLoopAttributes::default()).unwrap();
        // Ordinary and explicitly layered windows share the exclusive Winit ABI.
        // The host stub records the open call and refuses allocation.
        assert!(Window::new(&loop_.active, WindowAttributes::default()).is_err());
        abi::TEST_FRAME_OPEN.with(|state| {
            assert_eq!(*state.borrow(), Some((0, 0, 1024, 768, 60)));
        });
        assert!(
            Window::new_with_layers(&loop_.active, WindowAttributes::default(), Some(30),).is_err()
        );
        abi::TEST_FRAME_OPEN.with(|state| {
            assert_eq!(*state.borrow(), Some((0, 0, 1024, 768, 30)));
            *state.borrow_mut() = None;
        });
        for cadence in [0, 61] {
            assert!(
                Window::new_with_layers(&loop_.active, WindowAttributes::default(), Some(cadence),)
                    .is_err()
            );
            abi::TEST_FRAME_OPEN.with(|state| assert!(state.borrow().is_none()));
        }
        assert!(Window::new_streaming(&loop_.active, WindowAttributes::default()).is_err());
        abi::TEST_STREAMING_FRAME_OPEN.with(|state| {
            assert_eq!(*state.borrow(), Some((0,0,1024,768)));
        });
        // The streaming path must not allocate the layered foreground/background pair.
        abi::TEST_FRAME_OPEN.with(|state| assert!(state.borrow().is_none()));
        let id = WindowId::from_raw(7);
        {
            let mut redraws = loop_.active.redraws.lock().unwrap();
            redraws.push_back(id);
        }
        assert_eq!(loop_.drain_redraws(), vec![id]);
        loop_.active.redraws.lock().unwrap().push_back(id);
        assert_eq!(loop_.drain_redraws(), vec![id]);

        let window = Window {
            inner: Arc::new(WindowInner {
                id: 7,
                connection: loop_.active.connection.clone(),
                size: Mutex::new(dpi::PhysicalSize::new(1, 1)),
                pending_resize: Mutex::new(None),
                resize_failures: Mutex::new(0),
                resize_aspect_ratio: Mutex::new(None),
                position: Mutex::new(dpi::PhysicalPosition::new(0, 0)),
                focused: Mutex::new(None),
                maximized: Mutex::new(false),
                closed: AtomicBool::new(false),
                redraws: loop_.active.redraws.clone(),
                pending_window_events: loop_.active.pending_window_events.clone(),
                proxy: loop_.active.proxy.clone(),
                input: Mutex::new(crate::input_ext::InputQueue::default()),
                keyboard: Mutex::new(input::KeyboardState::default()),
            }),
        };
        window.request_redraw();
        assert!(loop_.active.proxy.woken.load(Ordering::Acquire));
        // Exercise real iteration: a failed fullscreen allocation must yield
        // to a newer small resize and deliver resize before its redraw.
        #[derive(Default)]
        struct ResizeApp(Vec<&'static str>);
        impl ApplicationHandler for ResizeApp {
            fn can_create_surfaces(&mut self, _: &dyn CoreActiveEventLoop) {}
            fn window_event(
                &mut self,
                _: &dyn CoreActiveEventLoop,
                _: WindowId,
                event: WindowEvent,
            ) {
                match event {
                    WindowEvent::SurfaceResized(size) => {
                        assert_eq!(size, dpi::PhysicalSize::new(640, 360));
                        self.0.push("resize");
                    },
                    WindowEvent::RedrawRequested => self.0.push("redraw"),
                    _ => {},
                }
            }
        }
        loop_.active.windows.lock().unwrap().push(Arc::downgrade(&window.inner));
        loop_.drain_redraws();
        let mut app = ResizeApp::default();
        abi::TEST_RESIZE.with(|state| {
            *state.borrow_mut() =
                (Some(abi::ResizeEvent { width: 2560, height: 1440, ..Default::default() }), -5)
        });
        loop_.iteration(&mut app, StartCause::Poll, false);
        assert!(app.0.is_empty());
        assert_eq!(window.surface_size(), dpi::PhysicalSize::new(1, 1));
        assert!(window.inner.pending_resize.lock().unwrap().is_some());
        // No new request retains the failed target for retry.
        loop_.iteration(&mut app, StartCause::Poll, false);
        assert!(app.0.is_empty());
        use crate::WindowExtTrueOS;
        window.trueos_set_resize_aspect_ratio(Some(dpi::PhysicalSize::new(4, 3))).unwrap();
        abi::TEST_RESIZE.with(|state| {
            *state.borrow_mut() =
                (Some(abi::ResizeEvent { width: 640, height: 360, ..Default::default() }), 0)
        });
        loop_.iteration(&mut app, StartCause::Poll, false);
        assert_eq!(app.0, vec!["resize", "redraw"]);
        assert_eq!(window.surface_size(), dpi::PhysicalSize::new(640, 360));
        let viewport = window.trueos_content_viewport();
        assert_eq!(viewport.size, dpi::PhysicalSize::new(480, 360));
        assert_eq!(viewport.position, dpi::PhysicalPosition::new(80, 0));
        assert!(window.trueos_set_resize_aspect_ratio(Some(dpi::PhysicalSize::new(0, 3))).is_err());
        assert_eq!(window.trueos_content_viewport(), viewport);
        window.trueos_set_resize_aspect_ratio(None).unwrap();
        assert_eq!(window.trueos_content_viewport().size, window.surface_size());
        assert!(window.inner.pending_resize.lock().unwrap().is_none());
        abi::TEST_RESIZE.with(|state| *state.borrow_mut() = (None, -5));
        drop(window);
        assert_eq!(
            loop_
                .active
                .pending_window_events
                .lock()
                .unwrap()
                .pop_front()
                .map(|(id, event)| (id, matches!(event, WindowEvent::Destroyed))),
            Some((id, true))
        );
    }

    #[test]
    fn proxy_wake_notifies_waiters() {
        let proxy = EventLoopProxy {
            woken: AtomicBool::new(false),
            sleeper: Arc::new((Mutex::new(()), Condvar::new())),
        };
        proxy.wake_up();
        assert!(proxy.woken.load(Ordering::Acquire));
    }
}
