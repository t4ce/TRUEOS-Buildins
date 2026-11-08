//! Console bytes remain owned until consumed; local prompts never read ahead.
use crate::Result;
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::time::Duration;
use tokio::time::Instant;
use zeroize::Zeroizing;
pub const POLL: Duration = Duration::from_millis(5);
pub trait Console {
    fn read(&mut self, out: &mut [u8]) -> usize;
    fn write(&mut self, bytes: &[u8]) -> usize;
    fn size(&self) -> (u32, u32);
    fn surface_generation(&self) -> Option<u64> {
        None
    }
}
pub struct Terminal<C> {
    pub console: C,
    skip_lf: bool,
}
impl<C: Console> Terminal<C> {
    pub fn new(console: C) -> Self {
        Self {
            console,
            skip_lf: false,
        }
    }
    pub fn size(&self) -> (u32, u32) {
        self.console.size()
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        let n = self.console.write(bytes);
        if n > bytes.len() {
            return Err("ssh: invalid terminal write count".into());
        }
        Ok(n)
    }
    pub fn read(&mut self, out: &mut [u8]) -> Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.skip_lf {
            let mut byte = [0];
            let n = self.console.read(&mut byte);
            if n > 1 {
                return Err("ssh: invalid terminal read count".into());
            }
            if n == 0 {
                return Ok(0);
            }
            self.skip_lf = false;
            if byte[0] != b'\n' {
                out[0] = byte[0];
                return Ok(1);
            }
        }
        let n = self.console.read(out);
        if n > out.len() {
            Err("ssh: invalid terminal read count".into())
        } else {
            Ok(n)
        }
    }
    pub async fn write_all(&mut self, mut bytes: &[u8], deadline: Option<Instant>) -> Result<()> {
        while !bytes.is_empty() {
            check_deadline(deadline)?;
            let n = self.write(bytes)?;
            bytes = &bytes[n..];
            if n == 0 {
                tokio::time::sleep(POLL).await
            }
        }
        Ok(())
    }
    pub async fn line(
        &mut self,
        prompt: &str,
        echo: bool,
        deadline: Option<Instant>,
    ) -> Result<Zeroizing<String>> {
        let mut surface_generation = self.console.surface_generation();
        let mut surface_size = self.console.size();
        self.write_all(prompt.as_bytes(), deadline).await?;
        let mut bytes = Zeroizing::new(Vec::new());
        let mut echoed = 0;
        let mut escape = 0;
        loop {
            check_deadline(deadline)?;
            let mut byte = [0];
            let count = self.read(&mut byte)?;
            let current = self.console.surface_generation();
            let current_size = self.console.size();
            if (current.is_some() && current != surface_generation) || current_size != surface_size {
                surface_generation = current;
                surface_size = current_size;
                self.write_all(b"\r\x1b[2K", deadline).await?;
                self.write_all(prompt.as_bytes(), deadline).await?;
                // Only the complete UTF8 prefix already shown to the user is
                // presentation state. Password/KBI secret answers stay hidden.
                if echo {
                    self.write_all(&bytes[..echoed], deadline).await?;
                }
            }
            if count == 0 {
                tokio::time::sleep(POLL).await;
                continue;
            }
            let byte = byte[0];
            if escape != 0 {
                if escape == 1 && matches!(byte, b'[' | b'O') {
                    escape = 2
                } else if escape == 1 || (0x40..=0x7e).contains(&byte) {
                    escape = 0
                }
                continue;
            }
            match byte {
                b'\r' | b'\n' => {
                    self.skip_lf = byte == b'\r';
                    self.write_all(b"\r\n", deadline).await?;
                    let text = core::str::from_utf8(&bytes)
                        .map_err(|_| "ssh: terminal response is not UTF8")?;
                    return Ok(Zeroizing::new(text.to_string()));
                }
                3 => return Err("ssh: cancelled".into()),
                8 | 127 => {
                    if let Some(last) = bytes.pop() {
                        if last & 0xc0 == 0x80 {
                            while bytes.last().is_some_and(|b| *b & 0xc0 == 0x80) {
                                bytes.pop();
                            }
                            bytes.pop();
                        }
                        if bytes.len() < echoed {
                            if echo {
                                self.write_all(b"\x08 \x08", deadline).await?
                            }
                            echoed = bytes.len()
                        }
                    }
                }
                27 => escape = 1,
                0..=31 => {}
                _ => {
                    if bytes.len() >= 32 * 1024 {
                        return Err("ssh: terminal response exceeds 32KiB".into());
                    }
                    bytes
                        .try_reserve(1)
                        .map_err(|_| "ssh: cannot allocate terminal response")?;
                    bytes.push(byte);
                    match core::str::from_utf8(&bytes[echoed..]) {
                        Ok(text) => {
                            if echo {
                                self.write_all(text.as_bytes(), deadline).await?
                            }
                            echoed = bytes.len()
                        }
                        Err(e) if e.error_len().is_none() => {}
                        Err(_) => return Err("ssh: terminal response is not UTF8".into()),
                    }
                }
            }
        }
    }
}
pub fn check_deadline(deadline: Option<Instant>) -> Result<()> {
    if deadline.is_some_and(|d| Instant::now() >= d) {
        Err("ssh: operation timed out".into())
    } else {
        Ok(())
    }
}

pub struct PlatformConsole {
    #[cfg(not(target_os = "trueos"))]
    input: std::sync::mpsc::Receiver<u8>,
}
impl PlatformConsole {
    pub fn new() -> Self {
        #[cfg(target_os = "trueos")]
        {
            Self {}
        }
        #[cfg(not(target_os = "trueos"))]
        {
            let (tx, input) = std::sync::mpsc::sync_channel(4096);
            std::thread::spawn(move || {
                use std::io::Read;
                let mut stdin = std::io::stdin().lock();
                let mut byte = [0];
                while stdin.read(&mut byte).ok() == Some(1) {
                    if tx.send(byte[0]).is_err() {
                        break;
                    }
                }
            });
            Self { input }
        }
    }
}
impl Console for PlatformConsole {
    fn surface_generation(&self) -> Option<u64> {
        #[cfg(target_os = "trueos")]
        {
            trueos::vshell::terminal_surface_snapshot()
                .ok()
                .map(|surface| surface.generation)
        }
        #[cfg(not(target_os = "trueos"))]
        {
            None
        }
    }
    fn read(&mut self, out: &mut [u8]) -> usize {
        #[cfg(target_os = "trueos")]
        {
            // Surface polling acknowledges an explicit Shell3 `tui` request.
            // A merely parked terminal remains inactive and keeps its input.
            if trueos::vshell::terminal_surface_snapshot().is_err() {
                return 0;
            }
            trueos::vshell::attached_read_available(out)
        }
        #[cfg(not(target_os = "trueos"))]
        {
            let mut n = 0;
            for byte in out {
                let Ok(b) = self.input.try_recv() else { break };
                *byte = b;
                n += 1
            }
            n
        }
    }
    fn write(&mut self, bytes: &[u8]) -> usize {
        #[cfg(target_os = "trueos")]
        {
            // Reentry must also work while final remote output is retained and
            // no input is being read. Zero preserves that output while parked.
            if trueos::vshell::terminal_surface_snapshot().is_err() {
                return 0;
            }
            trueos::vshell::attached_write(bytes)
        }
        #[cfg(not(target_os = "trueos"))]
        {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            let n = out.write(bytes).unwrap_or(0);
            let _ = out.flush();
            n
        }
    }
    fn size(&self) -> (u32, u32) {
        #[cfg(target_os = "trueos")]
        {
            trueos::vshell::terminal_surface_snapshot()
                .map(|s| (s.columns, s.rows))
                .unwrap_or((80, 24))
        }
        #[cfg(not(target_os = "trueos"))]
        {
            (80, 24)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Fake {
        input: VecDeque<u8>,
        output: Vec<u8>,
        write_limit: usize,
        block_once: bool,
    }
    impl Fake {
        fn new(bytes: &[u8]) -> Self {
            Self {
                input: bytes.iter().copied().collect(),
                output: Vec::new(),
                write_limit: 1,
                block_once: true,
            }
        }
    }
    impl Console for Fake {
        fn read(&mut self, out: &mut [u8]) -> usize {
            let mut n = 0;
            for slot in out {
                let Some(b) = self.input.pop_front() else {
                    break;
                };
                *slot = b;
                n += 1
            }
            n
        }
        fn write(&mut self, bytes: &[u8]) -> usize {
            if self.block_once {
                self.block_once = false;
                return 0;
            }
            let n = bytes.len().min(self.write_limit);
            self.output.extend_from_slice(&bytes[..n]);
            n
        }
        fn size(&self) -> (u32, u32) {
            (80, 24)
        }
    }
    #[tokio::test]
    async fn prompts_keep_crlf_suffix_and_secret_answers_never_echo() {
        let mut t = Terminal::new(Fake::new(b"secret\r\nnext\r"));
        let secret = t.line("Password: ", false, None).await.unwrap();
        assert_eq!(secret.as_str(), "secret");
        let next = t.line("next> ", true, None).await.unwrap();
        assert_eq!(next.as_str(), "next");
        let text = core::str::from_utf8(&t.console.output).unwrap();
        assert_eq!(text, "Password: \r\nnext> next\r\n");
    }
    #[tokio::test]
    async fn utf8_backspace_and_invalid_input_are_safe() {
        let mut t = Terminal::new(Fake::new("é\u{7f}ok\u{1b}[D\r".as_bytes()));
        assert_eq!(t.line("", true, None).await.unwrap().as_str(), "ok");
        let mut invalid = Terminal::new(Fake::new(b"\xff\r"));
        assert!(invalid.line("", false, None).await.is_err());
    }
    #[tokio::test]
    async fn cr_does_not_consume_next_sessions_keystrokes() {
        let mut t = Terminal::new(Fake::new(b"\rnext"));
        assert!(t.line("", true, None).await.unwrap().is_empty());
        let mut bytes = [0; 8];
        let n = t.read(&mut bytes).unwrap();
        assert_eq!(&bytes[..n], b"n");
        let n = t.read(&mut bytes).unwrap();
        assert_eq!(&bytes[..n], b"ext");
    }
    #[tokio::test]
    async fn control_c_and_expired_deadline_cancel_prompt() {
        let mut t = Terminal::new(Fake::new(b"\x03"));
        assert!(
            t.line("", false, None)
                .await
                .unwrap_err()
                .contains("cancelled")
        );
        let mut t = Terminal::new(Fake::new(b""));
        assert!(
            t.line("", false, Some(Instant::now()))
                .await
                .unwrap_err()
                .contains("timed out")
        );
    }

    struct ChangingSurface {
        inner: Fake,
        reads: usize,
        resize_at: Option<usize>,
        replace_at: Option<usize>,
        generation: Option<u64>,
    }
    impl ChangingSurface {
        fn new(input: &[u8]) -> Self {
            let mut inner = Fake::new(input);
            inner.block_once = false;
            inner.write_limit = usize::MAX;
            Self {
                inner,
                reads: 0,
                resize_at: None,
                replace_at: None,
                generation: Some(7),
            }
        }
    }
    impl Console for ChangingSurface {
        fn read(&mut self, out: &mut [u8]) -> usize {
            self.reads += 1;
            self.inner.read(out)
        }
        fn write(&mut self, bytes: &[u8]) -> usize {
            self.inner.write(bytes)
        }
        fn size(&self) -> (u32, u32) {
            if self.resize_at.is_some_and(|at| self.reads >= at) {
                (132, 48)
            } else {
                (80, 24)
            }
        }
        fn surface_generation(&self) -> Option<u64> {
            self.generation.map(|generation| {
                generation + u64::from(self.replace_at.is_some_and(|at| self.reads >= at))
            })
        }
    }
    #[tokio::test]
    async fn resize_redraws_complete_utf8_without_requiring_a_new_identity() {
        for generation in [Some(7), None] {
            let mut console = ChangingSurface::new("éok\r".as_bytes());
            console.generation = generation;
            console.resize_at = Some(3);
            let mut t = Terminal::new(console);
            assert_eq!(t.line("next> ", true, None).await.unwrap().as_str(), "éok");
            assert_eq!(t.console.inner.output, "next> é\r\x1b[2Knext> éok\r\n".as_bytes());
        }
    }
    #[tokio::test]
    async fn resize_does_not_reveal_secret_answers() {
        let mut console = ChangingSurface::new(b"secret\r");
        console.resize_at = Some(4);
        let mut t = Terminal::new(console);
        assert_eq!(t.line("Password: ", false, None).await.unwrap().as_str(), "secret");
        assert_eq!(t.console.inner.output, b"Password: \r\x1b[2KPassword: \r\n");
    }
    #[tokio::test]
    async fn replacement_still_redraws_at_unchanged_geometry() {
        let mut console = ChangingSurface::new(b"ok\r");
        console.replace_at = Some(2);
        let mut t = Terminal::new(console);
        assert_eq!(t.line("> ", true, None).await.unwrap().as_str(), "ok");
        assert_eq!(t.console.inner.output, b"> o\r\x1b[2K> ok\r\n");
    }
    #[tokio::test]
    async fn unchanged_surface_does_not_repeat_prompt() {
        let mut t = Terminal::new(ChangingSurface::new(b"ok\r"));
        assert_eq!(t.line("> ", true, None).await.unwrap().as_str(), "ok");
        assert_eq!(t.console.inner.output, b"> ok\r\n");
    }
}
