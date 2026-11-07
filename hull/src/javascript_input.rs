//! One-way Winit input records for the JavaScript business-logic task.
//!
//! Native routed records are authoritative for pointers, keyboard and pan.
//! Standard Winit input translations are deliberately not forwarded a second
//! time. Coordinates retain their native meaning; this adapter does not infer
//! physical monitor distances or connect JavaScript to layout or painting.
use alloc::{string::ToString, vec::Vec, borrow::ToOwned};
use serde_json::{Value, json};
use winit::event::{Ime, WindowEvent};
use winit::platform::trueos::{InputBatch, InputEvent, WindowExtTrueOS};
use winit::window::Window;

/// Keep admission failures visible without logging typed text or pointer payloads.
pub(super) fn forward(runtime: &mut crate::javascript::Runtime, generation: u64, event: Value) {
    if runtime.enqueue_input(generation, event) == crate::javascript::EnqueueResult::Invalid {
        trueos::logl::log(
            trueos::logl::level::ERROR,
            format_args!(
                "hull: JavaScript input rejected: malformed record or message exceeds 16 KiB"
            ),
        );
    }
}

pub(super) fn enable(window: &dyn Window) {
    window.trueos_capture_input(true);
}

pub(super) fn drain(window: &dyn Window) -> Vec<Value> {
    from_batch(window.trueos_window_id(), window.trueos_take_input())
}

/// Preserve backend delivery order when a translated state event follows raw input.
pub(super) fn before_supplemental(window: &dyn Window, event: &WindowEvent) -> Vec<Value> {
    if supplemental(window.trueos_window_id(), event).is_none() {
        return Vec::new();
    }
    from_batch_with_supplemental(window.trueos_window_id(), window.trueos_take_input(), event)
}

pub(super) fn from_batch_with_supplemental(
    window_id: u32,
    batch: InputBatch,
    event: &WindowEvent,
) -> Vec<Value> {
    let mut values = from_batch(window_id, batch);
    if let Some(value) = supplemental(window_id, event) {
        values.push(value);
    }
    values
}

/// Navigation discards any routed records still queued for the old page.
/// This leaves the standard Winit callbacks used by Hull's UI untouched.
pub(super) fn discard(window: &dyn Window) {
    let _ = window.trueos_take_input();
}

/// Preserve every native ABI field, including endpoint identity and raw flags.
/// Keyboard sequence/time fields and opaque device IDs use decimal strings so
/// extending them to 64 bits cannot silently lose integer precision in JS.
pub(super) fn from_batch(window_id: u32, batch: InputBatch) -> Vec<Value> {
    let mut values = Vec::with_capacity(batch.events.len() + usize::from(batch.dropped != 0));
    if batch.dropped != 0 {
        values.push(json!({
            "kind":"input-reset", "window_id":window_id,
            "source":"winit-trueos-routed", "reason":"native-input-overflow",
            "dropped":batch.dropped.to_string(), "reset_all":true,
        }));
    }
    for event in batch.events {
        let device_id = event.device_id().into_raw().to_string();
        let value = match event {
            InputEvent::Pointer(raw) => json!({
                "kind":"pointer", "window_id":window_id, "device_id":device_id,
                "source":"winit-trueos-routed", "coordinate_space":"ui4-native",
                "endpoint":{
                    "controller_id":raw.controller_id,"slot_id":raw.slot_id,
                    "ep_target":raw.ep_target,"hid_kind":raw.hid_kind,
                },
                "raw":{
                    "controller_id":raw.controller_id,"slot_id":raw.slot_id,
                    "ep_target":raw.ep_target,"hid_kind":raw.hid_kind,
                    "x":raw.x,"y":raw.y,"local_x":raw.local_x,"local_y":raw.local_y,
                    "dx":raw.dx,"dy":raw.dy,"wheel":raw.wheel,
                    "buttons_down":raw.buttons_down,"buttons_pressed":raw.buttons_pressed,
                    "buttons_released":raw.buttons_released,
                    "combo_id":raw.combo_id,"vcursor":raw.vcursor,
                },
            }),
            InputEvent::Pan(raw) => json!({
                "kind":"pan", "window_id":window_id, "device_id":device_id,
                "source":"winit-trueos-routed", "coordinate_space":"ui4-native",
                "endpoint":{
                    "controller_id":raw.controller_id,"slot_id":raw.slot_id,
                    "ep_target":raw.ep_target,"hid_kind":raw.hid_kind,
                },
                "raw":{
                    "controller_id":raw.controller_id,"slot_id":raw.slot_id,
                    "ep_target":raw.ep_target,"hid_kind":raw.hid_kind,
                    "phase":raw.phase,"x":raw.x,"y":raw.y,
                    "local_x":raw.local_x,"local_y":raw.local_y,
                    "dx":raw.dx,"dy":raw.dy,"combo_id":raw.combo_id,"vcursor":raw.vcursor,
                },
            }),
            InputEvent::Keyboard(raw) => {
                let decoded = raw
                    .utf8
                    .get(..usize::from(raw.utf8_len))
                    .ok_or_else(|| "utf8_len exceeds native ABI buffer".to_owned())
                    .and_then(|bytes| {
                        core::str::from_utf8(bytes)
                            .map(str::to_owned)
                            .map_err(|error| error.to_string())
                    });
                let valid = decoded.is_ok();
                let error = decoded.as_ref().err();
                // Named-key NUL is an ABI marker, not committed text.
                let text = decoded
                    .as_ref()
                    .ok()
                    .filter(|text| !text.is_empty() && !(raw.kind == 2 && raw.codepoint == 0));
                json!({
                    "kind":"keyboard", "window_id":window_id, "device_id":device_id,
                    "source":"winit-trueos-routed",
                    "endpoint":{
                        "controller_id":raw.controller_id,"slot_id":raw.slot_id,
                        "ep_target":raw.ep_target,"hid_kind":1,
                    },
                    "text":text,"utf8_valid":valid,"utf8_error":error,
                    "device_lost":raw.kind==3,
                    "reset_all_devices":raw.kind==3 && (raw.controller_id,raw.slot_id,raw.ep_target)==(0,0,0),
                    "repeat":raw.flags & (1<<5)!=0, "synthetic":raw.flags & 2!=0,
                    "hid_usage":if raw.kind==4 {Some(raw.key_code)} else {None},
                    "raw":{
                        "t_ms":raw.t_ms.to_string(),"seq":raw.seq.to_string(),"device_seq":raw.device_seq.to_string(),
                        "controller_id":raw.controller_id,"slot_id":raw.slot_id,"ep_target":raw.ep_target,
                        "modifiers":raw.modifiers,"kind":raw.kind,"utf8_len":raw.utf8_len,
                        "reserved0":raw.reserved0,"key_code":raw.key_code,"reserved1":raw.reserved1,
                        "codepoint":raw.codepoint,"utf8":raw.utf8,"flags":raw.flags,
                    },
                })
            }
            _ => json!({
                "kind":"input-reset", "window_id":window_id,
                "source":"winit-trueos-routed", "reason":"unsupported-native-input-variant",
                "reset_all":true,
            }),
        };
        values.push(value);
    }
    values
}

/// Forward window state that has no duplicate native pointer/keyboard record.
/// Committed native keyboard text is carried above; IME is a distinct Winit
/// composition stream, including its UTF-8 byte-indexed preedit cursor range.
pub(super) fn supplemental(window_id: u32, event: &WindowEvent) -> Option<Value> {
    let mut value = match event {
        WindowEvent::Focused(focused) => json!({"kind":"focus","focused":focused}),
        WindowEvent::SurfaceResized(size) => json!({
            "kind":"resize","coordinate_space":"ui4-native","width":size.width,"height":size.height,
        }),
        WindowEvent::ModifiersChanged(modifiers) => {
            let state = modifiers.state();
            json!({"kind":"modifiers","state_bits":state.bits(),
                "shift":state.shift_key(),"control":state.control_key(),"alt":state.alt_key(),"meta":state.meta_key()})
        }
        WindowEvent::Ime(ime) => match ime {
            Ime::Enabled => json!({"kind":"ime","phase":"enabled"}),
            Ime::Disabled => json!({"kind":"ime","phase":"disabled"}),
            Ime::Preedit(text, cursor) => json!({"kind":"ime","phase":"preedit","text":text,
                "cursor_bytes":cursor.map(|(start,end)|[start,end]),"cursor_units":"utf8-bytes"}),
            Ime::Commit(text) => json!({"kind":"ime","phase":"commit","text":text}),
            Ime::DeleteSurrounding {
                before_bytes,
                after_bytes,
            } => json!({"kind":"ime","phase":"delete-surrounding",
                "before_bytes":before_bytes,"after_bytes":after_bytes}),
        },
        _ => return None,
    };
    value["window_id"] = json!(window_id);
    value["source"] = json!("winit-window-state");
    Some(value)
}
