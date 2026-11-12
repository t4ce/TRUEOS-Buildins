#!/usr/bin/env python3
"""Exercise img's production startup/prompt row handlers without a viewer.

Only frame loading, terminal input and output are host adapters. The actual
run_line and service_command_channel bodies execute unchanged; no filesystem,
codec, UI4 rendering or native hardware result is inferred from these tests.
"""

from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def item(source: str, signature: str) -> str:
    start = source.index(signature)
    opening = source.index("{", start)
    depth, end = 1, opening + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end] + "\n"


HARNESS = r'''
extern crate self as trueos;
use std::{collections::VecDeque, sync::Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Alignment { Center, TopLeft, TopRight, BottomLeft, BottomRight }
#[derive(Debug)]
struct OpenFrame { source: String, alignment: Alignment, hit: bool }
static INPUT: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub mod vshell {
    pub fn attached_read_available(output: &mut [u8]) -> usize {
        let mut input = crate::INPUT.lock().unwrap();
        let length = output.len().min(input.len());
        for byte in &mut output[..length] { *byte = input.pop_front().unwrap(); }
        length
    }
}
mod level { pub const WARN: u8 = 1; }
mod logl {
    pub fn log(_: u8, message: core::fmt::Arguments<'_>) {
        crate::LOGS.lock().unwrap().push(message.to_string());
    }
}
fn terminal_line(text: &str) { LINES.lock().unwrap().push(text.to_string()); }
fn list_frames(_: &[OpenFrame]) { terminal_line("list"); }
fn convert_frame(_: &[OpenFrame]) { terminal_line("convert"); }
fn show_source(path: &str, alignment: Alignment, hit: bool, frames: &mut Vec<OpenFrame>) {
    frames.push(OpenFrame { source: path.to_string(), alignment, hit });
}
__ROW_HANDLER__
__PROMPT_HANDLER__

fn reset() {
    INPUT.lock().unwrap().clear();
    LINES.lock().unwrap().clear();
    LOGS.lock().unwrap().clear();
}
fn feed(text: &str) { INPUT.lock().unwrap().extend(text.bytes()); }

#[test]
fn bare_source_rows_keep_literal_spaces_unicode_and_punctuation() {
    reset();
    let paths = ["/apps/common/tree-with-bird.png", "apps/common/tree with bird.png",
                 "apps/common/a;$(literal).png", "kernel:logo", "tree with nohit",
                 "apps/common/鳥.png"];
    let mut frames = Vec::new();
    for path in paths { run_line(path, &mut frames); }
    assert_eq!(frames.len(), paths.len());
    for (frame, path) in frames.iter().zip(paths) {
        assert_eq!(frame.source, path);
        assert_eq!(frame.alignment, Alignment::Center);
        assert!(frame.hit);
    }
    assert!(LOGS.lock().unwrap().is_empty());
}

#[test]
fn native_script_rows_keep_comments_scope_and_explicit_show_options() {
    reset();
    let script = "# image startup\nfs-scope trueosfs\n\napps/common/tree with bird.png\nshow apps/common/second.png top-right nohit\n";
    let mut frames = Vec::new();
    for row in script.lines() { run_line(row, &mut frames); }
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].source, "apps/common/tree with bird.png");
    assert_eq!(frames[0].alignment, Alignment::Center);
    assert!(frames[0].hit);
    assert_eq!(frames[1].source, "apps/common/second.png");
    assert_eq!(frames[1].alignment, Alignment::TopRight);
    assert!(!frames[1].hit);
    assert!(LOGS.lock().unwrap().is_empty());
}

#[test]
fn legacy_show_options_still_apply_in_order() {
    reset();
    let mut frames = Vec::new();
    run_line("show common/tree.png top-left nohit bottom-right hit", &mut frames);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].source, "common/tree.png");
    assert_eq!(frames[0].alignment, Alignment::BottomRight);
    assert!(frames[0].hit);
}

#[test]
fn exact_show_reports_a_missing_source_instead_of_opening_its_name() {
    reset();
    let mut frames = Vec::new();
    run_line("show", &mut frames);
    run_line("  show  ", &mut frames);
    assert!(frames.is_empty());
    assert_eq!(LOGS.lock().unwrap().as_slice(),
        ["img: show requires a source", "img: show requires a source"]);
}

#[test]
fn unknown_explicit_show_options_are_still_rejected() {
    reset();
    let mut frames = Vec::new();
    run_line("show common/tree.png sideways", &mut frames);
    assert!(frames.is_empty());
    assert_eq!(LOGS.lock().unwrap().as_slice(), ["img: unknown show option=sideways"]);
}

#[test]
fn prompt_and_startup_use_the_same_literal_path_handler() {
    reset();
    let path = "apps/common/tree with bird.png";
    let mut frames = Vec::new();
    let mut command = Vec::new();
    feed(&format!("{path}\r\n"));
    assert!(!service_command_channel(&mut command, &mut frames));
    assert!(command.is_empty());
    assert_eq!(frames.len(), 1, "CRLF must not open the path twice");
    run_line(path, &mut frames);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].source, frames[1].source);
    assert_eq!(frames[0].alignment, frames[1].alignment);
    assert_eq!(frames[0].hit, frames[1].hit);
}

#[test]
fn partial_prompt_rows_wait_for_submission() {
    reset();
    let mut frames = Vec::new();
    let mut command = Vec::new();
    feed("apps/common/tree");
    assert!(!service_command_channel(&mut command, &mut frames));
    assert!(frames.is_empty());
    feed(" with bird.png\n");
    assert!(!service_command_channel(&mut command, &mut frames));
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].source, "apps/common/tree with bird.png");
}

#[test]
fn existing_prompt_controls_keep_their_behavior() {
    reset();
    let mut frames = Vec::new();
    run_line("common/tree.png", &mut frames);
    let mut command = Vec::new();
    feed("help\nlist\nconvert\nclose all\nexit\n");
    assert!(service_command_channel(&mut command, &mut frames));
    assert!(frames.is_empty());
    let lines = LINES.lock().unwrap();
    assert!(lines[0].contains("PATH | show PATH"));
    assert!(lines.iter().any(|line| line == "list"));
    assert!(lines.iter().any(|line| line == "convert"));
    assert!(lines.iter().any(|line| line == "img: all frames closed"));
}
'''


def main() -> None:
    source = (ROOT / "img/src/main.rs").read_text()
    harness = HARNESS.replace("__ROW_HANDLER__", item(source, "fn run_line("))
    harness = harness.replace("__PROMPT_HANDLER__", item(source, "fn service_command_channel("))
    with tempfile.TemporaryDirectory(prefix="trueos-img-native-rows-") as temporary:
        directory = Path(temporary)
        rust = directory / "tests.rs"
        rust.write_text(harness)
        for profile, flags in [("debug", []), ("optimized", ["-O"])]:
            executable = directory / profile
            subprocess.run(["rustc", "--edition=2024", "--test", str(rust), *flags,
                            "-o", str(executable)], check=True)
            print(f"img-native-rows profile={profile}", flush=True)
            subprocess.run([str(executable), "--test-threads=1"], check=True)


if __name__ == "__main__":
    main()
