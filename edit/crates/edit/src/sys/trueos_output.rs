// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Output through the Blueprint's attached native console.

unsafe extern "C" {
    fn trueos_cabi_shell_attached_write(bytes: *const u8, len: usize) -> usize;
}

pub fn write_stdout(text: &str) {
    let mut remaining = text.as_bytes();
    while !remaining.is_empty() {
        // The native console consumes the borrowed bytes during this call.
        let written =
            unsafe { trueos_cabi_shell_attached_write(remaining.as_ptr(), remaining.len()) };
        if written == 0 || written > remaining.len() {
            return;
        }
        remaining = &remaining[written..];
    }
}
