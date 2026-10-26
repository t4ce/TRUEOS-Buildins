# TRUEOS UI4 backend

This crate supplies Winit's `target_os = "trueos"` platform backend. It maps
top-level Winit windows to broker-owned UI4 visual frames.

The backend uses the versioned UI4 keyboard stream and preserves physical USB
HID usages, press/release state, modifiers, and device-loss resets. Window
visibility, hit testing, opacity, and title metadata use the versioned window
state ABI. Escape is delivered to the Winit application rather than closing a
frame in the host.

UI4 owns presentation. Raw-window-handle exposes a host-issued UI4 graphics
connection and a generation-bearing visual-frame ID; neither is an X11,
Wayland, or output handle. OpenGL and Glutin surface creation remain
unavailable until a TRUEOS graphics backend consumes those handles. UI4 stores
title metadata, but does not render title bars.

Run the backend checks without inheriting a parent Blueprint Cargo config:

```sh
cd /tmp
cargo test --manifest-path /home/t4ce/Repos/TRUEOS-Blueprints/vendor/winit/Cargo.toml -p winit-trueos
cargo check --manifest-path /home/t4ce/Repos/TRUEOS-Blueprints/vendor/winit/Cargo.toml -p winit-trueos
```

The full facade is selected for the Blueprint application target. The archived
TRUEOS standard library builds successfully. Use the Blueprint builder's
vendored `libc` overlay for this target. The raw check below still reaches the
archived toolchain's `restricted_std` gate in ordinary `std` consumers; it is
not a Winit backend error:

```sh
cd /tmp
RUSTC=/home/t4ce/Repos/TRUEOS-Rust-Toolchain-nightly-2026-07-10/bin/rustc \
  /home/t4ce/Repos/TRUEOS-Rust-Toolchain-nightly-2026-07-10/bin/cargo \
  -Zbuild-std=std,panic_abort -Zjson-target-spec check \
  --config 'patch.crates-io.libc.path="/home/t4ce/Repos/TRUEOS-Blueprints/vendor/libc-0.2.186"' \
  --manifest-path /home/t4ce/Repos/TRUEOS-Blueprints/vendor/winit/Cargo.toml \
  -p winit --no-default-features --features serde \
  --target /home/t4ce/Repos/TRUEOS-Blueprints/apps/target.json
```

This backend targets Winit 0.31. It does not make applications built against
the distinct Winit 0.30 API, including the current Alacritty checkout, source
compatible by itself.

## Aspect-preserving content

`WindowExtTrueOS::trueos_set_resize_aspect_ratio(Some(PhysicalSize::new(16, 9)))`
opts one window into a centered, aspect-preserving content viewport.
`trueos_content_viewport()` returns its physical position and size; `None`
restores unrestricted content sizing. Both ratio dimensions must be nonzero.

`surface_size()`, `SurfaceResized`, and both UI4 backing allocations retain the
full offered extent. The application lays out and draws only inside the content
viewport and subtracts its position when mapping pointer coordinates. Winit
leaves standard input coordinates in the full window's coordinate system.
Unused margins require initialization when storage is allocated, then can stay
untouched through retained region rendering.

## Input and multiple combos

The existing Winit callback API remains unchanged. UI4 routes window keyboard,
pointer (including tablet/eye-tracker positions), wheel, and pan events. All
keyboard/pointer/pan events now carry process-local `DeviceId`s; resolve them
with `winit::platform::trueos::device_source(id)` to recover the full controller,
slot, endpoint and device kind. IDs are not combo IDs, and rebinding a combo does
not change endpoint identity. Keyboard modifiers are the union of the keyboards
active in that window. Device-loss releases retain their originating device ID.

`listen_device_events` supports `Always`, `WhenFocused` (default), and `Never`.
Independent host sequence readers deliver physical keyboard keys, wheel,
buttons, and explicitly supplied relative cursor reports to `device_event`.
Mouse motion uses the signed relative report even at desktop edges; absolute
coordinates are never misreported as raw motion. Readers discard the initial
backlog and process at most 256 records per stream per iteration. Queue loss
logs a warning and releases tracked held inputs. Absolute-only pointing devices
remain available through window events and the native interface.

Enable native routed records on each window where needed:

```rust,ignore
use winit::platform::trueos::{InputEvent, WindowExtTrueOS};

window.trueos_capture_input(true);
// In about_to_wait, or after a standard window input callback:
let batch = window.trueos_take_input();
if batch.dropped != 0 {
    // Reset application held state and resample the native HID state.
}
for event in batch.events {
    let device = event.device_id();
    match event {
        InputEvent::Pointer(pointer) => {
            // pointer.combo_id, pointer.vcursor, dx/dy, buttons_down,
            // buttons_pressed/released, local and desktop coordinates.
        }
        InputEvent::Keyboard(key) => {
            // Endpoint identity, HID usage (kind 4), text, modifiers,
            // sequences, repeat and device-loss (kind 3).
        }
        InputEvent::Pan(pan) => {
            // Combo identity, endpoint and begin/update/end deltas.
        }
        _ => {}
    }
}
```

Capture is opt-in, bounded to 1024 records per window, with an explicit loss
counter. It does not consume standard events. Records follow backend drain
order (keyboard, pointer, pan), not a shared hardware timestamp order. UI4's
keyboard ABI has no combo ID; match its endpoint against the keyboard member
of `hid::input_combos()` when a persona is needed.

On TRUEOS, `winit::platform::trueos::hid` exposes the platform's VLayer input
API directly, using its canonical types rather than a second ABI copy:

- `InputCombo::request`, member binding, color, refresh and removal;
  `input_combos()` enumerates physical/virtual collections.
- `hid_hut_mice/keyboards/tablets()` discover endpoints and current state.
- `hid_tablet_read()` preserves tablet data (including pressure) not represented
  by ordinary Winit pointer events; `midi_read_v1()` has an independent sequence
  cursor and explicit loss count.
- `VCursor`, `VKeyboard`, and `VGamepad` request virtual devices, submit commands,
  and release capabilities on drop. `VGamepad::snapshot()` exposes virtual
  gamepad state. Physical gamepad samples are not currently exposed by VLayer.

For example, create one independently controlled persona; repeat for N personas:

```rust,ignore
use winit::platform::trueos::hid::{InputCombo, InputComboSourceKind, VCursor, VKeyboard};

let cursor = VCursor::request("player cursor")?;
let keyboard = VKeyboard::request("player keyboard")?;
let combo = InputCombo::request("player", InputComboSourceKind::Human, None)?;
combo.bind_cursor(&cursor)?;
combo.bind_keyboard(&keyboard)?;
// Keep cursor/keyboard alive; submit through their capability-backed methods.
// Bind a tablet endpoint and gamepad independently when available.
// Removing a combo removes its identity, not its member devices.
```

One combo holds at most one device of each supported class, matching VLayer's
contract. Multiple combos work concurrently; they are not collapsed into a
single synthetic keyboard or cursor. MIDI and tablet details stay in the native
interface because Winit has no standard MIDI/gamepad/pressure event equivalent.
The native namespace follows the platform VLayer API; standard Winit types and
callbacks retain their existing signatures.
