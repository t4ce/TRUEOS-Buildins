use std::collections::VecDeque;
use std::sync::Mutex;

use crate::{KeyboardOutputEvent, PanEvent, PointerEvent};
use winit_core::event::DeviceId;

/// Full endpoint identity. HID kind is 0 (virtual cursor), 1 (keyboard),
/// 2 (mouse), 3 (tablet), or 4 (eye tracker). Unknown kinds are preserved.
/// IDs are process-local and remain stable across windows and combo rebinding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputDevice {
    pub controller_id: u32,
    pub slot_id: u32,
    pub ep_target: u32,
    pub hid_kind: u32,
}

impl InputDevice {
    pub(crate) fn keyboard(controller_id: u32, slot_id: u32, ep_target: u32) -> Self {
        Self { controller_id, slot_id, ep_target, hid_kind: 1 }
    }
}

// Intern rather than truncate/hash the four full-width identifiers. Entries are retained
// so an unplugged device's ID is never reassigned to a different endpoint.
static DEVICES: Mutex<Vec<InputDevice>> = Mutex::new(Vec::new());

pub(crate) fn device_id(source: InputDevice) -> DeviceId {
    let mut devices = DEVICES.lock().unwrap();
    let index = devices.iter().position(|entry| *entry == source).unwrap_or_else(|| {
        devices.push(source);
        devices.len() - 1
    });
    DeviceId::from_raw(index as i64 + 1)
}

/// Resolve a TRUEOS Winit device ID to its full native endpoint.
/// Returns `None` for IDs not assigned by this backend.
pub fn device_source(id: DeviceId) -> Option<InputDevice> {
    let index = usize::try_from(id.into_raw().checked_sub(1)?).ok()?;
    DEVICES.lock().unwrap().get(index).copied()
}

pub(crate) fn keyboard_device(raw: &KeyboardOutputEvent) -> DeviceId {
    device_id(InputDevice::keyboard(raw.controller_id, raw.slot_id, raw.ep_target))
}
pub(crate) fn pointer_device(raw: &PointerEvent) -> DeviceId {
    device_id(InputDevice {
        controller_id: raw.controller_id,
        slot_id: raw.slot_id,
        ep_target: raw.ep_target,
        hid_kind: raw.hid_kind,
    })
}

/// Routed UI4 records, captured before translation to standard window events.
/// Pointer records include combo ID, virtual-cursor flag, deltas, and all 32 buttons.
/// Keyboard records include device identity, sequence, HID usage, text, repeat,
/// and device-loss/reset markers. Keyboard combo membership can be resolved
/// against `hid::input_combos()`; the keyboard ABI does not carry a combo ID.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum InputEvent {
    Pointer(PointerEvent),
    Keyboard(KeyboardOutputEvent),
    Pan(PanEvent),
}
impl InputEvent {
    pub fn device_id(&self) -> DeviceId {
        match self {
            Self::Pointer(raw) => pointer_device(raw),
            Self::Pan(raw) => device_id(InputDevice {
                controller_id: raw.controller_id,
                slot_id: raw.slot_id,
                ep_target: raw.ep_target,
                hid_kind: raw.hid_kind,
            }),
            Self::Keyboard(raw) => keyboard_device(raw),
        }
    }
}

/// Events ordered as consumed by the backend (keyboard, pointer, then pan per iteration).
/// These separate UI4 queues do not provide a shared chronological sequence.
#[derive(Debug, Default)]
pub struct InputBatch {
    pub events: Vec<InputEvent>,
    /// Oldest records lost since the last drain. On loss, reset held state and
    /// resample native HID state rather than assuming all transitions arrived.
    pub dropped: u64,
}

#[derive(Debug, Default)]
pub(crate) struct InputQueue {
    enabled: bool,
    events: VecDeque<InputEvent>,
    dropped: u64,
}
impl InputQueue {
    pub(crate) fn enable(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.events.clear();
            self.dropped = 0;
        }
    }
    pub(crate) fn push(&mut self, event: InputEvent) {
        if !self.enabled {
            return;
        }
        if self.events.len() == 1024 {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back(event);
    }
    pub(crate) fn take(&mut self) -> InputBatch {
        InputBatch {
            events: self.events.drain(..).collect(),
            dropped: std::mem::take(&mut self.dropped),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_endpoint_identity_survives_and_is_shared_across_windows() {
        let source = InputDevice::keyboard(u32::MAX, 2, 3);
        let id = device_id(source);
        assert_eq!(device_source(id), Some(source));
        assert_eq!(device_id(source), id);
        assert_ne!(device_id(InputDevice::keyboard(0, 2, 3)), id);
        assert_ne!(device_id(InputDevice { hid_kind: 2, ..source }), id);
        assert_eq!(device_source(DeviceId::from_raw(0)), None);
    }
    #[test]
    fn capture_is_opt_in_bounded_and_reports_loss() {
        let mut queue = InputQueue::default();
        queue.push(InputEvent::Pointer(PointerEvent::default()));
        assert!(queue.take().events.is_empty());
        queue.enable(true);
        for combo_id in 0..1026 {
            queue.push(InputEvent::Pointer(PointerEvent { combo_id, ..Default::default() }));
        }
        let batch = queue.take();
        assert_eq!(batch.dropped, 2);
        assert_eq!(batch.events.len(), 1024);
        assert!(matches!(batch.events[0], InputEvent::Pointer(raw) if raw.combo_id == 2));
        assert_eq!(queue.take().dropped, 0);
        queue.push(InputEvent::Keyboard(KeyboardOutputEvent::default()));
        queue.enable(false);
        assert!(queue.take().events.is_empty());
    }
}
