pub use trueos::clock::{Duration, Instant as SystemTime};

#[inline]
pub fn elapsed_since(when: SystemTime) -> Duration {
    SystemTime::now().saturating_duration_since(when).into()
}
