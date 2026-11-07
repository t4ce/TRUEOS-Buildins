use core::time::Duration;

/// A calendar timestamp supplied by a caller, measured from the Unix epoch.
///
/// TRUEOSFS does not currently record or set calendar timestamps. This value
/// lets native callers express file-time requests without linking a host clock;
/// those operations remain unsupported and fail with an explicit I/O error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct FileTimestamp(Duration);
impl FileTimestamp {
    /// Construct a timestamp without consulting a clock.
    pub const fn from_unix_duration(value: Duration) -> Self {
        Self(value)
    }
    /// Return the caller-supplied time since the Unix epoch.
    pub const fn as_unix_duration(self) -> Duration {
        self.0
    }
}
