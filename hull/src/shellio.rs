//! Bounded console output, URL input and frame diagnostics.
use alloc::{string::{String, ToString}, vec::Vec};
use alloc::collections::VecDeque;
#[cfg(not(target_os = "trueos"))]
use core::cell::RefCell;

const OUTPUT_LINES: usize = 256;
const LINE_CHARS: usize = 2048;
#[cfg(not(target_os = "trueos"))]
thread_local! {
    static OUTPUT: RefCell<VecDeque<String>> = const { RefCell::new(VecDeque::new()) };
}

#[cfg(target_os = "trueos")]
static OUTPUT: crate::platform::Mutex<VecDeque<String>> =
    crate::platform::Mutex::new(VecDeque::new());

fn with_output<R>(f: impl FnOnce(&mut VecDeque<String>) -> R) -> R {
    #[cfg(target_os = "trueos")]
    { f(&mut OUTPUT.lock().unwrap()) }
    #[cfg(not(target_os = "trueos"))]
    { OUTPUT.with(|output| f(&mut output.borrow_mut())) }
}

/// All terminal text is plain data, including script-authored escape sequences.
pub(super) fn line(text: &str) {
    with_output(|output| {
        for line in text.split('\n').take(OUTPUT_LINES) {
            let plain: String = line
                .chars()
                .take(LINE_CHARS)
                .map(|character| {
                    if character == '\t' {
                        ' '
                    } else if character.is_control() {
                        '�'
                    } else {
                        character
                    }
                })
                .collect();
            if output.len() == OUTPUT_LINES {
                output.pop_front();
            }
            output.push_back(plain);
        }
    });
}

pub(super) fn linef(arguments: core::fmt::Arguments<'_>) {
    line(&arguments.to_string());
}

pub(super) fn take_output() -> Vec<String> {
    with_output(|output| output.drain(..).collect())
}

pub(super) const MAX_SCRIPT_BYTES: usize = 64 * 1024;

/// Paste is inert until a separate typed submission command consumes it.
#[derive(Default)]
pub(super) struct ScriptInput {
    source: String,
    overflow: bool,
}

impl ScriptInput {
    pub fn append(&mut self, source: &str) {
        if self.overflow || self.source.len().saturating_add(source.len()) > MAX_SCRIPT_BYTES {
            self.overflow = true;
            return;
        }
        self.source.push_str(source);
    }

    pub fn len(&self) -> usize {
        self.source.len()
    }

    pub fn finish(self) -> Result<String, &'static str> {
        if self.overflow {
            Err("script exceeds 64 KiB; paste was discarded")
        } else {
            Ok(self.source)
        }
    }
}

/// Distinguishes draw attempts, completed frames, publication, and backpressure.
#[derive(Default)]
pub(super) struct FrameCounter {
    f: u64,
    attempts: u64,
    published: u64,
    busy: u64,
    stage: &'static str,
}

impl FrameCounter {
    pub fn attempt(&mut self) {
        self.attempts += 1;
        self.stage = "draw";
    }
    pub fn produced(&mut self, reason: &str, width: u32, height: u32, gpu: bool) {
        self.f += 1;
        linef(format_args!(
            "hull: f={} attempts={} published={} busy={} reason={} viewport={}x{} gpu={}",
            self.f, self.attempts, self.published, self.busy, reason, width, height, gpu,
        ));
    }
    pub fn publishing(&mut self) {
        self.stage = "publish";
    }
    pub fn published(&mut self) {
        self.published += 1;
        linef(format_args!(
            "hull: published={} f={}",
            self.published, self.f
        ));
    }
    pub fn busy(&mut self) {
        self.busy += 1;
        // Keep backpressure diagnostics from flooding the shell/compositor.
        if self.busy.is_power_of_two() {
            linef(format_args!(
                "hull: busy={} stage={} f={} attempts={} published={}",
                self.busy, self.stage, self.f, self.attempts, self.published,
            ));
        }
    }
    pub fn resizing(&mut self) {
        self.stage = "resize";
    }
}

#[derive(Default)]
pub(super) struct ShellInput {
    bytes: Vec<u8>,
    overflow: bool,
}

impl ShellInput {
    pub fn push(&mut self, byte: u8) -> Option<Result<String, &'static str>> {
        match byte {
            b'\r' | b'\n' => {
                let bytes = core::mem::take(&mut self.bytes);
                if core::mem::take(&mut self.overflow) {
                    return Some(Err("URL is too long"));
                }
                if bytes.is_empty() {
                    return None;
                }
                Some(String::from_utf8(bytes).map_err(|_| "URL must be valid UTF-8"))
            }
            3 | 21 => {
                self.bytes.clear();
                self.overflow = false;
                None
            }
            8 | 127 => {
                self.bytes.pop();
                None
            }
            0x20..=0xff => {
                if self.bytes.len() < 4096 && !self.overflow {
                    self.bytes.push(byte);
                } else {
                    self.overflow = true;
                }
                None
            }
            _ => None,
        }
    }
}

pub(super) fn page_url(line: &str) -> Result<String, &'static str> {
    let line = line.trim();
    if line.len() > 4096 {
        return Err("URL is too long");
    }
    if line.chars().any(char::is_whitespace) {
        return Err("enter one HTTPS or loopback preview URL per line");
    }
    let mut url = url::Url::parse(line)
        .map_err(|_| "enter an HTTPS or loopback preview URL")?;
    if !crate::resource_url::allowed(&url) {
        return Err("enter an HTTPS or loopback preview URL");
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pasted_script_preserves_literal_commands_and_refuses_partial_overflow() {
        let mut script = ScriptInput::default();
        script.append("console.log('🦀');\n:end\n:run\n");
        assert_eq!(script.finish().unwrap(), "console.log('🦀');\n:end\n:run\n");
        let mut script = ScriptInput::default();
        script.append(&"x".repeat(MAX_SCRIPT_BYTES));
        assert_eq!(script.len(), MAX_SCRIPT_BYTES);
        script.append("🦀");
        assert!(
            script.finish().is_err(),
            "overflow must never run a truncated script"
        );
    }
    #[test]
    fn console_output_keeps_terminal_controls_as_plain_text_and_bounds_the_tail() {
        take_output();
        line("log: \u{1b}[2J\r\tdata\nnext");
        assert_eq!(take_output(), ["log: �[2J� data", "next"]);
        for index in 0..300 {
            linef(format_args!("{index}"));
        }
        let lines = take_output();
        assert_eq!(lines.len(), OUTPUT_LINES);
        assert_eq!(lines.first().map(String::as_str), Some("44"));
        assert_eq!(lines.last().map(String::as_str), Some("299"));
        line(&"🦀".repeat(LINE_CHARS + 1));
        assert_eq!(take_output()[0].chars().count(), LINE_CHARS);
    }

    #[test]
    fn input_keeps_partial_lines_and_handles_crlf() {
        let mut input = ShellInput::default();
        for byte in b"https://example.com/" {
            assert!(input.push(*byte).is_none());
        }
        assert_eq!(input.push(b'\r'), Some(Ok("https://example.com/".into())));
        assert!(input.push(b'\n').is_none());
    }
    #[test]
    fn long_line_is_rejected_and_next_url_is_accepted() {
        let mut input = ShellInput::default();
        for _ in 0..4097 {
            input.push(b'x');
        }
        assert!(input.push(b'\n').unwrap().is_err());
        input.push(b'a');
        assert_eq!(input.push(b'\n'), Some(Ok("a".into())));
    }
    #[test]
    fn only_valid_https_urls_are_queued() {
        assert_eq!(
            page_url(" https://example.com/path?q=1#part "),
            Ok("https://example.com/path?q=1".into())
        );
        for input in [
            "http://example.com/",
            "file:///tmp/test",
            "https://example.com/ extra",
            "not a url",
        ] {
            assert!(page_url(input).is_err());
        }
    }
}
