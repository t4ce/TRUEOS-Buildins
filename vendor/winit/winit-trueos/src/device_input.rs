use crate::{InputDevice, abi, input, input_ext};
use std::collections::BTreeMap;
use winit_core::event::{
    DeviceEvent, DeviceId, ElementState, MouseScrollDelta, RawKeyEvent, WindowEvent,
};

#[derive(Debug, Default)]
pub(crate) struct DeviceInput {
    cursor_seq: u64,
    keyboard_seq: u64,
    cursor_ready: bool,
    keyboard_ready: bool,
    buttons: BTreeMap<DeviceId, u32>,
    keyboard: input::KeyboardState,
    enabled: bool,
}
impl DeviceInput {
    fn cursor(&mut self, raw: &abi::CursorEvent) -> Vec<(DeviceId, DeviceEvent)> {
        let id = input_ext::device_id(InputDevice {
            controller_id: raw.controller_id,
            slot_id: raw.slot_id,
            ep_target: raw.ep_target,
            hid_kind: u32::from(raw.hid_kind),
        });
        let mut events = Vec::new();
        let lost = raw.flags & (1 << 31) != 0;
        let down = if lost { 0 } else { raw.buttons_down };
        let previous = self.buttons.insert(id, down).unwrap_or(0);
        if lost {
            self.buttons.remove(&id);
        }
        // Only explicitly supplied, unbounded relative reports qualify as raw motion.
        if !lost && raw.reserved0 & 1 != 0 && raw.flags & 1 != 0 {
            events.push((
                id,
                DeviceEvent::PointerMotion {
                    delta: (f64::from(raw.reserved1 as i16), f64::from(raw.reserved2 as i16)),
                },
            ));
        }
        if !lost && raw.wheel != 0 {
            events.push((
                id,
                DeviceEvent::MouseWheel {
                    delta: MouseScrollDelta::LineDelta(0.0, f32::from(raw.wheel)),
                },
            ));
        }
        for bit in 0..32 {
            if (previous ^ down) & (1 << bit) != 0 {
                events.push((
                    id,
                    DeviceEvent::Button {
                        button: bit + 1,
                        state: if down & (1 << bit) != 0 {
                            ElementState::Pressed
                        } else {
                            ElementState::Released
                        },
                    },
                ));
            }
        }
        events
    }
    fn keyboard(&mut self, raw: &abi::KeyboardOutputEvent) -> Vec<(DeviceId, DeviceEvent)> {
        if !matches!(raw.kind, 3 | 4) {
            return Vec::new();
        }
        self.keyboard
            .translate(raw)
            .into_iter()
            .filter_map(|event| {
                if let WindowEvent::KeyboardInput { device_id: Some(id), event, .. } = event {
                    Some((
                        id,
                        DeviceEvent::Key(RawKeyEvent {
                            physical_key: event.physical_key,
                            state: event.state,
                        }),
                    ))
                } else {
                    None
                }
            })
            .collect()
    }
    fn reset_buttons(&mut self) -> Vec<(DeviceId, DeviceEvent)> {
        let mut events = Vec::new();
        for (id, down) in std::mem::take(&mut self.buttons) {
            for bit in 0..32 {
                if down & (1 << bit) != 0 {
                    events.push((
                        id,
                        DeviceEvent::Button { button: bit + 1, state: ElementState::Released },
                    ));
                }
            }
        }
        events
    }
    pub(crate) fn poll(&mut self, enabled: bool) -> Vec<(DeviceId, DeviceEvent)> {
        let mut events = Vec::new();
        if self.enabled && !enabled {
            self.buttons.clear();
            self.keyboard = input::KeyboardState::default();
        }
        self.enabled = enabled;
        // Independent sequence cursors: never pop the host's shared queues.
        // A bounded batch keeps busy devices from starving redraws. Initial backlog
        // is discarded until each reader catches up, avoiding historical key presses.
        let mut cursor = [abi::CursorEvent::default(); 256];
        let mut dropped = 0;
        let cursor_started = std::time::Instant::now();
        let count = unsafe {
            abi::trueos_cabi_input_read_cursor_events_since(
                self.cursor_seq,
                cursor.as_mut_ptr(),
                256,
                &mut self.cursor_seq,
                &mut dropped,
            )
        }
        .min(256) as usize;
        crate::event_loop::report_slow_stage("raw cursor read", cursor_started);
        if dropped != 0 {
            events.extend(self.reset_buttons());
            tracing::warn!(dropped, "TRUEOS raw cursor reader lost records");
        }
        if self.cursor_ready && enabled {
            for raw in &cursor[..count] {
                events.extend(self.cursor(raw));
            }
        }
        if count < 256 {
            self.cursor_ready = true;
        }
        let mut keyboard = [abi::KeyboardOutputEvent::default(); 256];
        dropped = 0;
        let keyboard_started = std::time::Instant::now();
        let count = unsafe {
            abi::trueos_cabi_input_read_keyboard_output_since(
                self.keyboard_seq,
                keyboard.as_mut_ptr(),
                256,
                &mut self.keyboard_seq,
                &mut dropped,
            )
        }
        .min(256) as usize;
        crate::event_loop::report_slow_stage("raw keyboard read", keyboard_started);
        if dropped != 0 {
            events
                .extend(self.keyboard(&abi::KeyboardOutputEvent { kind: 3, ..Default::default() }));
            tracing::warn!(dropped, "TRUEOS raw keyboard reader lost records");
        }
        if self.keyboard_ready && enabled {
            for raw in &keyboard[..count] {
                events.extend(self.keyboard(raw));
            }
        }
        if count < 256 {
            self.keyboard_ready = true;
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_motion_uses_relative_report_even_at_desktop_edge() {
        let mut state = DeviceInput::default();
        let raw = abi::CursorEvent {
            hid_kind: 2,
            x: 0.0,
            y: 0.0,
            flags: 1,
            reserved0: 1,
            reserved1: (-50i16) as u16,
            reserved2: 20,
            ..Default::default()
        };
        let first = state.cursor(&raw);
        let second = state.cursor(&raw);
        assert!(matches!(first[0].1, DeviceEvent::PointerMotion { delta: (-50.0, 20.0) }));
        assert_eq!(first, second);
        assert!(state.cursor(&abi::CursorEvent { reserved0: 0, ..raw }).is_empty());
    }
    #[test]
    fn buttons_and_unplug_are_per_device() {
        let mut state = DeviceInput::default();
        for slot_id in [1, 2] {
            state.cursor(&abi::CursorEvent {
                slot_id,
                buttons_down: 1 << 31,
                ..Default::default()
            });
        }
        let lost =
            state.cursor(&abi::CursorEvent { slot_id: 1, flags: 1 << 31, ..Default::default() });
        assert_eq!(lost.len(), 1);
        assert!(matches!(
            lost[0].1,
            DeviceEvent::Button { button: 32, state: ElementState::Released }
        ));
        assert_eq!(state.buttons.len(), 1);
        assert_eq!(state.reset_buttons().len(), 1);
    }
}
