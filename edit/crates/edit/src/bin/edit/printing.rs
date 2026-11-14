//! Print the current in-memory document as DIN A4 pages.

use edit::buffer::TextBuffer;
#[allow(unused_imports)]
use stdext::platform::prelude::*;

/// Reading the gap buffer must not save it, move its cursor, or mark it clean.
fn snapshot_document(buffer: &TextBuffer) -> String {
    let mut bytes = Vec::with_capacity(buffer.text_length());
    loop {
        let chunk = buffer.read_forward(bytes.len());
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(chunk);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(target_os = "trueos")]
use stdext::platform::time::Duration;
#[cfg(target_os = "trueos")]
use trueos::print2d::{JobId, JobState};

#[cfg(target_os = "trueos")]
#[derive(Default)]
pub struct PrintState {
    requested: bool,
    open: bool,
    job: Option<JobId>,
    last_state: Option<JobState>,
    document: String,
    message: String,
}

#[cfg(target_os = "trueos")]
impl PrintState {
    pub fn request(&mut self) {
        self.open = true;
        // Repeated shortcut input reopens the active job, rather than printing
        // duplicate physical pages while that job is still in progress.
        self.requested |= self.job.is_none();
    }

    pub fn read_timeout(&self, timeout: Duration) -> Duration {
        if self.job.is_some() { timeout.min(Duration::from_millis(250)) } else { timeout }
    }

    pub fn poll(&mut self) -> bool {
        let Some(job) = self.job else { return false };
        match job.status() {
            Ok(state) => {
                if self.last_state == Some(state) {
                    return false;
                }
                self.last_state = Some(state);
                self.message = format!("Job {}: {}", job.get(), job_state_text(state));
                crate::trueos_log(&format!("print A4: {}", self.message));
                if state.is_terminal() {
                    self.job = None;
                }
            }
            Err(error) => {
                self.message = format!("Job {}: status unavailable ({error:?})", job.get());
                self.job = None;
            }
        }
        true
    }
}

#[cfg(target_os = "trueos")]
fn job_state_text(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "Queued",
        JobState::WaitingForPrinter => "Waiting for printer",
        JobState::Rendering => "Preparing pages",
        JobState::Connecting => "Connecting to printer",
        JobState::Sending => "Sending pages",
        JobState::Submitted => "Accepted by printer",
        JobState::Printing => "Printing",
        JobState::Completed => "Completed",
        JobState::Failed => "Failed",
        JobState::Canceled => "Canceled",
        JobState::OutcomeUnknown => "Delivery uncertain; check printer before retrying",
    }
}

#[cfg(target_os = "trueos")]
pub fn draw(ctx: &mut edit::tui::Context, state: &mut crate::state::State) {
    use edit::tui::ButtonStyle;
    if stdext::platform::mem::take(&mut state.printing.requested) {
        if let Some(document) = state.documents.active() {
            let text = snapshot_document(&document.buffer.borrow());
            state.printing.document = document.filename.clone();
            state.printing.last_state = None;
            match trueos::print2d::a4::submit_text(&text) {
                Ok(job) => {
                    state.printing.job = Some(job);
                    state.printing.message = format!("Job {}: Queued", job.get());
                    crate::trueos_log(&format!(
                        "print A4: queued job={} document={}",
                        job.get(),
                        document.filename
                    ));
                }
                Err(error) => {
                    state.printing.message = format!("Could not queue print job: {error:?}");
                    crate::trueos_log(&format!("print A4: {}", state.printing.message));
                }
            }
        } else {
            state.printing.message = String::from("Open a document before printing");
        }
        ctx.needs_rerender();
    }
    if !state.printing.open {
        return;
    }
    ctx.modal_begin("print-a4", "Print A4");
    ctx.block_begin("content");
    ctx.inherit_focus();
    if !state.printing.document.is_empty() {
        ctx.label("document", &state.printing.document);
    }
    ctx.label("status", &state.printing.message);
    if ctx.button("close", "Close", ButtonStyle::default()) {
        state.printing.open = false;
    }
    ctx.block_end();
    if ctx.modal_end() {
        state.printing.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_snapshot_preserves_unsaved_unicode_text_and_editor_state() {
        let mut buffer = TextBuffer::new(true).unwrap();
        buffer.write_raw("first\r\nGrüße\tworld\n".as_bytes());
        buffer.cursor_move_to_logical(edit::helpers::Point { x: 2, y: 1 });
        let position = buffer.cursor_logical_pos();
        let generation = buffer.generation();
        assert!(buffer.is_dirty());
        let text = snapshot_document(&buffer);
        // Edit canonicalizes inserted CRLF to the buffer's current LF mode.
        assert_eq!(text, "first\nGrüße\tworld\n");
        assert!(buffer.is_dirty());
        assert_eq!(buffer.generation(), generation);
        assert_eq!(buffer.cursor_logical_pos(), position);
    }

    #[test]
    fn print_snapshot_does_not_modify_a_clean_buffer_or_include_encoding_bom() {
        let mut buffer = TextBuffer::new(true).unwrap();
        buffer.write_raw("A4 receipt\n".as_bytes());
        buffer.set_encoding("UTF-8 BOM");
        buffer.mark_as_clean();
        let text = snapshot_document(&buffer);
        assert_eq!(text, "A4 receipt\n");
        assert!(!buffer.is_dirty());
        assert_eq!(buffer.encoding(), "UTF-8 BOM");
    }

    #[test]
    fn print_snapshot_contains_both_sides_of_an_edited_gap() {
        let mut buffer = TextBuffer::new(true).unwrap();
        buffer.write_raw("before after".as_bytes());
        buffer.cursor_move_to_logical(edit::helpers::Point { x: 7, y: 0 });
        buffer.write_raw("Grüße ".as_bytes());
        let generation = buffer.generation();
        assert_eq!(snapshot_document(&buffer), "before Grüße after");
        assert!(buffer.is_dirty());
        assert_eq!(buffer.generation(), generation);
    }
}
