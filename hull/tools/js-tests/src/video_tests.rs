use crate::{
    javascript::{Report, Runtime},
    scripts::Scripts,
    video::Playback,
};
use std::{
    cell::RefCell,
    time::{Duration, Instant},
};
#[derive(Default)]
struct Native {
    commands: Vec<(u32, u64, String)>,
    status: i32,
    fail: bool,
}
thread_local! {static NATIVE:RefCell<Native>=RefCell::new(Native::default());}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_vmedia_video_command_v1(
    op: u32,
    id: u64,
    _: u64,
    input: *const u8,
    len: usize,
    _: *mut u8,
    _: usize,
) -> i32 {
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(input, len) }
    };
    NATIVE.with(|native| {
        let mut native = native.borrow_mut();
        native
            .commands
            .push((op, id, String::from_utf8_lossy(bytes).into_owned()));
        if op == 8 {
            if native.fail { -2 } else { 10 }
        } else {
            native.status
        }
    })
}
fn begin(runtime: &mut Runtime, generation: u64, html: &str) {
    let doc = rp5::parse(html);
    let mut scripts = Scripts::default();
    scripts.extract(&doc, "https://fixture.test/path/page");
    runtime.begin_page(
        generation,
        "https://fixture.test/path/page",
        scripts.sources,
        doc,
    );
}
fn drive(runtime: &mut Runtime, playback: &mut Playback, reports: &mut Vec<Report>, turns: usize) {
    for _ in 0..turns {
        reports.extend(runtime.poll());
        playback.poll(runtime);
    }
}
fn initial(runtime: &mut Runtime, playback: &mut Playback, generation: u64) -> Vec<Report> {
    let start = Instant::now();
    let mut reports = Vec::new();
    loop {
        drive(runtime, playback, &mut reports, 1);
        if runtime.take_initial_document(generation).is_some() {
            return reports;
        }
        assert!(start.elapsed() < Duration::from_secs(2), "{reports:?}");
    }
}
fn logs(reports: &[Report]) -> Vec<String> {
    reports
        .iter()
        .filter_map(|r| match r {
            Report::Console { batch, .. } => Some(&batch.messages),
            _ => None,
        })
        .flatten()
        .map(|m| m.text.clone())
        .collect()
}
#[test]
fn page_play_is_pending_until_first_frame_and_hull_drop_sends_no_close() {
    NATIVE.with(|n| *n.borrow_mut() = Native::default());
    let mut runtime = Runtime::start().unwrap();
    let mut playback = Playback::default();
    begin(
        &mut runtime,
        1,
        r#"<video id=v src='../movie.mp4'></video><script>
        const v=document.getElementById('v');if(!(v instanceof HTMLVideoElement)||!(v instanceof HTMLMediaElement)||!v.paused)throw Error('video wrapper');
        v.addEventListener('playing',()=>console.log('playing event'));v.play().then(()=>console.log('play resolved'),e=>console.log(e.name));
    </script>"#,
    );
    let mut reports = initial(&mut runtime, &mut playback, 1);
    assert!(logs(&reports).is_empty());
    NATIVE.with(|n| {
        let mut n = n.borrow_mut();
        assert_eq!(
            n.commands[0],
            (8, 0, "https://fixture.test/movie.mp4".into())
        );
        n.status = 1;
    });
    drive(&mut runtime, &mut playback, &mut reports, 16);
    assert_eq!(logs(&reports), ["playing event", "play resolved"]);
    drop(playback);
    drop(runtime);
    NATIVE.with(|n| {
        assert!(
            n.borrow()
                .commands
                .iter()
                .all(|(op, _, _)| *op == 8 || *op == 9)
        )
    });
}
#[test]
fn multiple_page_elements_coalesce_to_one_latest_request() {
    NATIVE.with(|n| *n.borrow_mut() = Native::default());
    let mut runtime = Runtime::start().unwrap();
    let mut playback = Playback::default();
    begin(
        &mut runtime,
        1,
        r#"<video id=a src='/old.mp4'></video><video id=b><source src='/new.mp4'></video><script>
        document.getElementById('a').play().catch(e=>console.log('old '+e.name));
        document.getElementById('b').play().then(()=>console.log('new playing'));
    </script>"#,
    );
    let mut reports = initial(&mut runtime, &mut playback, 1);
    assert_eq!(logs(&reports), ["old AbortError"]);
    NATIVE.with(|n| {
        let mut n = n.borrow_mut();
        let requests: Vec<_> = n.commands.iter().filter(|(op, _, _)| *op == 8).collect();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].2.ends_with("/new.mp4"));
        n.status = 1;
    });
    drive(&mut runtime, &mut playback, &mut reports, 16);
    assert_eq!(logs(&reports), ["old AbortError", "new playing"]);
}
#[test]
fn failed_native_handoff_rejects_play_and_navigation_ignores_old_receipts() {
    NATIVE.with(|n| {
        *n.borrow_mut() = Native {
            fail: true,
            ..Default::default()
        }
    });
    let mut runtime = Runtime::start().unwrap();
    let mut playback = Playback::default();
    begin(
        &mut runtime,
        1,
        r#"<video src='/movie.mp4'></video><script>document.querySelector('video').play().catch(e=>console.log(e.name))</script>"#,
    );
    let reports = initial(&mut runtime, &mut playback, 1);
    assert_eq!(logs(&reports), ["NotSupportedError"]);
    begin(&mut runtime, 2, "<p>new page</p>");
    runtime.video_status(1, 1, 1);
    let reports = initial(&mut runtime, &mut playback, 2);
    assert!(logs(&reports).is_empty());
}
#[test]
fn invalid_sources_reject_without_native_io_or_autoplay() {
    NATIVE.with(|n| *n.borrow_mut() = Native::default());
    let mut runtime = Runtime::start().unwrap();
    let mut playback = Playback::default();
    begin(
        &mut runtime,
        1,
        r#"<video src='/unused.mp4' autoplay></video><script>
        const v=document.createElement('video');v.play().catch(e=>console.log(e.name));
        v.src='javascript:evil()';v.play().catch(e=>console.log(e.name));
    </script>"#,
    );
    let reports = initial(&mut runtime, &mut playback, 1);
    assert_eq!(logs(&reports), ["NotSupportedError", "NotSupportedError"]);
    NATIVE.with(|n| assert!(n.borrow().commands.is_empty()));
}
