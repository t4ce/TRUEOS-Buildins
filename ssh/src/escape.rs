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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_escape_only_disconnects_at_line_start() {
        let mut e = LocalEscape::new();
        let mut out = Vec::new();
        assert!(!e.forward(b"echo ~. stays\r\n", &mut out));
        assert_eq!(out, b"echo ~. stays\r\n");
        out.clear();
        assert!(e.forward(b"~.", &mut out));
        assert!(out.is_empty());
    }
    #[test]
    fn fragmented_tilde_and_literal_tilde_keep_exact_bytes() {
        let mut e = LocalEscape::new();
        let mut out = Vec::new();
        assert!(!e.forward(b"~", &mut out));
        assert!(out.is_empty());
        assert!(!e.forward(b"~x\r~", &mut out));
        assert_eq!(out, b"~x\r");
        assert!(!e.forward(b"y", &mut out));
        assert_eq!(out, b"~x\r~y");
    }
}
