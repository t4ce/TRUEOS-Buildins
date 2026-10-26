use dpi::PhysicalPosition;
use winit_core::event::{
    ButtonSource, ElementState, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, PointerSource,
    TouchPhase, WindowEvent,
};
use winit_core::keyboard::{
    Key, KeyCode, KeyLocation, ModifiersState, NamedKey, NativeKey, NativeKeyCode, PhysicalKey,
};

use crate::abi::{KeyboardOutputEvent, PointerEvent};

#[allow(dead_code)] // Used by the physical-key v1 conversion on TRUEOS builds.
pub(crate) fn modifiers(bits: u8) -> Modifiers {
    let mut state = ModifiersState::empty();
    state.set(ModifiersState::CONTROL, bits & 0x11 != 0);
    state.set(ModifiersState::SHIFT, bits & 0x22 != 0);
    state.set(ModifiersState::ALT, bits & 0x44 != 0);
    state.set(ModifiersState::META, bits & 0x88 != 0);
    state.into()
}

fn named_key(code: u16) -> Option<NamedKey> {
    Some(match code {
        1 => NamedKey::Backspace,
        2 => NamedKey::Tab,
        3 => NamedKey::Enter,
        4 => NamedKey::Escape,
        6 => NamedKey::Delete,
        7 => NamedKey::Insert,
        8 => NamedKey::Home,
        9 => NamedKey::End,
        10 => NamedKey::PageUp,
        11 => NamedKey::PageDown,
        12 => NamedKey::ArrowUp,
        13 => NamedKey::ArrowDown,
        14 => NamedKey::ArrowLeft,
        15 => NamedKey::ArrowRight,
        16 => NamedKey::Meta,
        17 => NamedKey::PrintScreen,
        101 => NamedKey::F1,
        102 => NamedKey::F2,
        103 => NamedKey::F3,
        104 => NamedKey::F4,
        105 => NamedKey::F5,
        106 => NamedKey::F6,
        107 => NamedKey::F7,
        108 => NamedKey::F8,
        109 => NamedKey::F9,
        110 => NamedKey::F10,
        111 => NamedKey::F11,
        112 => NamedKey::F12,
        _ => return None,
    })
}

pub(crate) fn keyboard_event(raw: &KeyboardOutputEvent) -> Option<WindowEvent> {
    if raw.kind == 4 {
        return physical_event(raw);
    }
    if !matches!(raw.kind, 1 | 2) || raw.flags & (1 << 31) != 0 {
        return None;
    }
    let state = if raw.flags & 1 != 0 { ElementState::Pressed } else { ElementState::Released };
    let bytes = raw.utf8.get(..usize::from(raw.utf8_len))?;
    let decoded = std::str::from_utf8(bytes).ok()?;
    // Named keys with no text are encoded as a NUL by the current host producer.
    let text = if raw.kind == 2 && raw.codepoint == 0 || decoded.is_empty() {
        None
    } else {
        Some(decoded.into())
    };
    let logical_key = if raw.kind == 2 {
        if raw.key_code == 5 {
            Key::Character(" ".into())
        } else {
            named_key(raw.key_code)
                .map(Key::Named)
                .unwrap_or(Key::Unidentified(NativeKey::Unidentified))
        }
    } else {
        Key::Character(text.clone()?)
    };
    // The ABI carries committed text/custom named codes, not HID usages or an unmodified layout
    // key.
    let key_without_modifiers = if raw.kind == 2 {
        logical_key.clone()
    } else {
        Key::Unidentified(NativeKey::Unidentified)
    };
    let text = if state.is_pressed() { text } else { None };
    Some(WindowEvent::KeyboardInput {
        device_id: Some(crate::input_ext::keyboard_device(raw)),
        event: KeyEvent {
            physical_key: PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
            logical_key,
            text: text.clone(),
            location: KeyLocation::Standard,
            state,
            repeat: false,
            text_with_all_modifiers: text,
            key_without_modifiers,
        },
        is_synthetic: raw.flags & 2 != 0,
    })
}

pub fn scancode_to_physicalkey(scancode: u32) -> PhysicalKey {
    u16::try_from(scancode)
        .ok()
        .map(physical_key)
        .unwrap_or(PhysicalKey::Unidentified(NativeKeyCode::Unidentified))
}

pub fn physicalkey_to_scancode(key: PhysicalKey) -> Option<u32> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    let usage = match code {
        KeyCode::KeyA => 4,
        KeyCode::KeyB => 5,
        KeyCode::KeyC => 6,
        KeyCode::KeyD => 7,
        KeyCode::KeyE => 8,
        KeyCode::KeyF => 9,
        KeyCode::KeyG => 10,
        KeyCode::KeyH => 11,
        KeyCode::KeyI => 12,
        KeyCode::KeyJ => 13,
        KeyCode::KeyK => 14,
        KeyCode::KeyL => 15,
        KeyCode::KeyM => 16,
        KeyCode::KeyN => 17,
        KeyCode::KeyO => 18,
        KeyCode::KeyP => 19,
        KeyCode::KeyQ => 20,
        KeyCode::KeyR => 21,
        KeyCode::KeyS => 22,
        KeyCode::KeyT => 23,
        KeyCode::KeyU => 24,
        KeyCode::KeyV => 25,
        KeyCode::KeyW => 26,
        KeyCode::KeyX => 27,
        KeyCode::KeyY => 28,
        KeyCode::KeyZ => 29,
        KeyCode::Digit1 => 30,
        KeyCode::Digit2 => 31,
        KeyCode::Digit3 => 32,
        KeyCode::Digit4 => 33,
        KeyCode::Digit5 => 34,
        KeyCode::Digit6 => 35,
        KeyCode::Digit7 => 36,
        KeyCode::Digit8 => 37,
        KeyCode::Digit9 => 38,
        KeyCode::Digit0 => 39,
        KeyCode::Enter => 40,
        KeyCode::Escape => 41,
        KeyCode::Backspace => 42,
        KeyCode::Tab => 43,
        KeyCode::Space => 44,
        KeyCode::Minus => 45,
        KeyCode::Equal => 46,
        KeyCode::BracketLeft => 47,
        KeyCode::BracketRight => 48,
        KeyCode::Backslash => 49,
        KeyCode::Semicolon => 51,
        KeyCode::Quote => 52,
        KeyCode::Backquote => 53,
        KeyCode::Comma => 54,
        KeyCode::Period => 55,
        KeyCode::Slash => 56,
        KeyCode::CapsLock => 57,
        KeyCode::F1 => 58,
        KeyCode::F2 => 59,
        KeyCode::F3 => 60,
        KeyCode::F4 => 61,
        KeyCode::F5 => 62,
        KeyCode::F6 => 63,
        KeyCode::F7 => 64,
        KeyCode::F8 => 65,
        KeyCode::F9 => 66,
        KeyCode::F10 => 67,
        KeyCode::F11 => 68,
        KeyCode::F12 => 69,
        KeyCode::PrintScreen => 70,
        KeyCode::ScrollLock => 71,
        KeyCode::Pause => 72,
        KeyCode::Insert => 73,
        KeyCode::Home => 74,
        KeyCode::PageUp => 75,
        KeyCode::Delete => 76,
        KeyCode::End => 77,
        KeyCode::PageDown => 78,
        KeyCode::ArrowRight => 79,
        KeyCode::ArrowLeft => 80,
        KeyCode::ArrowDown => 81,
        KeyCode::ArrowUp => 82,
        KeyCode::NumLock => 83,
        KeyCode::NumpadDivide => 84,
        KeyCode::NumpadMultiply => 85,
        KeyCode::NumpadSubtract => 86,
        KeyCode::NumpadAdd => 87,
        KeyCode::NumpadEnter => 88,
        KeyCode::Numpad1 => 89,
        KeyCode::Numpad2 => 90,
        KeyCode::Numpad3 => 91,
        KeyCode::Numpad4 => 92,
        KeyCode::Numpad5 => 93,
        KeyCode::Numpad6 => 94,
        KeyCode::Numpad7 => 95,
        KeyCode::Numpad8 => 96,
        KeyCode::Numpad9 => 97,
        KeyCode::Numpad0 => 98,
        KeyCode::NumpadDecimal => 99,
        KeyCode::IntlBackslash => 100,
        KeyCode::ContextMenu => 101,
        KeyCode::Power => 102,
        KeyCode::NumpadEqual => 103,
        KeyCode::F13 => 104,
        KeyCode::F14 => 105,
        KeyCode::F15 => 106,
        KeyCode::F16 => 107,
        KeyCode::F17 => 108,
        KeyCode::F18 => 109,
        KeyCode::F19 => 110,
        KeyCode::F20 => 111,
        KeyCode::F21 => 112,
        KeyCode::F22 => 113,
        KeyCode::F23 => 114,
        KeyCode::F24 => 115,
        KeyCode::ControlLeft => 224,
        KeyCode::ShiftLeft => 225,
        KeyCode::AltLeft => 226,
        KeyCode::MetaLeft => 227,
        KeyCode::ControlRight => 228,
        KeyCode::ShiftRight => 229,
        KeyCode::AltRight => 230,
        KeyCode::MetaRight => 231,
        _ => return None,
    };
    Some(usage)
}

fn physical_key(usage: u16) -> PhysicalKey {
    let code = match usage {
        4 => KeyCode::KeyA,
        5 => KeyCode::KeyB,
        6 => KeyCode::KeyC,
        7 => KeyCode::KeyD,
        8 => KeyCode::KeyE,
        9 => KeyCode::KeyF,
        10 => KeyCode::KeyG,
        11 => KeyCode::KeyH,
        12 => KeyCode::KeyI,
        13 => KeyCode::KeyJ,
        14 => KeyCode::KeyK,
        15 => KeyCode::KeyL,
        16 => KeyCode::KeyM,
        17 => KeyCode::KeyN,
        18 => KeyCode::KeyO,
        19 => KeyCode::KeyP,
        20 => KeyCode::KeyQ,
        21 => KeyCode::KeyR,
        22 => KeyCode::KeyS,
        23 => KeyCode::KeyT,
        24 => KeyCode::KeyU,
        25 => KeyCode::KeyV,
        26 => KeyCode::KeyW,
        27 => KeyCode::KeyX,
        28 => KeyCode::KeyY,
        29 => KeyCode::KeyZ,
        30 => KeyCode::Digit1,
        31 => KeyCode::Digit2,
        32 => KeyCode::Digit3,
        33 => KeyCode::Digit4,
        34 => KeyCode::Digit5,
        35 => KeyCode::Digit6,
        36 => KeyCode::Digit7,
        37 => KeyCode::Digit8,
        38 => KeyCode::Digit9,
        39 => KeyCode::Digit0,
        40 => KeyCode::Enter,
        41 => KeyCode::Escape,
        42 => KeyCode::Backspace,
        43 => KeyCode::Tab,
        44 => KeyCode::Space,
        45 => KeyCode::Minus,
        46 => KeyCode::Equal,
        47 => KeyCode::BracketLeft,
        48 => KeyCode::BracketRight,
        49 => KeyCode::Backslash,
        51 => KeyCode::Semicolon,
        52 => KeyCode::Quote,
        53 => KeyCode::Backquote,
        54 => KeyCode::Comma,
        55 => KeyCode::Period,
        56 => KeyCode::Slash,
        57 => KeyCode::CapsLock,
        58 => KeyCode::F1,
        59 => KeyCode::F2,
        60 => KeyCode::F3,
        61 => KeyCode::F4,
        62 => KeyCode::F5,
        63 => KeyCode::F6,
        64 => KeyCode::F7,
        65 => KeyCode::F8,
        66 => KeyCode::F9,
        67 => KeyCode::F10,
        68 => KeyCode::F11,
        69 => KeyCode::F12,
        70 => KeyCode::PrintScreen,
        71 => KeyCode::ScrollLock,
        72 => KeyCode::Pause,
        73 => KeyCode::Insert,
        74 => KeyCode::Home,
        75 => KeyCode::PageUp,
        76 => KeyCode::Delete,
        77 => KeyCode::End,
        78 => KeyCode::PageDown,
        79 => KeyCode::ArrowRight,
        80 => KeyCode::ArrowLeft,
        81 => KeyCode::ArrowDown,
        82 => KeyCode::ArrowUp,
        83 => KeyCode::NumLock,
        84 => KeyCode::NumpadDivide,
        85 => KeyCode::NumpadMultiply,
        86 => KeyCode::NumpadSubtract,
        87 => KeyCode::NumpadAdd,
        88 => KeyCode::NumpadEnter,
        89 => KeyCode::Numpad1,
        90 => KeyCode::Numpad2,
        91 => KeyCode::Numpad3,
        92 => KeyCode::Numpad4,
        93 => KeyCode::Numpad5,
        94 => KeyCode::Numpad6,
        95 => KeyCode::Numpad7,
        96 => KeyCode::Numpad8,
        97 => KeyCode::Numpad9,
        98 => KeyCode::Numpad0,
        99 => KeyCode::NumpadDecimal,
        100 => KeyCode::IntlBackslash,
        101 => KeyCode::ContextMenu,
        102 => KeyCode::Power,
        103 => KeyCode::NumpadEqual,
        104 => KeyCode::F13,
        105 => KeyCode::F14,
        106 => KeyCode::F15,
        107 => KeyCode::F16,
        108 => KeyCode::F17,
        109 => KeyCode::F18,
        110 => KeyCode::F19,
        111 => KeyCode::F20,
        112 => KeyCode::F21,
        113 => KeyCode::F22,
        114 => KeyCode::F23,
        115 => KeyCode::F24,
        224 => KeyCode::ControlLeft,
        225 => KeyCode::ShiftLeft,
        226 => KeyCode::AltLeft,
        227 => KeyCode::MetaLeft,
        228 => KeyCode::ControlRight,
        229 => KeyCode::ShiftRight,
        230 => KeyCode::AltRight,
        231 => KeyCode::MetaRight,
        _ => return PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
    };
    PhysicalKey::Code(code)
}

fn physical_named(usage: u16) -> Option<NamedKey> {
    Some(match usage {
        40 | 88 => NamedKey::Enter,
        41 => NamedKey::Escape,
        42 => NamedKey::Backspace,
        43 => NamedKey::Tab,
        57 => NamedKey::CapsLock,
        58..=69 => return named_key(101 + usage - 58),
        70 => NamedKey::PrintScreen,
        71 => NamedKey::ScrollLock,
        72 => NamedKey::Pause,
        73 => NamedKey::Insert,
        74 => NamedKey::Home,
        75 => NamedKey::PageUp,
        76 => NamedKey::Delete,
        77 => NamedKey::End,
        78 => NamedKey::PageDown,
        79 => NamedKey::ArrowRight,
        80 => NamedKey::ArrowLeft,
        81 => NamedKey::ArrowDown,
        82 => NamedKey::ArrowUp,
        83 => NamedKey::NumLock,
        101 => NamedKey::ContextMenu,
        224 | 228 => NamedKey::Control,
        225 | 229 => NamedKey::Shift,
        226 | 230 => NamedKey::Alt,
        227 | 231 => NamedKey::Meta,
        _ => return None,
    })
}

fn physical_event(raw: &KeyboardOutputEvent) -> Option<WindowEvent> {
    let state = if raw.flags & 1 != 0 { ElementState::Pressed } else { ElementState::Released };
    let text: Option<winit_core::keyboard::SmolStr> = if state.is_pressed() && raw.utf8_len != 0 {
        Some(std::str::from_utf8(raw.utf8.get(..usize::from(raw.utf8_len))?).ok()?.into())
    } else {
        None
    };
    let logical_key = physical_named(raw.key_code)
        .map(Key::Named)
        .or_else(|| text.clone().map(Key::Character))
        .unwrap_or(Key::Unidentified(NativeKey::Unidentified));
    let location = match raw.key_code {
        224..=227 => KeyLocation::Left,
        228..=231 => KeyLocation::Right,
        83..=99 | 103 => KeyLocation::Numpad,
        _ => KeyLocation::Standard,
    };
    Some(WindowEvent::KeyboardInput {
        device_id: Some(crate::input_ext::keyboard_device(raw)),
        is_synthetic: raw.flags & 2 != 0,
        event: KeyEvent {
            physical_key: physical_key(raw.key_code),
            logical_key: logical_key.clone(),
            key_without_modifiers: physical_named(raw.key_code)
                .map(Key::Named)
                .unwrap_or(Key::Unidentified(NativeKey::Unidentified)),
            text: text.clone(),
            text_with_all_modifiers: text,
            location,
            state,
            repeat: raw.flags & (1 << 5) != 0,
        },
    })
}

#[derive(Debug, Default)]
pub(crate) struct KeyboardState {
    held: std::collections::BTreeMap<(u32, u32, u32, u16), KeyEvent>,
    modifiers: Modifiers,
    device_modifiers: std::collections::BTreeMap<(u32, u32, u32), u8>,
}

impl KeyboardState {
    pub(crate) fn translate(&mut self, raw: &KeyboardOutputEvent) -> Vec<WindowEvent> {
        let mut events = Vec::new();
        if raw.kind == 3 {
            let all = (raw.controller_id, raw.slot_id, raw.ep_target) == (0, 0, 0);
            self.held.retain(|&(controller, slot, endpoint, _), held| {
                if all
                    || (controller, slot, endpoint)
                        == (raw.controller_id, raw.slot_id, raw.ep_target)
                {
                    let mut event = held.clone();
                    event.state = ElementState::Released;
                    event.repeat = false;
                    event.text = None;
                    event.text_with_all_modifiers = None;
                    events.push(WindowEvent::KeyboardInput {
                        device_id: Some(crate::input_ext::device_id(crate::InputDevice::keyboard(
                            controller, slot, endpoint,
                        ))),
                        event,
                        is_synthetic: true,
                    });
                    false
                } else {
                    true
                }
            });
            self.device_modifiers.retain(|&endpoint, _| {
                !all && endpoint != (raw.controller_id, raw.slot_id, raw.ep_target)
            });
            let next =
                modifiers(self.device_modifiers.values().fold(0, |bits, value| bits | value));
            if self.modifiers != next {
                self.modifiers = next;
                events.push(WindowEvent::ModifiersChanged(next));
            }
            return events;
        }
        let Some(mut translated) = keyboard_event(raw) else {
            return events;
        };
        self.device_modifiers
            .insert((raw.controller_id, raw.slot_id, raw.ep_target), raw.modifiers);
        let next_modifiers =
            modifiers(self.device_modifiers.values().fold(0, |bits, value| bits | value));
        if self.modifiers != next_modifiers {
            self.modifiers = next_modifiers;
            events.push(WindowEvent::ModifiersChanged(next_modifiers));
        }
        if raw.kind == 4 {
            let identity = (raw.controller_id, raw.slot_id, raw.ep_target, raw.key_code);
            if let WindowEvent::KeyboardInput { event, .. } = &mut translated {
                if event.state.is_pressed() {
                    self.held.insert(identity, event.clone());
                } else if let Some(pressed) = self.held.remove(&identity) {
                    event.logical_key = pressed.logical_key;
                    event.key_without_modifiers = pressed.key_without_modifiers;
                } else {
                    // No press was delivered (e.g. after queue-overflow reset).
                    return events;
                }
            }
        }
        events.push(translated);
        events
    }
}

pub(crate) fn pointer_events(raw: &PointerEvent) -> Vec<WindowEvent> {
    let position = PhysicalPosition::new(f64::from(raw.local_x), f64::from(raw.local_y));
    // UI4 exposes the same desktop button bits for virtual cursors, relative
    // mice and absolute tablets (including the remote HID/UDP pointer).
    // Map tablets to standard mouse events so ordinary Winit consumers can
    // click as well as hover. The DeviceId/native record retains HID kind 3.
    let mouse = matches!(raw.hid_kind, 0 | 2 | 3);
    let mut events = vec![WindowEvent::PointerMoved {
        device_id: Some(crate::input_ext::pointer_device(raw)),
        position,
        primary: true,
        source: if mouse { PointerSource::Mouse } else { PointerSource::Unknown },
    }];
    for (bits, state) in [
        (raw.buttons_pressed, ElementState::Pressed),
        (raw.buttons_released, ElementState::Released),
    ] {
        for bit in 0..32u16 {
            if bits & (1u32 << bit) == 0 {
                continue;
            }
            let button = if mouse {
                ButtonSource::Mouse(match bit {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Back,
                    4 => MouseButton::Forward,
                    other => MouseButton::try_from_u8(other as u8).expect("button bit is in 0..32"),
                })
            } else {
                ButtonSource::Unknown(bit + 1)
            };
            events.push(WindowEvent::PointerButton {
                device_id: Some(crate::input_ext::pointer_device(raw)),
                state,
                position,
                primary: true,
                button,
                is_macos_activation_click: false,
            });
        }
    }
    if raw.wheel != 0 {
        events.push(WindowEvent::MouseWheel {
            device_id: Some(crate::input_ext::pointer_device(raw)),
            delta: MouseScrollDelta::LineDelta(0.0, raw.wheel as f32),
            phase: TouchPhase::Moved,
        });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_press_release_preserve_usage_and_logical_key() {
        let mut state = KeyboardState::default();
        let mut raw = KeyboardOutputEvent {
            controller_id: 1,
            slot_id: 2,
            ep_target: 3,
            kind: 4,
            key_code: 4,
            flags: 1,
            utf8_len: 1,
            utf8: [b'A', 0, 0, 0],
            codepoint: b'A' as u32,
            modifiers: 2,
            ..Default::default()
        };
        let down = state.translate(&raw);
        assert!(matches!(&down[1], WindowEvent::KeyboardInput { event, .. }
            if event.physical_key == PhysicalKey::Code(KeyCode::KeyA)
                && event.logical_key == Key::Character("A".into()) && event.state.is_pressed()));
        raw.flags = 0;
        raw.utf8_len = 0;
        raw.codepoint = 0;
        raw.modifiers = 0;
        let up = state.translate(&raw);
        assert!(matches!(&up[1], WindowEvent::KeyboardInput { event, .. }
            if event.physical_key == PhysicalKey::Code(KeyCode::KeyA)
                && event.logical_key == Key::Character("A".into())
                && event.state == ElementState::Released && event.text.is_none()));
        assert!(state.held.is_empty());
    }

    #[test]
    fn loss_releases_only_matching_device_and_overflow_resets_all() {
        let mut state = KeyboardState::default();
        for slot_id in [1, 2] {
            state.translate(&KeyboardOutputEvent {
                controller_id: 9,
                slot_id,
                kind: 4,
                key_code: 225,
                flags: 1,
                modifiers: 2,
                ..Default::default()
            });
        }
        let lost = state.translate(&KeyboardOutputEvent {
            controller_id: 9,
            slot_id: 1,
            kind: 3,
            ..Default::default()
        });
        assert_eq!(state.held.len(), 1);
        assert!(matches!(&lost[0], WindowEvent::KeyboardInput { event, is_synthetic: true, .. }
            if event.state == ElementState::Released && event.location == KeyLocation::Left));
        let reset = state.translate(&KeyboardOutputEvent { kind: 3, ..Default::default() });
        assert!(state.held.is_empty());
        assert!(
            matches!(reset.last(), Some(WindowEvent::ModifiersChanged(m)) if m.state().is_empty())
        );
    }

    #[test]
    fn physical_modifier_only_reports_and_unknown_usages_are_safe() {
        let raw = KeyboardOutputEvent {
            kind: 4,
            key_code: 230,
            flags: 1,
            modifiers: 0x40,
            ..Default::default()
        };
        let Some(WindowEvent::KeyboardInput { event, .. }) = keyboard_event(&raw) else {
            panic!("missing modifier")
        };
        assert_eq!(event.physical_key, PhysicalKey::Code(KeyCode::AltRight));
        assert_eq!(event.logical_key, Key::Named(NamedKey::Alt));
        assert_eq!(event.location, KeyLocation::Right);
        assert_eq!(physical_key(0xffff), PhysicalKey::Unidentified(NativeKeyCode::Unidentified));
    }

    #[test]
    fn concurrent_keyboard_modifiers_and_device_loss_are_independent() {
        let mut state = KeyboardState::default();
        for (slot_id, key_code, modifiers) in [(1, 225, 2), (2, 224, 1)] {
            state.translate(&KeyboardOutputEvent {
                slot_id,
                key_code,
                modifiers,
                kind: 4,
                flags: 1,
                ..Default::default()
            });
        }
        assert!(state.modifiers.state().shift_key());
        assert!(state.modifiers.state().control_key());
        state.translate(&KeyboardOutputEvent { slot_id: 2, kind: 3, ..Default::default() });
        assert!(state.modifiers.state().shift_key());
        assert!(!state.modifiers.state().control_key());
    }

    #[test]
    fn all_mouse_buttons_keep_their_indices_and_device_identity() {
        for hid_kind in [0, 2, 3] {
            for bit in 0..32 {
                let raw = PointerEvent {
                    slot_id: 12,
                    hid_kind,
                    buttons_pressed: 1 << bit,
                    ..Default::default()
                };
                let events = pointer_events(&raw);
                assert!(matches!(events[1], WindowEvent::PointerButton {
                    button: ButtonSource::Mouse(button), device_id: Some(id), ..
                } if button as u8 == bit && crate::device_source(id).unwrap() == crate::InputDevice {
                    controller_id: 0, slot_id: 12, ep_target: 0, hid_kind,
                }));
            }
        }
    }

    #[test]
    fn local_and_remote_pointers_deliver_clicks_at_their_own_positions() {
        let local = PointerEvent {
            controller_id: 1,
            slot_id: 5,
            hid_kind: 2,
            local_x: 12,
            local_y: 34,
            combo_id: 7,
            ..Default::default()
        };
        let remote = PointerEvent {
            controller_id: 0x55445048,
            slot_id: 0x55000001,
            hid_kind: 3,
            local_x: 210,
            local_y: 340,
            combo_id: 8,
            vcursor: 1,
            ..Default::default()
        };
        assert_ne!(
            crate::input_ext::pointer_device(&local),
            crate::input_ext::pointer_device(&remote)
        );
        // Both buttons can be held concurrently without merging the devices.
        for pressed in [true, false] {
            for source in [local, remote] {
                let raw = PointerEvent {
                    buttons_pressed: u32::from(pressed),
                    buttons_released: u32::from(!pressed),
                    ..source
                };
                let events = pointer_events(&raw);
                let expected_id = crate::input_ext::pointer_device(&source);
                let expected_position =
                    PhysicalPosition::new(f64::from(source.local_x), f64::from(source.local_y));
                assert_eq!(events.len(), 2);
                assert!(
                    matches!(events[0], WindowEvent::PointerMoved { device_id: Some(id), position, source: PointerSource::Mouse, .. }
                    if id == expected_id && position == expected_position)
                );
                assert!(
                    matches!(events[1], WindowEvent::PointerButton { device_id: Some(id), position, button: ButtonSource::Mouse(MouseButton::Left), state, .. }
                    if id == expected_id && position == expected_position && state.is_pressed() == pressed)
                );
            }
        }
    }

    #[test]
    fn unknown_pointer_kinds_keep_their_native_buttons() {
        let events = pointer_events(&PointerEvent {
            hid_kind: 99,
            buttons_pressed: 1,
            ..Default::default()
        });
        assert!(matches!(events[0], WindowEvent::PointerMoved {
            source: PointerSource::Unknown,
            ..
        }));
        assert!(matches!(events[1], WindowEvent::PointerButton {
            button: ButtonSource::Unknown(1),
            ..
        }));
    }

    #[test]
    fn pan_phases_deltas_and_unknown_phase() {
        for (phase, expected) in
            [(1, TouchPhase::Started), (2, TouchPhase::Moved), (3, TouchPhase::Ended)]
        {
            let raw = crate::PanEvent { phase, dx: -4, dy: 8, ..Default::default() };
            assert!(
                matches!(pan_event(&raw), Some(WindowEvent::PanGesture { delta, phase, device_id: Some(_), })
                if delta == PhysicalPosition::new(-4.0, 8.0) && phase == expected)
            );
        }
        assert!(pan_event(&crate::PanEvent::default()).is_none());
    }

    #[test]
    fn right_hand_modifiers_are_preserved() {
        assert_eq!(modifiers(0xf0).state(), modifiers(0x0f).state());
        assert!(modifiers(0x40).state().alt_key());
        assert!(!modifiers(0x40).state().control_key());
    }

    #[test]
    fn unicode_text_has_no_invented_physical_key() {
        let raw = KeyboardOutputEvent {
            kind: 1,
            flags: 3,
            utf8_len: 4,
            utf8: [0xf0, 0x9f, 0xa6, 0x80],
            codepoint: 0x1f980,
            ..Default::default()
        };
        let Some(WindowEvent::KeyboardInput { event, is_synthetic, .. }) = keyboard_event(&raw)
        else {
            panic!("missing key event")
        };
        assert_eq!(event.text.as_deref(), Some("🦀"));
        assert_eq!(event.physical_key, PhysicalKey::Unidentified(NativeKeyCode::Unidentified));
        assert!(is_synthetic);
    }

    #[test]
    fn named_key_nul_is_not_inserted_as_text() {
        let raw = KeyboardOutputEvent {
            kind: 2,
            key_code: 12,
            flags: 1,
            utf8_len: 1,
            ..Default::default()
        };
        let Some(WindowEvent::KeyboardInput { event, .. }) = keyboard_event(&raw) else {
            panic!("missing key event")
        };
        assert_eq!(event.logical_key, Key::Named(NamedKey::ArrowUp));
        assert_eq!(event.text, None);
    }

    #[test]
    fn malformed_text_and_device_loss_are_not_keys() {
        for raw in [
            KeyboardOutputEvent { kind: 1, utf8_len: 5, ..Default::default() },
            KeyboardOutputEvent {
                kind: 1,
                utf8_len: 1,
                utf8: [0xff, 0, 0, 0],
                ..Default::default()
            },
            KeyboardOutputEvent { kind: 3, flags: 1 << 31, ..Default::default() },
        ] {
            assert!(keyboard_event(&raw).is_none());
        }
    }

    #[test]
    fn pointer_uses_local_position_and_delivers_releases() {
        let events = pointer_events(&PointerEvent {
            hid_kind: 2,
            local_x: -3,
            local_y: 9,
            x: 700,
            y: 500,
            buttons_pressed: 1,
            buttons_released: 2,
            wheel: -2,
            ..Default::default()
        });
        assert!(
            matches!(events[0], WindowEvent::PointerMoved { position, .. } if position == PhysicalPosition::new(-3.0, 9.0))
        );
        assert!(matches!(events[1], WindowEvent::PointerButton {
            button: ButtonSource::Mouse(MouseButton::Left),
            state: ElementState::Pressed,
            ..
        }));
        assert!(matches!(events[2], WindowEvent::PointerButton {
            button: ButtonSource::Mouse(MouseButton::Right),
            state: ElementState::Released,
            ..
        }));
        assert!(matches!(events[3], WindowEvent::MouseWheel {
            delta: MouseScrollDelta::LineDelta(0.0, -2.0),
            ..
        }));
    }
}

pub(crate) fn pan_event(raw: &crate::PanEvent) -> Option<WindowEvent> {
    let phase = match raw.phase {
        1 => TouchPhase::Started,
        2 => TouchPhase::Moved,
        3 => TouchPhase::Ended,
        _ => return None,
    };
    Some(WindowEvent::PanGesture {
        device_id: Some(crate::InputEvent::Pan(*raw).device_id()),
        delta: PhysicalPosition::new(raw.dx as f32, raw.dy as f32),
        phase,
    })
}
