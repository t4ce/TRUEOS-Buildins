//! TRUEOS terminal zoom. Values are native font scales, with 2 equal to 100%.

#[allow(unused_imports)]
use stdext::platform::prelude::*;

use edit::helpers::{CoordType, Rect, Size};
use edit::input::vk;
use edit::oklab::StraightRgba;
use edit::tui::{Anchor, ButtonStyle, Context, FloatSpec};
use stdext::arena::Arena;
use stdext::arena_format;
use stdext::collections::BString;

pub struct FontSize {
    scale: u8,
    reported: Option<u8>,
    open: bool,
    return_to_page: bool,
}

impl Default for FontSize {
    fn default() -> Self {
        Self { scale: 2, reported: None, open: false, return_to_page: false }
    }
}

impl FontSize {
    pub fn invalidate_terminal(&mut self) {
        self.reported = None;
    }

    pub fn append_report<'a>(&mut self, arena: &'a Arena, output: &mut BString<'a>) {
        if self.reported != Some(self.scale) {
            stdext::arena_write_fmt!(arena, output, "\x1b]777;font-scale;1;{}\x07", self.scale);
            self.reported = Some(self.scale);
        }
    }

    pub fn take_page_focus(&mut self) -> bool {
        stdext::platform::mem::take(&mut self.return_to_page)
    }

    fn choose(&mut self, scale: u8) {
        debug_assert!((1..=8).contains(&scale));
        self.scale = scale;
        self.open = false;
        self.return_to_page = true;
    }
}

fn header_widths(width: CoordType) -> (CoordType, CoordType, bool) {
    let width = width.max(0);
    let full = width >= 17;
    let control = width.min(if full { 17 } else { 6 });
    (width - control, control, full)
}

/// Reserve the trailing cell before laying out the ordinary menu. Its nested
/// table is clipped to the remaining width rather than pushing zoom offscreen.
pub fn begin_header(ctx: &mut Context, background: StraightRgba, foreground: StraightRgba) {
    let (left, _, _) = header_widths(ctx.size().width);
    ctx.table_begin("editor-header");
    ctx.attr_background_rgba(background);
    ctx.attr_foreground_rgba(foreground);
    ctx.attr_intrinsic_size(Size { width: ctx.size().width, height: 1 });
    ctx.table_next_row();
    ctx.block_begin("menus");
    ctx.attr_intrinsic_size(Size { width: left, height: 1 });
}

pub fn end_header(ctx: &mut Context, state: &mut FontSize) {
    ctx.block_end(); // clipped ordinary menus
    let (_, width, full) = header_widths(ctx.size().width);
    ctx.block_begin("font-size");
    ctx.attr_intrinsic_size(Size { width, height: 1 });
    let label = arena_format!(
        ctx.arena(),
        "{}{}%",
        if full { "Font Size: " } else { "" },
        u16::from(state.scale) * 50
    );
    if ctx.button("toggle", &label, ButtonStyle::default().bracketed(false)) {
        state.open = !state.open;
        state.return_to_page |= !state.open;
        ctx.needs_rerender();
    }
    if width >= 6 {
        ctx.attr_padding(Rect::two(0, 1));
    }

    if state.open {
        ctx.block_begin("picker");
        ctx.attr_float(FloatSpec {
            anchor: Anchor::Last,
            gravity_x: 1.0,
            gravity_y: 0.0,
            offset_x: width as f32,
            offset_y: 1.0,
        });
        ctx.attr_border();
        ctx.focus_on_first_present();
        let escape = ctx.contains_focus() && ctx.consume_shortcut(vk::ESCAPE);
        let available = ctx.size();
        ctx.scrollarea_begin(
            "choices-scroll",
            Size {
                width: (available.width.min(10) - 2).max(1),
                height: (available.height - 3).clamp(1, 8),
            },
        );
        ctx.inherit_focus();
        ctx.table_begin("choices");
        ctx.inherit_focus();
        let mut selected = None;
        for scale in 1..=8 {
            ctx.table_next_row();
            ctx.inherit_focus();
            if ctx.button(
                "choice",
                &arena_format!(ctx.arena(), "{}%", u16::from(scale) * 50),
                ButtonStyle::default().bracketed(false).checked(state.scale == scale),
            ) {
                selected = Some(scale);
            }
            ctx.inherit_focus();
            if state.scale == scale {
                ctx.focus_on_first_present();
            }
        }
        ctx.table_end();
        ctx.scrollarea_end();
        ctx.block_end(); // floating picker
        if let Some(scale) = selected {
            state.choose(scale);
        } else if escape {
            state.open = false;
            state.return_to_page = true;
        } else if !ctx.contains_focus() {
            // Clicking another control keeps that control's intended focus.
            state.open = false;
        }
        if !state.open {
            ctx.needs_rerender();
        }
    }
    ctx.block_end(); // trailing control
    ctx.table_end();
}

#[cfg(test)]
mod tests {
    use edit::buffer::{RcTextBuffer, TextBuffer};
    use edit::helpers::Point;
    use edit::input::{Input, InputMouse, InputMouseState, kbmod};
    use edit::tui::Tui;

    use super::*;

    fn draw(ctx: &mut Context, state: &mut FontSize, buffer: RcTextBuffer) {
        begin_header(ctx, StraightRgba::from_rgba(0xff000000), StraightRgba::from_rgba(0xffffffff));
        ctx.menubar_begin();
        for (index, name) in ["File", "Edit", "View", "Help"].into_iter().enumerate() {
            ctx.next_block_id_mixin(index as u64 + 1);
            ctx.button("ordinary", name, ButtonStyle::default());
        }
        ctx.menubar_end();
        end_header(ctx, state);
        ctx.textarea("page", buffer);
        ctx.inherit_focus();
        if state.take_page_focus() {
            ctx.steal_focus();
        }
    }

    fn prepare(tui: &mut Tui, state: &mut FontSize, buffer: &RcTextBuffer, input: Option<Input>) {
        draw(&mut tui.create_context(input), state, buffer.clone());
        while tui.needs_settling() {
            draw(&mut tui.create_context(None), state, buffer.clone());
        }
    }

    fn mouse(state: InputMouseState, x: CoordType, y: CoordType) -> Input<'static> {
        Input::Mouse(InputMouse {
            state,
            modifiers: kbmod::NONE,
            position: Point { x, y },
            scroll: Point::default(),
            drag: false,
        })
    }

    fn click(
        tui: &mut Tui,
        state: &mut FontSize,
        buffer: &RcTextBuffer,
        x: CoordType,
        y: CoordType,
    ) {
        prepare(tui, state, buffer, Some(mouse(InputMouseState::Left, x, y)));
        prepare(tui, state, buffer, Some(mouse(InputMouseState::None, x, y)));
    }

    #[test]
    fn reports_each_selected_native_step_once_and_restores_it_on_reentry() {
        let arena = Arena::new(1024 * 1024).unwrap();
        let mut state = FontSize::default();
        let mut output = BString::empty();
        state.append_report(&arena, &mut output);
        assert_eq!(output.as_str(), "\x1b]777;font-scale;1;2\x07");
        for scale in 1..=8 {
            state.choose(scale);
            let mut output = BString::empty();
            state.append_report(&arena, &mut output);
            assert_eq!(output.as_str(), format!("\x1b]777;font-scale;1;{scale}\x07"));
            state.append_report(&arena, &mut output);
            assert_eq!(output.as_str().matches("font-scale").count(), 1);
        }
        state.invalidate_terminal();
        let mut output = BString::empty();
        state.append_report(&arena, &mut output);
        assert_eq!(output.as_str(), "\x1b]777;font-scale;1;8\x07");
    }

    #[test]
    fn header_always_reserves_zoom_before_giving_space_to_menus() {
        for width in 0..=160 {
            let (left, control, full) = header_widths(width);
            assert_eq!(left + control, width);
            assert_eq!(control, width.min(if full { 17 } else { 6 }));
            assert!(control > 0 || width == 0);
        }
    }

    #[test]
    fn rightmost_control_stays_clickable_when_ordinary_menus_are_clipped() {
        let arena = Arena::new(1024 * 1024).unwrap();
        for width in [6, 12, 24, 80] {
            let mut state = FontSize::default();
            let mut tui = Tui::new().unwrap();
            let buffer = TextBuffer::new_rc(false).unwrap();
            prepare(&mut tui, &mut state, &buffer, Some(Input::Resize(Size { width, height: 12 })));
            assert!(tui.render(&arena).contains("100%"), "visible percentage at {width} columns");
            click(&mut tui, &mut state, &buffer, width - 3, 0);
            assert!(state.open, "right control at {width} columns");
        }
    }

    #[test]
    fn short_picker_scrolls_to_400_and_keyboard_selection_returns_to_page() {
        let arena = Arena::new(1024 * 1024).unwrap();
        for height in [5, 6] {
            let mut state = FontSize::default();
            let mut tui = Tui::new().unwrap();
            let buffer = TextBuffer::new_rc(false).unwrap();
            prepare(&mut tui, &mut state, &buffer, Some(Input::Resize(Size { width: 24, height })));
            tui.render(&arena);
            click(&mut tui, &mut state, &buffer, 21, 0);
            for _ in 0..6 {
                prepare(&mut tui, &mut state, &buffer, Some(Input::Keyboard(vk::DOWN)));
            }
            assert!(tui.render(&arena).contains("400%"));
            prepare(&mut tui, &mut state, &buffer, Some(Input::Keyboard(vk::RETURN)));
            assert_eq!(state.scale, 8);
            assert!(!state.open);
            prepare(&mut tui, &mut state, &buffer, Some(Input::Text("x")));
            assert_eq!(buffer.borrow().text_length(), 1);
        }
    }

    #[test]
    fn each_size_is_selected_with_one_click() {
        for scale in 1..=8 {
            let mut state = FontSize::default();
            let mut tui = Tui::new().unwrap();
            let buffer = TextBuffer::new_rc(false).unwrap();
            prepare(
                &mut tui,
                &mut state,
                &buffer,
                Some(Input::Resize(Size { width: 40, height: 12 })),
            );
            click(&mut tui, &mut state, &buffer, 37, 0);
            click(&mut tui, &mut state, &buffer, 34, CoordType::from(scale) + 1);
            assert_eq!(state.scale, scale);
            assert!(!state.open);
            prepare(&mut tui, &mut state, &buffer, Some(Input::Text("x")));
            assert_eq!(buffer.borrow().text_length(), 1);
        }
    }

    #[test]
    fn escape_closes_without_changing_scale_and_returns_keyboard_to_page() {
        let mut state = FontSize::default();
        let mut tui = Tui::new().unwrap();
        let buffer = TextBuffer::new_rc(false).unwrap();
        prepare(&mut tui, &mut state, &buffer, Some(Input::Resize(Size { width: 40, height: 12 })));
        click(&mut tui, &mut state, &buffer, 37, 0);
        prepare(&mut tui, &mut state, &buffer, Some(Input::Keyboard(vk::ESCAPE)));
        assert!(!state.open);
        assert_eq!(state.scale, 2);
        prepare(&mut tui, &mut state, &buffer, Some(Input::Text("x")));
        assert_eq!(buffer.borrow().text_length(), 1);
    }
}
