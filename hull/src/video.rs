//! Detached native playback handoff. Dropping Hull never closes the player.
use super::javascript::Runtime;
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
    pub fn poll(&mut self, runtime: &Runtime) {
        if let Some((generation, request)) = runtime.take_video_request() {
            let rc = command(8, 0, request.url.as_bytes());
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
            }
        }
    }
}
