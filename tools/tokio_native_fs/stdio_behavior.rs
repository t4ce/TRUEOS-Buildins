#[path = "../../../TRUEOS/tools/testpy/native_tokio/boundary.rs"]
mod runtime_boundary;

use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex, OnceLock},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Default)]
struct Console {
    bytes: VecDeque<u8>,
    output: Vec<(u32, Vec<u8>)>,
    generation: u32,
    claims: usize,
    reads: usize,
    inject_after_empty_probe: bool,
    read_error: isize,
    child_status: i32,
    backpressure: usize,
    send_error: isize,
}
static CONSOLE: OnceLock<(Mutex<Console>, Condvar)> = OnceLock::new();
fn console() -> &'static (Mutex<Console>, Condvar) {
    CONSOLE.get_or_init(Default::default)
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_write(stream: u32, p: *const u8, n: usize) {
    console()
        .0
        .lock()
        .unwrap()
        .output
        .push((stream, unsafe { std::slice::from_raw_parts(p, n) }.to_vec()));
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_blueprint_terminal_lease_current_v1(
    _ready: u64,
    out: *mut u64,
) -> i32 {
    console().0.lock().unwrap().claims += 1;
    unsafe { out.write(5) };
    0
}
#[unsafe(no_mangle)]
extern "C" fn trueos_cabi_shell_attached_observe_v1() -> u32 {
    console().0.lock().unwrap().generation
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_shell_attached_read_v1(p: *mut u8, n: usize) -> isize {
    let mut c = console().0.lock().unwrap();
    c.reads += 1;
    if c.read_error != 0 {
        return c.read_error;
    }
    if c.inject_after_empty_probe {
        c.inject_after_empty_probe = false;
        c.bytes.extend(b"arrived");
        c.generation += 1;
        console().1.notify_all();
        return 0;
    }
    let count = n.min(c.bytes.len());
    for i in 0..count {
        unsafe { p.add(i).write(c.bytes.pop_front().unwrap()) }
    }
    count as isize
}
#[unsafe(no_mangle)]
extern "C" fn trueos_cabi_shell_attached_wait_after_v1(observed: u32, _timeout: u64) -> i32 {
    let c = console().0.lock().unwrap();
    if c.generation != observed {
        return 1;
    }
    let (c, result) = console()
        .1
        .wait_timeout_while(c, Duration::from_secs(2), |c| c.generation == observed)
        .unwrap();
    assert!(!result.timed_out(), "native stdin lost its generation wake");
    assert_ne!(c.generation, observed);
    1
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_blueprint_child_read_v1(_h: u64, p: *mut u8, n: usize) -> isize {
    unsafe { trueos_cabi_shell_attached_read_v1(p, n) }
}
#[unsafe(no_mangle)]
extern "C" fn trueos_cabi_blueprint_child_status_v1(_h: u64) -> i32 {
    console().0.lock().unwrap().child_status
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_blueprint_child_send_v1(_h: u64, p: *const u8, n: usize) -> isize {
    let mut c = console().0.lock().unwrap();
    if c.send_error != 0 {
        return c.send_error;
    }
    if c.backpressure > 0 {
        c.backpressure -= 1;
        return -3;
    }
    let count = n.min(3);
    c.output
        .push((7, unsafe { std::slice::from_raw_parts(p, count) }.to_vec()));
    count as isize
}

fn main() {
    let child = std::env::var("TRUEOS_MCP_STDIO").as_deref() == Ok("1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut out = tokio::io::stdout();
        out.write_all(b"stdout").await.unwrap();
        out.flush().await.unwrap();
        let mut err = tokio::io::stderr();
        err.write_all(b"stderr").await.unwrap();
        err.flush().await.unwrap();
        {
            let c = console().0.lock().unwrap();
            assert_eq!(c.claims, 0, "transcript writes must not claim input");
            let stdout: Vec<u8> = c
                .output
                .iter()
                .filter(|(s, _)| *s == if child { 7 } else { 1 })
                .flat_map(|(_, b)| b.iter().copied())
                .collect();
            assert_eq!(stdout, b"stdout");
            assert!(c.output.contains(&(2, b"stderr".to_vec())));
        }
        console().0.lock().unwrap().inject_after_empty_probe = true;
        let mut input = tokio::io::stdin();
        let mut bytes = [0; 7];
        input.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"arrived");
        let empty: &mut [u8] = &mut [];
        assert_eq!(input.read(empty).await.unwrap(), 0);
        if child {
            assert_eq!(console().0.lock().unwrap().claims, 0);
            console().0.lock().unwrap().child_status = 4;
            assert_eq!(input.read(&mut bytes).await.unwrap(), 0);
            console().0.lock().unwrap().backpressure = 2;
            out.write_all(b"backpressure").await.unwrap();
            out.flush().await.unwrap();
            console().0.lock().unwrap().send_error = -1;
            let mut broken = tokio::io::stdout();
            // Async write queues the bytes; flushing observes the worker error.
            let write = broken.write_all(b"broken").await;
            let error = match write {
                Err(e) => e,
                Ok(()) => broken.flush().await.unwrap_err(),
            };
            assert_eq!(error.kind(), tokio::io::ErrorKind::BrokenPipe);
            println!("6 native child stdio boundary cases passed");
        } else {
            assert!(console().0.lock().unwrap().claims > 0);
            console().0.lock().unwrap().read_error = -2;
            assert_eq!(
                input.read(&mut bytes).await.unwrap_err().kind(),
                tokio::io::ErrorKind::NotConnected
            );
            println!("5 native attached stdio boundary cases passed");
        }
    });
}
