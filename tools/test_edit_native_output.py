#!/usr/bin/env python3
"""Exercise Edit's production native output loop at its attached-console ABI."""
from pathlib import Path
import subprocess
import tempfile


SOURCE = Path(__file__).resolve().parents[1] / "edit/crates/edit/src/sys/trueos_output.rs"
HARNESS = r'''
#![forbid(unsafe_op_in_unsafe_fn)]
#[path = PRODUCTION_PATH]
mod production;
use std::{cell::RefCell, collections::VecDeque};

#[derive(Default)]
struct Console {
    counts: VecDeque<usize>,
    calls: Vec<Vec<u8>>,
    output: Vec<u8>,
}
thread_local! { static CONSOLE: RefCell<Console> = RefCell::default(); }

#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_shell_attached_write(bytes: *const u8, len: usize) -> usize {
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    CONSOLE.with_borrow_mut(|console| {
        console.calls.push(bytes.to_vec());
        let count = console.counts.pop_front().unwrap_or(len);
        if count <= len { console.output.extend_from_slice(&bytes[..count]); }
        count
    })
}

fn run(text: &str, counts: &[usize]) -> Console {
    CONSOLE.with_borrow_mut(|console| *console = Console {
        counts: counts.iter().copied().collect(), ..Console::default()
    });
    production::write_stdout(text);
    CONSOLE.with_borrow_mut(std::mem::take)
}

#[test]
fn empty_output_does_not_call_console() {
    let result = run("", &[]);
    assert!(result.calls.is_empty());
    assert!(result.output.is_empty());
}

#[test]
fn complete_output_keeps_ansi_utf8_and_nul_bytes() {
    let text = "\x1b[2;7Hλ🦀\0\r\n";
    let result = run(text, &[]);
    assert_eq!(result.calls, [text.as_bytes()]);
    assert_eq!(result.output, text.as_bytes());
}

#[test]
fn short_writes_advance_bytes_including_inside_utf8() {
    let text = "λ🦀\x1b[0m\0tail";
    let result = run(text, &[1, 2, 1]);
    assert_eq!(result.calls, [
        &text.as_bytes()[0..], &text.as_bytes()[1..],
        &text.as_bytes()[3..], &text.as_bytes()[4..],
    ]);
    assert_eq!(result.output, text.as_bytes());
}

#[test]
fn zero_progress_returns_without_retrying() {
    let first = run("hello", &[0, 1]);
    assert_eq!(first.calls.len(), 1);
    assert!(first.output.is_empty());
    let later = run("hello", &[2, 0, 1]);
    assert_eq!(later.calls, [b"hello".as_slice(), b"llo".as_slice()]);
    assert_eq!(later.output, b"he");
}

#[test]
fn invalid_count_returns_without_advancing_or_retrying() {
    let first = run("hello", &[6, 1]);
    assert_eq!(first.calls.len(), 1);
    assert!(first.output.is_empty());
    let later = run("hello", &[2, usize::MAX, 1]);
    assert_eq!(later.calls, [b"hello".as_slice(), b"llo".as_slice()]);
    assert_eq!(later.output, b"he");
}
'''


def main():
    with tempfile.TemporaryDirectory(prefix="edit-native-output-") as directory:
        source = Path(directory) / "tests.rs"
        binary = Path(directory) / "tests"
        source.write_text(HARNESS.replace("PRODUCTION_PATH", '"' + str(SOURCE) + '"'))
        subprocess.run(
            ["rustc", "--edition=2024", "--test", str(source), "-o", str(binary)],
            check=True,
        )
        subprocess.run([str(binary)], check=True, timeout=10)


if __name__ == "__main__":
    main()
