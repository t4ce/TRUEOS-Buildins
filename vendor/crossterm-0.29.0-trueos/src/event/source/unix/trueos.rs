#[allow(unused_imports)]
use crate::alloc_prelude::*;
use super::parser::Parser;
use crate::io;
use core::time::Duration;

use crate::event::{source::EventSource, timeout::PollTimeout, Event, InputDiscardStatus, InternalEvent};
#[cfg(feature = "event-stream")]
use crate::event::sys::Waker;

const BUFFER_SIZE: usize = 1024;

pub(crate) trait TerminalTransport: Send + Sync {
    fn observe(&self) -> u32;
    fn surface(&self) -> io::Result<Option<(u64, u32, u32)>>;
    fn read(&self, bytes: &mut [u8]) -> io::Result<usize>;
    fn wait_after(&self, observed: u32, timeout_ms: u64) -> io::Result<()>;
}

#[cfg(target_os = "trueos")]
pub(crate) struct AttachedTerminal;

#[cfg(target_os = "trueos")]
impl TerminalTransport for AttachedTerminal {
    fn observe(&self) -> u32 { crate::trueos::observe() }
    fn surface(&self) -> io::Result<Option<(u64, u32, u32)>> { crate::trueos::surface() }
    fn read(&self, bytes: &mut [u8]) -> io::Result<usize> { crate::trueos::read(bytes) }
    fn wait_after(&self, observed: u32, timeout_ms: u64) -> io::Result<()> {
        crate::trueos::wait_after(observed, timeout_ms)
    }
}

pub(crate) struct TerminalEventSource<T> {
    transport: T,
    parser: Parser,
    buffer: [u8; BUFFER_SIZE],
    surface: Option<(u64, u32, u32)>,
    #[cfg(feature = "event-stream")]
    waker: Waker,
}

#[cfg(target_os = "trueos")]
pub(crate) type NativeInternalEventSource = TerminalEventSource<AttachedTerminal>;

#[cfg(target_os = "trueos")]
impl NativeInternalEventSource {
    pub(crate) fn new() -> io::Result<Self> {
        crate::trueos::claim()?;
        Self::with_transport(AttachedTerminal)
    }
}

impl<T: TerminalTransport> TerminalEventSource<T> {
    fn with_transport(transport: T) -> io::Result<Self> {
        let surface = transport.surface()?;
        Ok(Self {
            transport,
            parser: Parser::default(),
            buffer: [0; BUFFER_SIZE],
            surface,
            #[cfg(feature = "event-stream")]
            waker: Waker::new(),
        })
    }

    fn surface_event(&mut self) -> io::Result<Option<InternalEvent>> {
        let Some(current) = self.transport.surface()? else { return Ok(None); };
        let previous = self.surface.replace(current);
        if previous.is_some_and(|previous| previous != current) {
            if previous.is_some_and(|previous| previous.0 != current.0) {
                // Bytes and partial escapes belong to the transport incarnation
                // that supplied them. Never replay them after lease reentry.
                self.parser = Parser::default();
            }
            return Ok(Some(InternalEvent::Event(Event::Resize(
                current.1.min(u32::from(u16::MAX)) as u16,
                current.2.min(u32::from(u16::MAX)) as u16,
            ))));
        }
        Ok(None)
    }
}

fn timeout_ms(duration: Option<Duration>) -> u64 {
    duration.map_or(u64::MAX, |duration| {
        duration.as_nanos().div_ceil(1_000_000).min(u128::from(u64::MAX)) as u64
    })
}

impl<T: TerminalTransport> EventSource for TerminalEventSource<T> {
    fn try_read(&mut self, timeout: Option<Duration>) -> io::Result<Option<InternalEvent>> {
        let timeout = PollTimeout::new(timeout);
        loop {
            // Observe before all readiness probes, including surface state.
            // The subsequent wait cannot lose input, resize, or reentry that
            // races either probe or registration of the native wait.
            let observed = self.transport.observe();
            if let Some(event) = self.surface_event()? { return Ok(Some(event)); }
            if let Some(event) = self.parser.next() { return Ok(Some(event)); }
            #[cfg(feature = "event-stream")]
            if self.waker.take() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "terminal poll was woken"));
            }

            let count = self.transport.read(&mut self.buffer)?;
            if count > self.buffer.len() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid terminal read count"));
            }
            if count != 0 {
                self.parser.advance(&self.buffer[..count], count == self.buffer.len());
                if let Some(event) = self.parser.next() { return Ok(Some(event)); }
                if timeout.elapsed() { return Ok(None); }
                continue;
            }
            if timeout.elapsed() { return Ok(None); }
            self.transport.wait_after(observed, timeout_ms(timeout.leftover()))?;
        }
    }

    fn discard_buffered_input(&mut self) -> InputDiscardStatus {
        self.parser.discard_buffered_input()
    }

    #[cfg(feature = "event-stream")]
    fn waker(&self) -> Waker { self.waker.clone() }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::alloc_prelude::*;
    use super::*;
    use crate::event::KeyCode;
    use std::{collections::VecDeque, sync::{Arc, Mutex}};

    #[derive(Default)]
    struct State {
        generation: u32,
        surface: Option<(u64, u32, u32)>,
        bytes: VecDeque<u8>,
        after_observe: Option<Vec<u8>>,
        on_wait: Option<Vec<u8>>,
        #[cfg(feature = "event-stream")]
        on_wait_waker: Option<Waker>,
        waits: Vec<(u32, u64)>,
        reads: Vec<usize>,
    }

    #[derive(Clone, Default)]
    struct Transport(Arc<Mutex<State>>);

    impl TerminalTransport for Transport {
        fn observe(&self) -> u32 {
            let mut state = self.0.lock().unwrap();
            let observed = state.generation;
            if let Some(bytes) = state.after_observe.take() {
                state.bytes.extend(bytes);
                state.generation += 1;
            }
            observed
        }
        fn surface(&self) -> io::Result<Option<(u64, u32, u32)>> {
            Ok(self.0.lock().unwrap().surface)
        }
        fn read(&self, bytes: &mut [u8]) -> io::Result<usize> {
            let mut state = self.0.lock().unwrap();
            let count = bytes.len().min(state.bytes.len());
            for slot in &mut bytes[..count] { *slot = state.bytes.pop_front().unwrap(); }
            state.reads.push(count);
            Ok(count)
        }
        fn wait_after(&self, observed: u32, timeout_ms: u64) -> io::Result<()> {
            let mut state = self.0.lock().unwrap();
            state.waits.push((observed, timeout_ms));
            if let Some(bytes) = state.on_wait.take() {
                state.bytes.extend(bytes);
                state.generation += 1;
            }
            #[cfg(feature = "event-stream")]
            if let Some(waker) = state.on_wait_waker.take() { waker.wake()?; }
            Ok(())
        }
    }

    fn source() -> (TerminalEventSource<Transport>, Transport) {
        let transport = Transport::default();
        transport.0.lock().unwrap().surface = Some((1, 120, 40));
        (TerminalEventSource::with_transport(transport.clone()).unwrap(), transport)
    }

    fn key(character: char) -> Option<InternalEvent> {
        Some(InternalEvent::Event(Event::Key(KeyCode::Char(character).into())))
    }

    #[test]
    fn zero_timeout_still_probes_input_without_waiting() {
        let (mut source, transport) = source();
        transport.0.lock().unwrap().bytes.extend(b"ab");
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(), key('a'));
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(), key('b'));
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(), None);
        assert!(transport.0.lock().unwrap().waits.is_empty());
    }

    #[test]
    fn input_racing_observation_is_read_without_entering_wait() {
        let (mut source, transport) = source();
        transport.0.lock().unwrap().after_observe = Some(b"x".to_vec());
        assert_eq!(source.try_read(None).unwrap(), key('x'));
        assert!(transport.0.lock().unwrap().waits.is_empty());
    }

    #[test]
    fn input_racing_wait_registration_is_not_lost() {
        let (mut source, transport) = source();
        transport.0.lock().unwrap().on_wait = Some(b"x".to_vec());
        assert_eq!(source.try_read(None).unwrap(), key('x'));
        assert_eq!(transport.0.lock().unwrap().waits, vec![(0, u64::MAX)]);
    }

    #[test]
    fn same_size_reentry_discards_decoded_old_input() {
        let (mut source, transport) = source();
        source.parser.advance(b"old", false);
        transport.0.lock().unwrap().surface = Some((2, 120, 40));
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(),
            Some(InternalEvent::Event(Event::Resize(120, 40))));
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(), None);
    }

    #[test]
    fn geometry_change_retains_input_from_the_same_lease() {
        let (mut source, transport) = source();
        source.parser.advance(b"x", false);
        transport.0.lock().unwrap().surface = Some((1, 121, 40));
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(),
            Some(InternalEvent::Event(Event::Resize(121, 40))));
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(), key('x'));
    }

    #[test]
    fn detached_snapshot_keeps_previous_lease_for_reentry_comparison() {
        let (mut source, transport) = source();
        transport.0.lock().unwrap().surface = None;
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(), None);
        transport.0.lock().unwrap().surface = Some((2, 120, 40));
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(),
            Some(InternalEvent::Event(Event::Resize(120, 40))));
    }

    #[test]
    fn reads_remain_batched_and_discard_clears_decoded_keys() {
        let (mut source, transport) = source();
        transport.0.lock().unwrap().bytes.extend(vec![b'x'; 2000]);
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(), key('x'));
        assert_eq!(transport.0.lock().unwrap().reads, vec![BUFFER_SIZE]);
        assert_eq!(source.discard_buffered_input(), InputDiscardStatus::Complete);
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(), key('x'));
        assert_eq!(transport.0.lock().unwrap().reads, vec![BUFFER_SIZE, 976]);
    }

    #[test]
    fn finite_waits_round_up_without_losing_submillisecond_deadlines() {
        assert_eq!(timeout_ms(None), u64::MAX);
        assert_eq!(timeout_ms(Some(Duration::ZERO)), 0);
        assert_eq!(timeout_ms(Some(Duration::from_nanos(1))), 1);
        assert_eq!(timeout_ms(Some(Duration::from_micros(1001))), 2);
    }

    #[cfg(feature = "event-stream")]
    #[test]
    fn event_stream_wake_before_poll_is_consumed_once() {
        let (mut source, transport) = source();
        source.waker().wake().unwrap();
        assert_eq!(source.try_read(None).unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(transport.0.lock().unwrap().reads.is_empty());
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(), None);
    }

    #[cfg(feature = "event-stream")]
    #[test]
    fn event_stream_wake_at_wait_registration_interrupts_the_source() {
        let (mut source, transport) = source();
        transport.0.lock().unwrap().on_wait_waker = Some(source.waker());
        assert_eq!(source.try_read(None).unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert_eq!(transport.0.lock().unwrap().waits.len(), 1);
    }

    #[cfg(feature = "bracketed-paste")]
    #[test]
    fn discarded_paste_is_quarantined_until_its_end_marker() {
        let (mut source, transport) = source();
        source.parser.advance(b"\x1b[200~unfinished", true);
        assert_eq!(source.discard_buffered_input(), InputDiscardStatus::BracketedPasteInProgress);
        transport.0.lock().unwrap().bytes.extend(b"ignored\r\x1b[201~n");
        assert_eq!(source.try_read(Some(Duration::ZERO)).unwrap(), key('n'));
        assert_eq!(source.discard_buffered_input(), InputDiscardStatus::Complete);
    }
}
