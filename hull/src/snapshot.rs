//! Best-effort SVG persistence, polled cooperatively after the live frame exists.
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};
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
    pub fn prepare(&mut self, page_url: &str, svg: String) {
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
            async_fs::write_file(format!("{root}/layout.svg").as_bytes(), svg.as_bytes()).await
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
    fn preparation_has_no_io_and_each_poll_returns_while_storage_is_pending() {
        STATE.with(|state| *state.borrow_mut() = Default::default());
        let mut snapshot = Snapshot::default();
        snapshot.prepare("https://example.com/", "<svg/>".into());
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
    fn slow_snapshot_is_discarded_and_can_be_replaced_after_navigation() {
        STATE.with(|state| *state.borrow_mut() = Default::default());
        let mut snapshot = Snapshot::default();
        snapshot.prepare("https://example.com/", "<svg/>".into());
        snapshot.poll();
        snapshot.started = Some(Instant::now() - Duration::from_secs(6));
        snapshot.poll();
        assert!(snapshot.pending.is_none());
        snapshot.prepare("https://new.example/", "<svg/>".into());
        assert!(snapshot.started.is_none());
        STATE.with(|state| state.borrow_mut().ready = true);
        snapshot.poll();
        assert!(snapshot.pending.is_none());
    }
}
