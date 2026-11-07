extern crate alloc;
extern crate self as trueos;
pub type Result<T> = core::result::Result<T, alloc::string::String>;
#[path = "generated_native_terminal.rs"]
mod actual;
pub mod vshell {
    use std::{cell::RefCell, collections::VecDeque};
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Lease {
        Active,
        Parked,
        ReentryRequested,
    }
    #[derive(Clone, Copy)]
    pub struct TerminalSurfaceSnapshot {
        pub generation: u64,
        pub columns: u32,
        pub rows: u32,
    }
    pub struct State {
        pub lease: Lease,
        pub instance: u64,
        pub generation: u64,
        pub acknowledgements: usize,
        pub input: VecDeque<u8>,
        pub output: Vec<u8>,
        pub write_limit: usize,
        pub reads: usize,
        pub writes: usize,
        pub park_after_write: Option<usize>,
    }
    impl Default for State {
        fn default() -> Self {
            Self {
                lease: Lease::Active,
                instance: 81,
                generation: 3,
                acknowledgements: 0,
                input: b"input".iter().copied().collect(),
                output: Vec::new(),
                write_limit: 2,
                reads: 0,
                writes: 0,
                park_after_write: None,
            }
        }
    }
    thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
    pub fn state<T>(f: impl FnOnce(&mut State) -> T) -> T {
        STATE.with(|s| f(&mut s.borrow_mut()))
    }
    pub fn reset(lease: Lease) {
        state(|s| {
            *s = State::default();
            s.lease = lease;
        });
    }
    pub fn terminal_surface_snapshot() -> core::result::Result<TerminalSurfaceSnapshot, ()> {
        state(|s| {
            match s.lease {
                Lease::Parked => return Err(()),
                Lease::ReentryRequested => {
                    s.lease = Lease::Active;
                    s.generation += 1;
                    s.acknowledgements += 1;
                }
                Lease::Active => {}
            }
            Ok(TerminalSurfaceSnapshot {
                generation: s.generation,
                columns: 91,
                rows: 27,
            })
        })
    }
    pub fn attached_read_available(out: &mut [u8]) -> usize {
        state(|s| {
            assert_eq!(s.lease, Lease::Active);
            s.reads += 1;
            let mut n = 0;
            for slot in out {
                let Some(byte) = s.input.pop_front() else {
                    break;
                };
                *slot = byte;
                n += 1;
            }
            n
        })
    }
    pub fn attached_write(bytes: &[u8]) -> usize {
        state(|s| {
            assert_eq!(s.lease, Lease::Active);
            s.writes += 1;
            let n = bytes.len().min(s.write_limit);
            s.output.extend_from_slice(&bytes[..n]);
            if s.park_after_write
                .is_some_and(|limit| s.output.len() >= limit)
            {
                s.park_after_write = None;
                s.lease = Lease::Parked;
            }
            n
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use actual::{Console, PlatformConsole, Terminal};
    use vshell::{Lease, reset, state};
    #[test]
    fn merely_parked_console_cannot_reclaim_input_or_output() {
        reset(Lease::Parked);
        let mut c = PlatformConsole::new();
        assert_eq!(c.read(&mut [0; 8]), 0);
        assert_eq!(c.write(b"retained"), 0);
        assert_eq!(c.size(), (80, 24));
        state(|s| {
            assert_eq!(s.lease, Lease::Parked);
            assert_eq!(s.acknowledgements, 0);
            assert_eq!(s.input.iter().copied().collect::<Vec<_>>(), b"input");
            assert_eq!(s.reads, 0);
            assert_eq!(s.writes, 0);
            assert!(s.output.is_empty());
        });
    }
    #[test]
    fn explicit_reentry_read_acks_once_and_retains_same_instance_and_suffix() {
        reset(Lease::ReentryRequested);
        let mut c = PlatformConsole::new();
        let mut b = [0; 3];
        assert_eq!(c.read(&mut b), 3);
        assert_eq!(&b, b"inp");
        assert_eq!(c.read(&mut b), 2);
        assert_eq!(&b[..2], b"ut");
        state(|s| {
            assert_eq!(s.acknowledgements, 1);
            assert_eq!(s.instance, 81);
            assert_eq!(s.generation, 4);
        });
    }
    #[tokio::test]
    async fn retained_output_reentry_works_without_any_input_poll() {
        reset(Lease::Parked);
        let mut terminal = Terminal::new(PlatformConsole::new());
        assert_eq!(terminal.write(b"final-output").unwrap(), 0);
        state(|s| s.lease = Lease::ReentryRequested);
        terminal.write_all(b"final-output", None).await.unwrap();
        state(|s| {
            assert_eq!(s.output, b"final-output");
            assert_eq!(s.acknowledgements, 1);
            assert_eq!(s.reads, 0);
            assert_eq!(s.instance, 81);
        });
    }
    #[test]
    fn geometry_poll_acknowledges_request_and_returns_one_coherent_snapshot() {
        reset(Lease::ReentryRequested);
        let c = PlatformConsole::new();
        assert_eq!(c.size(), (91, 27));
        state(|s| {
            assert_eq!(s.acknowledgements, 1);
            assert_eq!(s.reads, 0);
            assert_eq!(s.writes, 0);
            assert_eq!(s.instance, 81);
        });
    }
    async fn prompt_reentry(
        prompt: &str,
        echo: bool,
        prefix: &[u8],
        suffix: &[u8],
        expected: &str,
        visible_prefix: &[u8],
    ) {
        reset(Lease::Active);
        state(|s| s.input = prefix.iter().copied().collect());
        let mut terminal = Terminal::new(PlatformConsole::new());
        let reentry = async {
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                loop {
                    if state(|s| s.input.is_empty()) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("prefix was consumed");
            state(|s| s.lease = Lease::Parked);
            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            state(|s| {
                assert_eq!(s.lease, Lease::Parked);
                assert_eq!(s.acknowledgements, 0);
            });
            let offset = state(|s| {
                let offset = s.output.len();
                s.lease = Lease::ReentryRequested;
                s.input.extend(suffix);
                offset
            });
            offset
        };
        let (answer, offset) = tokio::join!(terminal.line(prompt, echo, None), reentry);
        assert_eq!(answer.unwrap().as_str(), expected);
        state(|s| {
            let redraw = [b"\r\x1b[2K".as_slice(), prompt.as_bytes(), visible_prefix].concat();
            assert!(
                s.output[offset..].starts_with(&redraw),
                "redraw bytes: {:?}",
                &s.output[offset..]
            );
            assert_eq!(s.acknowledgements, 1);
            assert_eq!(s.instance, 81);
            if !echo {
                assert!(
                    !s.output
                        .windows(expected.len())
                        .any(|part| part == expected.as_bytes()),
                    "secret answer reached terminal output"
                );
            }
        });
    }
    #[tokio::test]
    async fn park_during_first_short_prompt_write_restores_the_complete_prompt() {
        reset(Lease::Active);
        state(|s| {
            s.input.clear();
            s.park_after_write = Some(2);
        });
        let mut terminal = Terminal::new(PlatformConsole::new());
        let reentry = async {
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                loop {
                    if state(|s| s.lease == Lease::Parked) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("first short write parked the surface");
            state(|s| {
                assert_eq!(s.output, b"Tr");
                s.lease = Lease::ReentryRequested;
                s.input.extend(b"yes\r");
            });
        };
        let prompt = "Trust this host key? [yes/no] ";
        let (answer, ()) = tokio::join!(terminal.line(prompt, true, None), reentry);
        assert_eq!(answer.unwrap().as_str(), "yes");
        state(|s| {
            let redraw = [b"\r\x1b[2K".as_slice(), prompt.as_bytes(), b"yes\r\n"].concat();
            assert!(s.output.ends_with(&redraw));
            assert_eq!(s.acknowledgements, 1);
        });
    }
    #[tokio::test]
    async fn minishell_reentry_restores_visible_typed_prefix() {
        prompt_reentry("ssh> ", true, b"ho", b"st\r", "host", b"ho").await;
    }
    #[tokio::test]
    async fn host_trust_reentry_restores_fingerprint_context_and_visible_answer() {
        prompt_reentry("The authenticity of host is unknown.\r\nFingerprint is SHA256:fixture.\r\nTrust this host key? [yes/no] ",true,b"y",b"es\r","yes",b"y").await;
    }
    #[tokio::test]
    async fn visible_kbi_reentry_restores_only_complete_utf8_prefix() {
        prompt_reentry(
            "Challenge name\r\nChallenge instructions\r\nVisible answer: ",
            true,
            "é".as_bytes(),
            b"x\r",
            "éx",
            "é".as_bytes(),
        )
        .await;
    }
    #[tokio::test]
    async fn incomplete_utf8_prefix_is_not_replayed_before_its_continuation() {
        prompt_reentry("Visible answer: ", true, b"\xc3", b"\xa9\r", "é", b"").await;
    }
    #[tokio::test]
    async fn password_reentry_restores_question_but_never_secret_prefix() {
        prompt_reentry(
            "user@host password: ",
            false,
            b"secret",
            b"-password\r",
            "secret-password",
            b"",
        )
        .await;
    }
    #[tokio::test]
    async fn secret_kbi_reentry_restores_context_without_secret_answer() {
        prompt_reentry(
            "Challenge name\r\nChallenge instructions\r\nOTP: ",
            false,
            b"private",
            b"-OTP\r",
            "private-OTP",
            b"",
        )
        .await;
    }
}
