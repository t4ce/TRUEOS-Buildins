// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cell-space metadata for the terminal's independent Conway background task.
//! The terminal rasterizer supplies glyph coverage and color, not this module.

use std::fmt;

use crate::helpers::{CoordType, Point, Rect};

const MAX_EXCLUSIONS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Page {
    pub document: usize,
    pub rect: Rect,
    pub scroll: Point,
    pub wrap: bool,
    pub tab_size: CoordType,
    pub glyph_input_revision: u64,
}

#[derive(Default)]
pub(crate) struct EditorBackground {
    previous_page: Option<Page>,
    page: Option<Page>,
    enabled: bool,
    managed: bool,
    exclusions: [Rect; MAX_EXCLUSIONS],
    exclusion_count: usize,
    exclusion_overflow: bool,
    scroll_revision: u64,
    emission_revision: u64,
}

impl EditorBackground {
    pub fn begin_frame(&mut self) {
        self.page = None;
        self.enabled = false;
        self.exclusion_count = 0;
        self.exclusion_overflow = false;
    }

    pub fn set_page(&mut self, enabled: bool, page: Page) {
        self.managed = true;
        self.enabled = enabled;
        self.page = Some(page);
    }

    pub fn exclude(&mut self, rect: Rect) {
        let Some(page) = self.page else { return };
        let rect = rect.intersect(page.rect);
        if rect.is_empty() {
            return;
        }
        if self.exclusion_count == MAX_EXCLUSIONS {
            // Falling back to the opaque editor is preferable to affecting a popup.
            self.exclusion_overflow = true;
            return;
        }
        self.exclusions[self.exclusion_count] = rect;
        self.exclusion_count += 1;
    }

    pub fn finish_frame(&mut self) {
        if let Some(page) = self.page {
            if let Some(previous) = self.previous_page {
                if previous.document != page.document
                    || previous.rect != page.rect
                    || previous.scroll != page.scroll
                    || previous.wrap != page.wrap
                    || previous.tab_size != page.tab_size
                {
                    self.scroll_revision = self.scroll_revision.wrapping_add(1);
                }
            }
            if self.previous_page.is_none_or(|previous| {
                previous.document != page.document
                    || previous.glyph_input_revision != page.glyph_input_revision
            }) {
                self.emission_revision = self.emission_revision.wrapping_add(1);
            }
        }
        self.previous_page = self.page;
    }
}

impl fmt::Display for EditorBackground {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.managed {
            return Ok(());
        }
        let rect = self.page.map_or_else(Rect::default, |page| page.rect);
        let enabled = self.enabled && !self.exclusion_overflow && !rect.is_empty();
        write!(
            f,
            "\x1b]777;edit-life;1;{};{};{};{};{};{};{};{}",
            u8::from(enabled),
            rect.left,
            rect.top,
            rect.width().max(0),
            rect.height().max(0),
            self.scroll_revision,
            self.emission_revision,
            self.exclusion_count,
        )?;
        for rect in &self.exclusions[..self.exclusion_count] {
            write!(f, ";{};{};{};{}", rect.left, rect.top, rect.width(), rect.height())?;
        }
        f.write_str("\x07")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Page {
        Page {
            document: 1,
            rect: Rect { left: 4, top: 1, right: 79, bottom: 23 },
            scroll: Point::default(),
            wrap: false,
            tab_size: 4,
            glyph_input_revision: 0,
        }
    }

    fn publish(state: &mut EditorBackground, page: Page) {
        state.begin_frame();
        state.set_page(true, page);
        state.finish_frame();
    }

    #[test]
    fn protocol_uses_cell_viewport_and_clipped_popup_exclusions() {
        let mut state = EditorBackground::default();
        state.begin_frame();
        state.set_page(true, page());
        state.exclude(Rect { left: 0, top: 0, right: 12, bottom: 8 });
        state.exclude(Rect { left: 0, top: 23, right: 80, bottom: 24 });
        state.finish_frame();
        assert_eq!(state.to_string(), "\x1b]777;edit-life;1;1;4;1;75;22;0;1;1;4;1;8;7\x07");
    }

    #[test]
    fn unchanged_frames_and_popups_do_not_emit_or_reset() {
        let mut state = EditorBackground::default();
        publish(&mut state, page());
        publish(&mut state, page());
        state.exclude(page().rect);
        assert_eq!((state.scroll_revision, state.emission_revision), (0, 1));
        let mut typed = page();
        typed.glyph_input_revision += 1;
        publish(&mut state, typed);
        assert_eq!((state.scroll_revision, state.emission_revision), (0, 2));
        typed.scroll.y = 3;
        publish(&mut state, typed);
        assert_eq!((state.scroll_revision, state.emission_revision), (1, 2));
    }

    #[test]
    fn document_and_layout_changes_reset_without_continuous_emission() {
        let mut state = EditorBackground::default();
        let mut next = page();
        publish(&mut state, next);
        next.document = 2;
        publish(&mut state, next);
        assert_eq!((state.scroll_revision, state.emission_revision), (1, 2));
        next.rect.top += 2;
        publish(&mut state, next);
        next.wrap = true;
        publish(&mut state, next);
        next.tab_size = 8;
        publish(&mut state, next);
        assert_eq!((state.scroll_revision, state.emission_revision), (4, 2));
    }

    #[test]
    fn toggle_and_closing_the_document_publish_disabled_state() {
        let mut state = EditorBackground::default();
        assert_eq!(state.to_string(), "");
        publish(&mut state, page());
        state.begin_frame();
        state.set_page(false, page());
        state.finish_frame();
        assert!(state.to_string().starts_with("\x1b]777;edit-life;1;0;4;1;75;22;0;1;0"));
        state.begin_frame();
        state.finish_frame();
        assert_eq!(state.to_string(), "\x1b]777;edit-life;1;0;0;0;0;0;0;1;0\x07");
    }

    #[test]
    fn exclusion_overflow_preserves_opaque_popups() {
        let mut state = EditorBackground::default();
        publish(&mut state, page());
        for _ in 0..=MAX_EXCLUSIONS {
            state.exclude(page().rect);
        }
        assert!(state.to_string().starts_with("\x1b]777;edit-life;1;0;"));
        assert_eq!(state.exclusion_count, MAX_EXCLUSIONS);
    }

    #[test]
    fn rendered_textarea_metadata_excludes_editor_chrome_and_modal() {
        use crate::buffer::TextBuffer;
        use crate::helpers::Size;
        use crate::input::Input;
        use crate::tui::{Context, Tui};
        use stdext::arena::Arena;

        let buffer = TextBuffer::new_rc(false).unwrap();
        buffer.borrow_mut().set_margin_enabled(true);
        buffer.borrow_mut().write_raw(b"colored glyphs");
        let mut tui = Tui::new().unwrap();
        let draw = |ctx: &mut Context, modal: bool| {
            ctx.label("menubar", "View");
            ctx.textarea("page", buffer.clone());
            ctx.attr_editor_background(true);
            ctx.attr_intrinsic_size(Size { width: 0, height: 22 });
            ctx.label("statusbar", "1:1");
            if modal {
                ctx.modal_begin("about", "About");
                ctx.label("body", "Opaque");
                ctx.modal_end();
            }
        };
        {
            let mut ctx = tui.create_context(Some(Input::Resize(Size { width: 80, height: 24 })));
            draw(&mut ctx, false);
        }
        while tui.needs_settling() {
            draw(&mut tui.create_context(None), false);
        }
        let arena = Arena::new(1024 * 1024).unwrap();
        let output = tui.render(&arena);
        assert!(output.starts_with("\x1b[?2026h"));
        assert!(output.ends_with("\x1b]777;edit-life;1;1;4;1;75;22;0;1;0\x07\x1b[?2026l"));

        draw(&mut tui.create_context(None), true);
        while tui.needs_settling() {
            draw(&mut tui.create_context(None), true);
        }
        let output = tui.render(&arena);
        let (_, metadata) = output.rsplit_once("\x1b]777;").unwrap();
        assert!(metadata.starts_with("edit-life;1;1;4;1;75;22;0;1;1;"));
        assert!(metadata.ends_with("\x07\x1b[?2026l"));
        assert_eq!(metadata.matches(';').count(), 13, "one opaque modal rectangle");
    }
}
