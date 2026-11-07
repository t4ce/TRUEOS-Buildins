use alloc::vec::Vec;
#[derive(Default)]
pub struct LocalEscape {
    at_line_start: bool,
    pending_tilde: bool,
}

impl LocalEscape {
    pub fn new() -> Self {
        Self {
            at_line_start: true,
            pending_tilde: false,
        }
    }

    pub fn forward(&mut self, input: &[u8], output: &mut Vec<u8>) -> bool {
        for &byte in input {
            if self.pending_tilde {
                self.pending_tilde = false;
                match byte {
                    b'.' => return true,
                    b'~' => {
                        output.push(b'~');
                        self.at_line_start = false;
                        continue;
                    }
                    _ => {
                        output.push(b'~');
                        self.at_line_start = false;
                    }
                }
            } else if self.at_line_start && byte == b'~' {
                self.pending_tilde = true;
                continue;
            }
            output.push(byte);
            self.at_line_start = matches!(byte, b'\r' | b'\n');
        }
        false
    }
}

