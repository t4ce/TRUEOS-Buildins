//! Minimal line entry over native raw console bytes, without read-ahead.
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_LINE_BYTES: usize = 32 * 1024;

pub(crate) struct LineReader<R> {
    reader: R,
    skip_lf: bool,
}
impl<R: AsyncRead + Unpin> LineReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            skip_lf: false,
        }
    }

    pub async fn read_line(
        &mut self,
        mut echo: impl FnMut(&str),
    ) -> Result<Option<String>, String> {
        let mut line = Vec::new();
        let mut echoed = 0;
        let mut escape = 0;
        loop {
            // One byte avoids taking the next command's keys out of the native
            // console queue when this line returns its terminal lease to Shell.
            let mut byte = [0];
            if self
                .reader
                .read(&mut byte)
                .await
                .map_err(|e| e.to_string())?
                == 0
            {
                return if line.is_empty() {
                    Ok(None)
                } else {
                    finish(line).map(Some)
                };
            }
            let byte = byte[0];
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if escape != 0 {
                if escape == 1 && matches!(byte, b'[' | b'O') {
                    escape = 2;
                } else if escape == 1 || (0x40..=0x7e).contains(&byte) {
                    escape = 0;
                }
                continue;
            }
            match byte {
                b'\r' | b'\n' => {
                    self.skip_lf = byte == b'\r';
                    echo("\r\n");
                    return finish(line).map(Some);
                }
                3 => {
                    echo("^C\r\n");
                    return Ok(Some(String::new()));
                }
                4 => {
                    return if line.is_empty() {
                        Ok(None)
                    } else {
                        finish(line).map(Some)
                    };
                }
                8 | 127 => {
                    if let Some(last) = line.pop() {
                        if last & 0xc0 == 0x80 {
                            while line.last().is_some_and(|byte| *byte & 0xc0 == 0x80) {
                                line.pop();
                            }
                            line.pop();
                        }
                        if line.len() < echoed {
                            echo("\x08 \x08");
                            echoed = line.len();
                        }
                    }
                }
                21 => {
                    for _ in core::str::from_utf8(&line[..echoed]).unwrap_or("").chars() {
                        echo("\x08 \x08");
                    }
                    line.clear();
                    echoed = 0;
                }
                27 => escape = 1,
                0..=31 => {}
                _ => {
                    if line.len() == MAX_LINE_BYTES {
                        return Err("URL input exceeds32KiB".into());
                    }
                    line.try_reserve(1)
                        .map_err(|_| "cannot allocate URL input")?;
                    line.push(byte);
                    match core::str::from_utf8(&line[echoed..]) {
                        Ok(text) => {
                            echo(text);
                            echoed = line.len();
                        }
                        Err(error) if error.error_len().is_none() => {}
                        Err(_) => return Err("URL input is not UTF8".into()),
                    }
                }
            }
        }
    }
}
fn finish(line: Vec<u8>) -> Result<String, String> {
    String::from_utf8(line).map_err(|_| "URL input is not UTF8".into())
}
