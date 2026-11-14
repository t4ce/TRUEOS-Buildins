#![no_std]
#![no_main]
extern crate alloc;
mod printing;
use alloc::{format, vec, vec::Vec};
use paper::Editor;
use trueos::ui4_scene::{SpriteCorner, SpriteQuad};
use trueos::ui4_winit::{SpriteBackend, SpriteCommand};
use trueos::{
    ui4_scene::{self, Damage, Error, Frame},
    vgpu::*,
};

#[global_allocator]
static ALLOCATOR: trueos::TrueosAllocator = trueos::TrueosAllocator;
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    trueos::panic_abort("Paper panic\n")
}

fn main() {
    if let Err(error) = run() {
        trueos::logl::log(
            trueos::logl::level::ERROR,
            format_args!("Paper: failed errno={error}"),
        );
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    main();
    trueos::entry::finish()
}

fn ui(error: Error) -> i32 {
    match error {
        Error::Busy => ERR_BUSY,
        Error::NotFound => -2,
        _ => ERR_IO,
    }
}

fn run() -> Result<(), i32> {
    // Consume the one-shot runscript before any UI4 window can become visible.
    let settings = match trueos::async_fs::block_on(trueos::async_fs::read_file(b"vFile:launch")) {
        Ok(bytes) => {
            let script = core::str::from_utf8(&bytes).map_err(|_| -22)?;
            paper::script_settings(script).map_err(|message| {
                trueos::logl::log(trueos::logl::level::ERROR, message);
                -22
            })?
        }
        Err(trueos::async_fs::ERR_NOT_FOUND) => paper::Settings::default(),
        Err(error) => return Err(error),
    };
    let scale = settings.scale;
    let (width_mm, height_mm) = settings.sheet.dimensions();
    let page = ui4_scene::physical_extent(width_mm, height_mm).map_err(ui)?;
    let output = ui4_scene::output_dimensions().map_err(ui)?;
    let mut frame = Frame::open_immutable(
        (output.0 as i32 - page.0 as i32) / 2,
        (output.1 as i32 - page.1 as i32) / 2,
        page.0,
        page.1,
    )
    .map_err(ui)?;
    frame.set_title("Paper").map_err(ui)?;
    let print_menu = [ui4_scene::MenuEntry::new("Print A4", request_print)];
    frame.register_context_menu(&print_menu).map_err(ui)?;
    let device = Device::open(Capabilities::DEFAULT.union(Capabilities::PRESENT))?;
    let result = (|| {
        let queue = device.create_queue(QueueClass::Render)?;
        let shader =
            device.create_shader_module(SHADER_PACKAGE_CLIP_POSITION3_IMMEDIATE_RGBA_FNV1A64)?;
        let pipeline = device.create_render_pipeline(shader, 12, 0)?;
        let mut editor = Editor::with_settings(settings);
        let mut sprites = vec![GlyphSlot::default(); editor.cells.len()];
        let mut text_events = 0u64;
        let mut backspaces = 0u64;
        let mut cell_clicks = 0u64;
        let mut dirty = true;
        let mut visible = true;
        let mut blink = trueos::clock::Instant::now();
        let mut print_requested = false;
        let mut print_job = None;
        let mut print_poll = trueos::clock::Instant::now();
        loop {
            frame.pump_context_menu(&print_menu, &mut print_requested).map_err(ui)?;
            match frame.take_resize_event() {
                Ok(Some(event)) => {
                    frame.resize(event.width, event.height).map_err(ui)?;
                    dirty = true;
                }
                Ok(None) | Err(Error::Busy) => {}
                Err(Error::NotFound) => return Ok(()),
                Err(error) => return Err(ui(error)),
            }
            let mut input = false;
            while let Some(event) = frame.take_keyboard_event().map_err(ui)? {
                use trueos::input::*;
                if print_shortcut(&event) {
                    print_requested = true;
                } else if event.kind == KEYBOARD_OUTPUT_KIND_TEXT {
                    if let Some(ch) = char::from_u32(event.codepoint) {
                        editor.type_char(ch);
                        text_events += 1;
                        input = true;
                    }
                } else if event.kind == KEYBOARD_OUTPUT_KIND_KEY
                    && event.flags & KEYBOARD_OUTPUT_FLAG_PRESS != 0
                {
                    // Space is delivered as text by the keyboard translator.
                    if event.key_code != KEYBOARD_KEY_SPACE {
                        editor.key(event.key_code);
                        if event.key_code == KEYBOARD_KEY_BACKSPACE {
                            backspaces += 1;
                        }
                        input = true;
                    }
                }
            }
            if print_requested {
                print_requested = false;
                if print_job.is_none() {
                    let _ = frame.set_title("Paper · Preparing A4 print");
                    match printing::render_page(&editor, scale, glyph_pixels) {
                        Ok(page) => match page.submit() {
                            Ok(job) => {
                                print_job = Some((job, Some(trueos::print2d::JobState::Queued)));
                                print_poll = trueos::clock::Instant::now();
                                print_status(&mut frame, job, "queued");
                            }
                            Err(error) => print_error(&mut frame, format!("submission: {error:?}")),
                        },
                        Err(error) => print_error(&mut frame, format!("page: {error:?}")),
                    }
                }
            }
            if print_poll.elapsed().as_millis() >= 250 {
                print_poll = trueos::clock::Instant::now();
                if let Some((job, last)) = print_job.as_mut() {
                    match job.status() {
                        Ok(state) => {
                            if *last != Some(state) {
                                print_status(&mut frame, *job, print_state(state));
                                *last = Some(state);
                            }
                            if state.is_terminal() { print_job = None; }
                        }
                        Err(error) => {
                            print_error(&mut frame, format!("status: {error:?}"));
                            print_job = None;
                        }
                    }
                }
            }
            while let Some(event) = frame.take_pointer_event().map_err(ui)? {
                if event.buttons_pressed & ui4_scene::POINTER_BUTTON_PRIMARY != 0 {
                    let hit = editor.click(event.local_x, event.local_y, page, scale);
                    cell_clicks += u64::from(hit);
                    input |= hit;
                }
            }
            if input {
                visible = true;
                blink = trueos::clock::Instant::now();
                dirty = true;
            }
            if blink.elapsed().as_millis() >= 500 {
                visible = !visible;
                blink = trueos::clock::Instant::now();
                dirty = true;
            }
            if dirty {
                draw(
                    &mut frame,
                    page,
                    scale,
                    device,
                    queue,
                    pipeline,
                    &editor,
                    &mut sprites,
                    visible,
                )?;
                if input {
                    let _ = trueos::logl::log_record(
                        trueos::logl::level::INFO,
                        "paper",
                        format_args!(
                            "Paper: input published text_events={} backspaces={} cell_clicks={} cursor={} occupied={}",
                            text_events,
                            backspaces,
                            cell_clicks,
                            editor.cursor,
                            editor
                                .cells
                                .iter()
                                .filter(|cell| cell.character.is_some())
                                .count()
                        ),
                    );
                }
                dirty = false;
            }
            trueos::vsys::sleep_ms(16);
        }
    })();
    let close = device.close();
    result.and(close)
}

fn request_print(requested: &mut bool) { *requested = true; }

fn print_shortcut(event: &trueos::input::TrueosKeyboardOutputEvent) -> bool {
    event.kind == trueos::input::KEYBOARD_OUTPUT_KIND_TEXT
        && event.modifiers & 0x11 != 0
        && matches!(event.codepoint, 16 | 80 | 112)
}

fn print_status(frame: &mut Frame, job: trueos::print2d::JobId, state: &str) {
    let _ = frame.set_title(&format!("Paper · Print {}: {state}", job.get()));
    let _ = trueos::vshell::line(&format!("Paper: A4 print job={} {state}", job.get()));
    trueos::logl::log(trueos::logl::level::INFO,
        format_args!("Paper: A4 print job={} {state}", job.get()));
}

fn print_error(frame: &mut Frame, error: alloc::string::String) {
    let _ = frame.set_title("Paper · A4 print failed");
    let _ = trueos::vshell::line(&format!("Paper: A4 print failed: {error}"));
    trueos::logl::log(trueos::logl::level::ERROR, format_args!("Paper: A4 print failed: {error}"));
}

fn print_state(state: trueos::print2d::JobState) -> &'static str {
    use trueos::print2d::JobState::*;
    match state {
        Queued => "queued", WaitingForPrinter => "waiting for printer", Rendering => "rendering",
        Connecting => "connecting", Sending => "sending", Submitted => "accepted by printer",
        Printing => "printing", Completed => "completed", Failed => "failed", Canceled => "canceled",
        OutcomeUnknown => "outcome unknown",
    }
}

fn draw(
    frame: &mut Frame,
    page: (u32, u32),
    scale: u32,
    device: Device,
    queue: Queue,
    pipeline: RenderPipeline,
    editor: &Editor,
    sprites: &mut [GlyphSlot],
    cursor_visible: bool,
) -> Result<(), i32> {
    for (index, cell) in editor.cells.iter().enumerate() {
        if let Some(ch) = cell.character
            && sprites[index].character != Some(ch)
        {
            let bytes = glyph_pixels(
                if editor.mode == paper::Mode::Gridpaper {
                    scale
                } else {
                    1
                },
                ch,
            )?;
            if bytes.len() < 8 {
                return Err(ERR_IO);
            }
            let width = u32::from_le_bytes(bytes[..4].try_into().unwrap());
            let height = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
            frame
                .upload_sprite_rgba8(index as u32 + 1, width, height, &bytes[8..])
                .map_err(ui)?;
            sprites[index] = GlyphSlot {
                character: Some(ch),
                width,
                height,
            };
        }
    }
    let (vertices, indices) = paper::geometry_for_settings(
        page,
        (frame.width(), frame.height()),
        paper::Settings {
            mode: editor.mode,
            scale,
            sheet: editor.sheet,
            wrap: editor.wrap,
        },
    );
    let vb = device.create_buffer(vertices.len(), BUFFER_USAGE_MAP_WRITE | BUFFER_USAGE_VERTEX)?;
    let ib = device.create_buffer(indices.len(), BUFFER_USAGE_MAP_WRITE | BUFFER_USAGE_INDEX)?;
    if device.write_buffer(vb, 0, &vertices)? != vertices.len()
        || device.write_buffer(ib, 0, &indices)? != indices.len()
    {
        return Err(ERR_IO);
    }
    loop {
        match frame.begin_gpu_frame() {
            Err(Error::Busy) => trueos::vsys::sleep_ms(16),
            result => {
                result.map_err(ui)?;
                break;
            }
        }
    }
    let mut batch = IndexedDrawBatchV2 {
        clear_rgba8_srgb: u32::from_le_bytes([255; 4]),
        draw_count: 1,
        ..Default::default()
    };
    batch.draws[0] = IndexedBatchDrawV2 {
        index_count: (indices.len() / 4) as u32,
        // White ruling keeps the native line batch clear path for blank sheets.
        rgba8_srgb: if editor.mode == paper::Mode::Whitepaper {
            u32::from_le_bytes([255; 4])
        } else {
            u32::from_le_bytes([190, 190, 190, 255])
        },
        topology: PRIMITIVE_TOPOLOGY_LINE_LIST,
        ..Default::default()
    };
    loop {
        let surface = device.acquire_ui4_surface(frame.window_id())?;
        match device.submit_ui4_indexed_batch_v2(queue, surface, pipeline, vb, ib, batch) {
            Ok(point) => {
                device.wait(queue, point.value)?;
                break;
            }
            Err(ERR_BUSY) => trueos::vsys::sleep_ms(16),
            Err(error) => return Err(error),
        }
    }
    let mut commands = Vec::with_capacity(editor.cells.len() + 1);
    for (index, cell) in editor.cells.iter().enumerate() {
        if cell.character.is_none() {
            continue;
        }
        let glyph = sprites[index];
        let (x, y, w, h) = editor.bounds(index, page, scale);
        // Centre the em box; crop its white border inside the grid lines.
        let left = x + w.saturating_sub(glyph.width) / 2;
        let top = y + h.saturating_sub(glyph.height) / 2;
        let inset = 1;
        if let Some(quad) = clipped_quad(
            index as u32 + 1,
            left,
            top,
            glyph.width,
            glyph.height,
            inset,
            frame.width(),
            frame.height(),
            u32::MAX,
        ) {
            commands.push(SpriteCommand {
                quad,
                backend: SpriteBackend::Bcs0,
            });
        }
    }
    if cursor_visible {
        let (x, y, w, h) = editor.bounds(editor.cursor, page, scale);
        if let Some(quad) = clipped_quad(
            0,
            x + w / 2,
            y + 2,
            1,
            h.saturating_sub(4),
            0,
            frame.width(),
            frame.height(),
            u32::from_le_bytes([0, 0, 0, 255]),
        ) {
            commands.push(SpriteCommand {
                quad,
                backend: SpriteBackend::Bcs0,
            });
        }
    }
    if !commands.is_empty() {
        loop {
            match frame.draw_sprite_commands(&commands) {
                Err(Error::Busy) => trueos::vsys::sleep_ms(16),
                result => {
                    result.map_err(ui)?;
                    break;
                }
            }
        }
    }
    loop {
        match frame.publish(Damage::full(frame.width(), frame.height())) {
            Err(Error::Busy) => trueos::vsys::sleep_ms(16),
            result => {
                result.map_err(ui)?;
                break;
            }
        }
    }
    device.destroy_buffer(vb)?;
    device.destroy_buffer(ib)
}

#[derive(Clone, Copy, Default)]
struct GlyphSlot {
    character: Option<char>,
    width: u32,
    height: u32,
}
fn glyph_pixels(scale: u32, ch: char) -> Result<Vec<u8>, i32> {
    let path = format!("vFile:microfont-grid/{scale}/{:x}", ch as u32);
    trueos::async_fs::block_on(trueos::async_fs::read_file(path.as_bytes()))
}
fn clipped_quad(
    id: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    inset: u32,
    vw: u32,
    vh: u32,
    color: u32,
) -> Option<SpriteQuad> {
    if w <= 2 * inset || h <= 2 * inset || x + inset >= vw || y + inset >= vh {
        return None;
    }
    let left = x + inset;
    let top = y + inset;
    let right = (x + w - inset).min(vw);
    let bottom = (y + h - inset).min(vh);
    let corner = |px: u32, py: u32| SpriteCorner {
        x: px as f32,
        y: py as f32,
        u: (px - x) as f32 / w as f32,
        v: (py - y) as f32 / h as f32,
    };
    Some(SpriteQuad {
        sprite_id: id,
        c0: corner(left, top),
        c1: corner(right, top),
        c2: corner(right, bottom),
        c3: corner(left, bottom),
        color_rgba: color,
        source_over: false,
    })
}
