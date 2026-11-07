//! Best-effort SVG persistence, polled cooperatively after the live frame exists.
#[cfg(test)]
use alloc::vec;
use alloc::{string::String, boxed::Box, format};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use crate::execution::{Duration, Instant};
use trueos::{async_fs, env, logl};

pub(super) fn app_root() -> String {
    env::var("TRUEOS_APP_FS_ROOT").unwrap_or_else(|_| "apps/hull".into())
}

#[derive(Default)]
pub(super) struct Snapshot {
    pending: Option<Pin<Box<dyn Future<Output = Result<(), i32>>>>>,
    started: Option<Instant>,
}

impl Snapshot {
    /// Creating the future performs no filesystem work. Hull first installs and
    /// publishes the layout, then calls poll once per event-loop iteration.
    pub fn prepare(&mut self, page_url: &str, svg: String, cookies: String) {
        *self = Self::default();
        // A diagnostic snapshot must not become an unbounded second document.
        if svg.len() > 8 * 1024 * 1024 {
            return;
        }
        let Ok(url) = url::Url::parse(page_url) else {
            return;
        };
        let Some(host) = url.host_str() else {
            return;
        };
        let root = format!("{}/{host}", app_root());
        self.pending = Some(Box::pin(async move {
            async_fs::create_dir_all(root.as_bytes()).await?;
            // Checked publication retains any existing record's type/key and
            // reports the durable commit before the snapshot is called saved.
            async_fs::write_file_typed_checked(
                format!("{root}/layout.svg").as_bytes(), svg.as_bytes(),
                async_fs::ContentTypeId::UTF8_TEXT, false, None,
            ).await?;
            async_fs::write_file_typed_checked(
                format!("{root}/storage.json").as_bytes(),
                cookies.as_bytes(),
                async_fs::ContentTypeId::UTF8_TEXT, false, None,
            )
            .await
        }));
    }

    pub fn poll(&mut self) {
        let Some(future) = self.pending.as_mut() else {
            return;
        };
        let started = self.started.get_or_insert_with(Instant::now);
        if started.elapsed() > Duration::from_secs(5) {
            self.pending = None; // Dropping the future cancels its filesystem operation.
            logl::log(
                logl::level::ERROR,
                format_args!("hull: SVG snapshot timed out"),
            );
            return;
        }
        if let Poll::Ready(result) = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            self.pending = None;
            match result {
                Ok(()) => logl::log(logl::level::INFO, format_args!("hull: SVG snapshot saved")),
                Err(code) => logl::log(
                    logl::level::ERROR,
                    format_args!("hull: SVG snapshot failed: {code}"),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_mock::STATE;

    #[test]
    fn saves_cookie_capture_once_beside_svg_and_replaces_it_with_empty_next_navigation() {
        STATE.with(|state| {
            *state.borrow_mut() = Default::default();
            state.borrow_mut().ready = true;
        });
        let mut snapshot = Snapshot::default();
        snapshot.prepare(
            "https://example.com/a",
            "<svg/>".into(),
            r#"[["cookie:a","one"],["localStorage:a","two"]]"#.into(),
        );
        STATE.with(|state| assert!(state.borrow().writes.is_empty()));
        snapshot.poll();
        snapshot.poll();
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(
                state.writes,
                vec![
                    (
                        b"apps/hull/example.com/layout.svg".to_vec(),
                        b"<svg/>".to_vec()
                    ),
                    (
                        b"apps/hull/example.com/storage.json".to_vec(),
                        br#"[["cookie:a","one"],["localStorage:a","two"]]"#.to_vec()
                    )
                ]
            );
        });
        snapshot.prepare(
            "https://example.com/b",
            "<svg>new</svg>".into(),
            "[]".into(),
        );
        snapshot.poll();
        STATE.with(|state| assert_eq!(state.borrow().writes.last().unwrap().1, b"[]"));
    }

    #[test]
    fn preparation_has_no_io_and_each_poll_returns_while_storage_is_pending() {
        STATE.with(|state| *state.borrow_mut() = Default::default());
        let mut snapshot = Snapshot::default();
        snapshot.prepare("https://example.com/", "<svg/>".into(), String::new());
        STATE.with(|state| assert_eq!(state.borrow().calls, 0));
        snapshot.poll();
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.calls, 1);
            assert_eq!(state.polls, 1);
        });
        assert!(snapshot.pending.is_some());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.ready = true;
            state.fail = true;
        });
        snapshot.poll();
        assert!(snapshot.pending.is_none());
    }

    #[test]
    fn repeated_snapshots_retain_record_identity_and_failed_commit_keeps_old_storage() {
        use crate::fs_mock::Record;
        const SVG: &[u8] = b"apps/hull/example.com/layout.svg";
        const STORAGE: &[u8] = b"apps/hull/example.com/storage.json";
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            *state = Default::default();
            state.ready = true;
            state.records = vec![
                Record { path: SVG.to_vec(), bytes: b"<svg>old</svg>".to_vec(),
                    content_type: 1, key: b"svg-key".to_vec(), file_id: 41 },
                Record { path: STORAGE.to_vec(), bytes: b"[[\"cookie:a\",\"old\"]]".to_vec(),
                    content_type: 2, key: b"storage-key".to_vec(), file_id: 42 },
            ];
        });
        let mut snapshot = Snapshot::default();
        snapshot.prepare("https://example.com/a", "<svg>first</svg>".into(), "[]".into());
        snapshot.poll();
        assert!(snapshot.pending.is_none());
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            assert_eq!(state.records[0].bytes, b"<svg>first</svg>");
            assert_eq!(state.records[0].content_type, 1);
            assert_eq!(state.records[0].key, b"svg-key");
            assert_eq!(state.records[0].file_id, 41);
            assert_eq!(state.records[1].bytes, b"[]");
            assert_eq!(state.records[1].key, b"storage-key");
            assert_eq!(state.records[1].file_id, 42);
            state.fail_path = Some(STORAGE.to_vec());
        });
        crate::logl::take_messages();
        snapshot.prepare("https://example.com/b", "<svg>second</svg>".into(), "[1]".into());
        snapshot.poll();
        assert!(snapshot.pending.is_none());
        STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.records[0].bytes, b"<svg>second</svg>");
            assert_eq!(state.records[1].bytes, b"[]");
            assert_eq!(state.records[1].content_type, 2);
            assert_eq!(state.records[1].key, b"storage-key");
            assert_eq!(state.records[1].file_id, 42);
        });
        let messages = crate::logl::take_messages();
        assert!(messages.iter().any(|message| message.contains("snapshot failed: -2")));
        assert!(!messages.iter().any(|message| message.contains("snapshot saved")));
    }

    #[test]
    fn slow_snapshot_is_discarded_and_can_be_replaced_after_navigation() {
        STATE.with(|state| *state.borrow_mut() = Default::default());
        let mut snapshot = Snapshot::default();
        snapshot.prepare("https://example.com/", "<svg/>".into(), String::new());
        snapshot.poll();
        snapshot.started = Some(Instant::now() - Duration::from_secs(6));
        snapshot.poll();
        assert!(snapshot.pending.is_none());
        snapshot.prepare("https://new.example/", "<svg/>".into(), String::new());
        assert!(snapshot.started.is_none());
        STATE.with(|state| state.borrow_mut().ready = true);
        snapshot.poll();
        assert!(snapshot.pending.is_none());
    }
}
