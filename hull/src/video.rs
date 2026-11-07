//! Detached native playback handoff. Dropping Hull never closes the player.
use alloc::{string::String, vec::Vec, format};
use super::javascript::Runtime;
use trueos::logl;
unsafe extern "C" {
    fn trueos_cabi_vmedia_video_command_v1(
        op: u32,
        a: u64,
        b: u64,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> i32;
}
fn command(op: u32, id: u32, input: &[u8]) -> i32 {
    unsafe {
        trueos_cabi_vmedia_video_command_v1(
            op,
            id as u64,
            0,
            input.as_ptr(),
            input.len(),
            core::ptr::null_mut(),
            0,
        )
    }
}
#[derive(Default)]
pub(super) struct Playback {
    pending: Option<(u64, u32, u32)>,
}
impl Playback {
    /// Report transitions even for media discovered without a JavaScript play
    /// promise. A queued request does not mean a first frame was published.
    pub fn poll(&mut self, runtime: &Runtime) -> Vec<String> {
        let mut messages = Vec::new();
        if let Some((generation, request)) = runtime.take_video_request() {
            let mut payload = request.url.as_bytes().to_vec();
            if let Some(referrer) = &request.referrer {
                payload.push(0);
                payload.extend_from_slice(referrer.as_bytes());
            }
            let rc = command(8, 0, &payload);
            let message = format!(
                "hull: video page={generation} request={} node={} {} code={rc} url={} referrer={}",
                request.id,
                request.node,
                if rc > 0 {
                    "queued; waiting for first frame"
                } else {
                    "handoff rejected"
                },
                request.url,
                request.referrer.as_deref().unwrap_or("none"),
            );
            logl::log(
                if rc > 0 {
                    logl::level::INFO
                } else {
                    logl::level::ERROR
                },
                format_args!("{message}"),
            );
            messages.push(message);
            if rc > 0 {
                self.pending = Some((generation, request.id, rc as u32));
            } else {
                self.pending = None;
                runtime.video_status(generation, request.id, rc);
            }
        }
        if let Some((generation, id, ticket)) = self.pending {
            let status = command(9, ticket, &[]);
            if status != 0 {
                self.pending = None;
                runtime.video_status(generation, id, status);
                let message = format!(
                    "hull: video page={generation} request={id} ticket={ticket} {} code={status}",
                    match status {
                        1 => "first frame published",
                        2 => "playback ended",
                        -125 => "superseded",
                        _ => "preparation failed before first frame",
                    },
                );
                logl::log(
                    if status > 0 {
                        logl::level::INFO
                    } else {
                        logl::level::ERROR
                    },
                    format_args!("{message}"),
                );
                messages.push(message);
            }
        }
        messages
    }
}
