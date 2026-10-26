//! Stable copies of the UI4 event records consumed by this backend.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PointerEvent {
    pub controller_id: u32,
    pub slot_id: u32,
    pub ep_target: u32,
    pub hid_kind: u32,
    pub x: u32,
    pub y: u32,
    pub local_x: i32,
    pub local_y: i32,
    pub dx: i32,
    pub dy: i32,
    pub wheel: i32,
    pub buttons_down: u32,
    pub buttons_pressed: u32,
    pub buttons_released: u32,
    pub combo_id: u32,
    pub vcursor: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyboardOutputEvent {
    pub t_ms: u32,
    pub seq: u32,
    pub device_seq: u32,
    pub controller_id: u32,
    pub slot_id: u32,
    pub ep_target: u32,
    pub modifiers: u8,
    pub kind: u8,
    pub utf8_len: u8,
    pub reserved0: u8,
    pub key_code: u16,
    pub reserved1: u16,
    pub codepoint: u32,
    pub utf8: [u8; 4],
    pub flags: u32,
}
/// Native UI4 pan record. Phase is 1 (begin), 2 (update), or 3 (end).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PanEvent {
    pub controller_id: u32,
    pub slot_id: u32,
    pub ep_target: u32,
    pub hid_kind: u32,
    pub phase: u32,
    pub x: u32,
    pub y: u32,
    pub local_x: i32,
    pub local_y: i32,
    pub dx: i32,
    pub dy: i32,
    pub combo_id: u32,
    pub vcursor: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ResizeEvent {
    pub old_width: u32,
    pub old_height: u32,
    pub width: u32,
    pub height: u32,
}

pub(crate) const WINDOW_STATE_V1_VERSION: u32 = 1;
pub(crate) const MAX_WINDOW_TITLE_BYTES: usize = 120;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WindowStateV1 {
    pub version: u32,
    pub visible: u32,
    pub hit_testable: u32,
    pub opacity: u32,
    pub focused: u32,
    /// [0]: observed maximize state; [1]: 0=keep, 1=maximize, 2=restore;
    /// [2]: reserved zero. UI4 maximize is borderless fullscreen.
    pub reserved: [u32; 3],
}

#[cfg(target_os = "trueos")]
unsafe extern "C" {
    pub(crate) fn trueos_cabi_ui4_display_open_v1() -> u64;
    pub(crate) fn trueos_cabi_ui4_display_retain_v1(connection: u64) -> i32;
    pub(crate) fn trueos_cabi_ui4_display_close_v1(connection: u64) -> i32;
    pub(crate) fn trueos_cabi_ui4_display_validate_window_v1(connection: u64, window: u32) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_frame_open_streaming(
        x: i32, y: i32, w: u32, h: u32,
    ) -> u32;
    /// Layered Winit contract: triple-buffered scene behind a double-buffered UI.
    pub(crate) fn trueos_cabi_ui4_winit_frame_open_v1(
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        background_hz: u32,
    ) -> u32;
    pub(crate) fn trueos_cabi_ui4_winit_frame_open_scaled_v1(
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        background_hz: u32,
        scene_divisor: u32,
    ) -> u32;
    pub(crate) fn trueos_cabi_ui4_solara_frame_close(id: u32) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_frame_set_position(id: u32, x: i32, y: i32) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_frame_get_position(id: u32, out_xy: *mut i32) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_frame_resize(id: u32, w: u32, h: u32) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_frame_set_escape_key_action(id: u32, action: u32) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_resize_event_take(id: u32, out: *mut ResizeEvent) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_keyboard_event_take_v1(
        id: u32,
        out: *mut KeyboardOutputEvent,
    ) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_pan_event_take(id: u32, out: *mut PanEvent) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_pointer_event_take(id: u32, out: *mut PointerEvent) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_window_state_get_v1(
        id: u32,
        out: *mut WindowStateV1,
    ) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_window_state_set_v1(
        id: u32,
        state: *const WindowStateV1,
    ) -> i32;
    pub(crate) fn trueos_cabi_ui4_scene_window_title_get_v1(
        id: u32,
        out: *mut u8,
        out_cap: usize,
    ) -> isize;
    pub(crate) fn trueos_cabi_ui4_scene_window_title_set_v1(
        id: u32,
        bytes: *const u8,
        len: usize,
    ) -> i32;
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_display_open_v1() -> u64 {
    1
}
#[cfg(not(target_os = "trueos"))]
#[expect(
    dead_code,
    reason = "Arc owns the one host connection reference; retain is exposed for ABI clients"
)]
pub(crate) unsafe fn trueos_cabi_ui4_display_retain_v1(_: u64) -> i32 {
    0
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_display_close_v1(_: u64) -> i32 {
    0
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_display_validate_window_v1(_: u64, _: u32) -> i32 {
    0
}
#[cfg(all(test, not(target_os = "trueos")))]
std::thread_local! {
    pub(crate) static TEST_FRAME_OPEN: std::cell::RefCell<Option<(i32, i32, u32, u32, u32)>> =
        const { std::cell::RefCell::new(None) };
}
#[cfg(all(test, not(target_os = "trueos")))]
std::thread_local! {
    pub(crate) static TEST_STREAMING_FRAME_OPEN: std::cell::RefCell<Option<(i32, i32, u32, u32)>> =
        const { std::cell::RefCell::new(None) };
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_frame_open_streaming(
    x: i32, y: i32, w: u32, h: u32,
) -> u32 {
    #[cfg(test)]
    TEST_STREAMING_FRAME_OPEN.with(|state| *state.borrow_mut() = Some((x,y,w,h)));
    #[cfg(not(test))]
    let _ = (x,y,w,h);
    0
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_winit_frame_open_scaled_v1(
    x: i32, y: i32, w: u32, h: u32, background_hz: u32, _: u32,
) -> u32 {
    unsafe { trueos_cabi_ui4_winit_frame_open_v1(x, y, w, h, background_hz) }
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_winit_frame_open_v1(
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    background_hz: u32,
) -> u32 {
    #[cfg(test)]
    TEST_FRAME_OPEN.with(|state| *state.borrow_mut() = Some((x, y, w, h, background_hz)));
    #[cfg(not(test))]
    let _ = (x, y, w, h, background_hz);
    0
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_solara_frame_close(_: u32) -> i32 {
    -5
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_frame_set_position(_: u32, _: i32, _: i32) -> i32 {
    -5
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_frame_get_position(_: u32, _: *mut i32) -> i32 {
    -5
}
#[cfg(all(test, not(target_os = "trueos")))]
std::thread_local! {
    pub(crate) static TEST_RESIZE: std::cell::RefCell<(Option<ResizeEvent>, i32)> =
        const { std::cell::RefCell::new((None, -5)) };
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_frame_resize(_: u32, _: u32, _: u32) -> i32 {
    #[cfg(test)]
    return TEST_RESIZE.with(|state| state.borrow().1);
    #[cfg(not(test))]
    -5
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_frame_set_escape_key_action(_: u32, _: u32) -> i32 {
    -5
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_resize_event_take(_: u32, out: *mut ResizeEvent) -> i32 {
    #[cfg(test)]
    if let Some(event) = TEST_RESIZE.with(|state| state.borrow_mut().0.take()) {
        unsafe { out.write(event) };
        return 0;
    }
    let _ = out;
    1
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_keyboard_event_take_v1(
    _: u32,
    _: *mut KeyboardOutputEvent,
) -> i32 {
    1
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_pointer_event_take(_: u32, _: *mut PointerEvent) -> i32 {
    1
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_window_state_get_v1(
    _: u32,
    _: *mut WindowStateV1,
) -> i32 {
    -5
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_window_state_set_v1(
    _: u32,
    _: *const WindowStateV1,
) -> i32 {
    -5
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_window_title_get_v1(
    _: u32,
    _: *mut u8,
    _: usize,
) -> isize {
    -5
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_window_title_set_v1(
    _: u32,
    _: *const u8,
    _: usize,
) -> i32 {
    -5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui4_window_state_v1_has_the_audited_layout() {
        assert_eq!(core::mem::size_of::<WindowStateV1>(), 32);
        assert_eq!(core::mem::align_of::<WindowStateV1>(), 4);
    }

    #[test]
    fn ui4_event_records_have_the_expected_c_layouts() {
        assert_eq!(core::mem::size_of::<KeyboardOutputEvent>(), 44);
        assert_eq!(core::mem::size_of::<ResizeEvent>(), 16);
        assert_eq!(core::mem::size_of::<PointerEvent>(), 64);
        assert_eq!(core::mem::size_of::<PanEvent>(), 52);
        assert_eq!(core::mem::size_of::<CursorEvent>(), 56);
    }
}

#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_ui4_scene_pan_event_take(_: u32, _: *mut PanEvent) -> i32 {
    1
}

/// Global HID cursor sequence record. reserved0 bit 0 marks signed relative
/// dx/dy in reserved1/reserved2; x/y are desktop coordinates, not raw deltas.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CursorEvent {
    pub t_ms: u32,
    pub seq: u32,
    pub controller_id: u32,
    pub slot_id: u32,
    pub ep_target: u32,
    pub hid_kind: u8,
    pub reserved0: u8,
    pub reserved1: u16,
    pub buttons_down: u32,
    pub wheel: i16,
    pub reserved2: u16,
    pub x: f64,
    pub y: f64,
    pub flags: u32,
}
#[cfg(target_os = "trueos")]
unsafe extern "C" {
    pub(crate) fn trueos_cabi_input_read_cursor_events_since(
        seq: u64,
        out: *mut CursorEvent,
        cap: u32,
        next: *mut u64,
        dropped: *mut u32,
    ) -> u32;
    pub(crate) fn trueos_cabi_input_read_keyboard_output_since(
        seq: u64,
        out: *mut KeyboardOutputEvent,
        cap: u32,
        next: *mut u64,
        dropped: *mut u32,
    ) -> u32;
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_input_read_cursor_events_since(
    _: u64,
    _: *mut CursorEvent,
    _: u32,
    _: *mut u64,
    _: *mut u32,
) -> u32 {
    0
}
#[cfg(not(target_os = "trueos"))]
pub(crate) unsafe fn trueos_cabi_input_read_keyboard_output_since(
    _: u64,
    _: *mut KeyboardOutputEvent,
    _: u32,
    _: *mut u64,
    _: *mut u32,
) -> u32 {
    0
}
