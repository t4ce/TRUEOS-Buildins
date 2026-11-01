use crate::javascript_input::{from_batch, supplemental};
use serde_json::json;
use winit_core::event::{
    DeviceId, Ime, Modifiers, MouseScrollDelta, PointerSource, TouchPhase, WindowEvent,
};
use winit_core::keyboard::{ModifiersKeys, ModifiersState};
use winit_trueos::{
    InputBatch, InputEvent, KeyboardOutputEvent, PanEvent, PointerEvent, device_source,
};

#[test]
fn two_pointers_keep_full_endpoint_identity_and_button_chords() {
    let first = PointerEvent {
        controller_id: u32::MAX,
        slot_id: 0xffff_fffe,
        ep_target: 0x8000_0001,
        hid_kind: 2,
        x: u32::MAX,
        y: 19,
        local_x: -23,
        local_y: 17,
        dx: -5,
        dy: 6,
        wheel: -120,
        buttons_down: 0x8000_0005,
        buttons_pressed: 0x8000_0001,
        buttons_released: 2,
        combo_id: u32::MAX,
        vcursor: 1,
    };
    let second = PointerEvent {
        slot_id: 0,
        buttons_down: 2,
        buttons_pressed: 2,
        buttons_released: 0,
        ..first
    };
    let values = from_batch(
        0xffff_fffe,
        InputBatch {
            events: vec![
                InputEvent::Pointer(first),
                InputEvent::Pointer(second),
                InputEvent::Pointer(first),
            ],
            dropped: 0,
        },
    );
    assert_eq!(values.len(), 3);
    assert_eq!(values[0]["device_id"], values[2]["device_id"]);
    assert_ne!(values[0]["device_id"], values[1]["device_id"]);
    let id = DeviceId::from_raw(values[0]["device_id"].as_str().unwrap().parse().unwrap());
    let endpoint = device_source(id).unwrap();
    assert_eq!(
        (
            endpoint.controller_id,
            endpoint.slot_id,
            endpoint.ep_target,
            endpoint.hid_kind
        ),
        (u32::MAX, 0xffff_fffe, 0x8000_0001, 2)
    );
    assert_eq!(values[0]["window_id"], json!(0xffff_fffeu32));
    assert_eq!(values[0]["coordinate_space"], "ui4-native");
    assert_eq!(values[0]["raw"]["buttons_down"], json!(0x8000_0005u32));
    assert_eq!(values[0]["raw"]["buttons_pressed"], json!(0x8000_0001u32));
    assert_eq!(values[0]["raw"]["buttons_released"], 2);
    assert_eq!(values[0]["raw"]["combo_id"], json!(u32::MAX));
    assert_eq!(values[0]["raw"]["local_x"], -23);
    assert_eq!(values[0]["raw"]["wheel"], -120);
    assert_eq!(values[0]["raw"]["vcursor"], 1);
}

#[test]
fn keyboard_preserves_utf8_modifiers_raw_flags_and_decimal_sequence_fields() {
    let raw = KeyboardOutputEvent {
        t_ms: u32::MAX,
        seq: u32::MAX - 1,
        device_seq: u32::MAX - 2,
        controller_id: 17,
        slot_id: u32::MAX,
        ep_target: 0x8000_0000,
        modifiers: 0xff,
        kind: 4,
        utf8_len: 4,
        reserved0: 7,
        key_code: 0x1234,
        reserved1: 0xabcd,
        codepoint: 0x1f600,
        utf8: [0xf0, 0x9f, 0x98, 0x80],
        flags: (1 << 5) | 2,
    };
    let values = from_batch(
        7,
        InputBatch {
            events: vec![InputEvent::Keyboard(raw)],
            dropped: 0,
        },
    );
    let event = &values[0];
    assert_eq!(event["kind"], "keyboard");
    assert_eq!(event["text"], "😀");
    assert_eq!(event["utf8_valid"], true);
    assert_eq!(event["raw"]["modifiers"], 255);
    assert_eq!(event["raw"]["seq"], json!((u32::MAX - 1).to_string()));
    assert_eq!(
        event["raw"]["device_seq"],
        json!((u32::MAX - 2).to_string())
    );
    assert_eq!(event["raw"]["t_ms"], json!(u32::MAX.to_string()));
    assert_eq!(event["raw"]["utf8"], json!([0xf0, 0x9f, 0x98, 0x80]));
    assert_eq!(event["raw"]["reserved0"], 7);
    assert_eq!(event["raw"]["reserved1"], 0xabcd);
    assert_eq!(event["hid_usage"], 0x1234);
    assert_eq!(event["repeat"], true);
    assert_eq!(event["synthetic"], true);
    assert_eq!(event["endpoint"]["ep_target"], json!(0x8000_0000u32));
}

#[test]
fn invalid_keyboard_utf8_and_device_loss_stay_explicit() {
    let invalid = KeyboardOutputEvent {
        utf8_len: 1,
        utf8: [0xff, 0, 0, 0],
        ..Default::default()
    };
    let too_long = KeyboardOutputEvent {
        utf8_len: 5,
        ..Default::default()
    };
    let reset = KeyboardOutputEvent {
        kind: 3,
        ..Default::default()
    };
    let values = from_batch(
        3,
        InputBatch {
            events: vec![
                InputEvent::Keyboard(invalid),
                InputEvent::Keyboard(too_long),
                InputEvent::Keyboard(reset),
            ],
            dropped: 0,
        },
    );
    assert_eq!(values[0]["utf8_valid"], false);
    assert!(values[0]["text"].is_null());
    assert_eq!(values[0]["raw"]["utf8"][0], 255);
    assert!(values[0]["utf8_error"].is_string());
    assert_eq!(values[1]["utf8_valid"], false);
    assert!(values[1]["utf8_error"].as_str().unwrap().contains("buffer"));
    assert_eq!(values[2]["device_lost"], true);
    assert_eq!(values[2]["reset_all_devices"], true);
}

#[test]
fn native_loss_reset_precedes_remaining_ordered_records() {
    let values = from_batch(
        9,
        InputBatch {
            events: vec![
                InputEvent::Keyboard(Default::default()),
                InputEvent::Pointer(Default::default()),
                InputEvent::Pan(PanEvent {
                    phase: 2,
                    combo_id: 4,
                    dx: -9,
                    vcursor: 1,
                    ..Default::default()
                }),
            ],
            dropped: u64::MAX,
        },
    );
    assert_eq!(
        values
            .iter()
            .map(|event| event["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["input-reset", "keyboard", "pointer", "pan"]
    );
    assert_eq!(values[0]["dropped"], json!(u64::MAX.to_string()));
    assert_eq!(values[0]["reset_all"], true);
    assert_eq!(values[3]["raw"]["phase"], 2);
    assert_eq!(values[3]["raw"]["dx"], -9);
    assert_eq!(values[3]["raw"]["combo_id"], 4);
}

#[test]
fn supplemental_state_preserves_ime_byte_ranges_and_avoids_duplicate_pointer_events() {
    let preedit = supplemental(
        12,
        &WindowEvent::Ime(Ime::Preedit("😀x".into(), Some((0, 4)))),
    )
    .unwrap();
    assert_eq!(preedit["cursor_bytes"], json!([0, 4]));
    assert_eq!(preedit["cursor_units"], "utf8-bytes");
    assert_eq!(preedit["text"], "😀x");
    let commit = supplemental(12, &WindowEvent::Ime(Ime::Commit("é".into()))).unwrap();
    assert_eq!(commit["phase"], "commit");
    assert_eq!(commit["text"], "é");
    let modifiers = Modifiers::new(
        ModifiersState::SHIFT | ModifiersState::CONTROL,
        ModifiersKeys::empty(),
    );
    let event = supplemental(12, &WindowEvent::ModifiersChanged(modifiers)).unwrap();
    assert_eq!(event["shift"], true);
    assert_eq!(event["control"], true);
    assert_eq!(event["alt"], false);
    assert_eq!(
        supplemental(12, &WindowEvent::Focused(true)).unwrap()["focused"],
        true
    );
    assert!(
        supplemental(
            12,
            &WindowEvent::PointerMoved {
                device_id: None,
                position: (1.0, 2.0).into(),
                primary: true,
                source: PointerSource::Mouse
            }
        )
        .is_none()
    );
    assert!(
        supplemental(
            12,
            &WindowEvent::MouseWheel {
                device_id: None,
                delta: MouseScrollDelta::LineDelta(1.0, -2.0),
                phase: TouchPhase::Moved
            }
        )
        .is_none()
    );
}

#[test]
fn routed_keyboard_precedes_its_translated_modifier_state() {
    let raw = KeyboardOutputEvent {
        controller_id: 8,
        slot_id: 9,
        ep_target: 10,
        kind: 4,
        key_code: 0xe0,
        modifiers: 0x11,
        ..Default::default()
    };
    let modifiers = Modifiers::new(ModifiersState::CONTROL, ModifiersKeys::empty());
    let values = crate::javascript_input::from_batch_with_supplemental(
        77,
        InputBatch {
            events: vec![InputEvent::Keyboard(raw)],
            dropped: 0,
        },
        &WindowEvent::ModifiersChanged(modifiers),
    );
    assert_eq!(values.len(), 2);
    assert_eq!(values[0]["kind"], "keyboard");
    assert_eq!(values[0]["raw"]["modifiers"], 0x11);
    assert_eq!(values[0]["hid_usage"], 0xe0);
    assert_eq!(values[1]["kind"], "modifiers");
    assert_eq!(values[1]["control"], true);
    assert_eq!(values[0]["window_id"], 77);
    assert_eq!(values[1]["window_id"], 77);
}

#[test]
fn named_key_nul_marker_is_not_committed_text() {
    let values = crate::javascript_input::from_batch(
        7,
        winit_trueos::InputBatch {
            events: vec![winit_trueos::InputEvent::Keyboard(
                winit_trueos::KeyboardOutputEvent {
                    kind: 2,
                    key_code: 3,
                    codepoint: 0,
                    utf8_len: 1,
                    utf8: [0; 4],
                    ..Default::default()
                },
            )],
            dropped: 0,
        },
    );
    assert!(values[0]["text"].is_null());
    assert_eq!(values[0]["utf8_valid"], true);
    assert_eq!(values[0]["raw"]["utf8"], serde_json::json!([0, 0, 0, 0]));
}
