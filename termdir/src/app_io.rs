//! Application errors retain owned UI details; transport errors keep their native type.
use alloc::string::{String, ToString};
use core::fmt;
pub use crossterm::io::ErrorKind;
pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Transport(crossterm::io::Error),
    Message { kind: ErrorKind, detail: String },
}
impl Error {
    pub fn new(kind: ErrorKind, detail: impl fmt::Display) -> Self {
        Self::Message {
            kind,
            detail: detail.to_string(),
        }
    }
    pub fn other(detail: impl fmt::Display) -> Self {
        Self::new(ErrorKind::Other, detail)
    }
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Transport(error) => error.kind(),
            Self::Message { kind, .. } => *kind,
        }
    }
}
impl From<crossterm::io::Error> for Error {
    fn from(error: crossterm::io::Error) -> Self {
        Self::Transport(error)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(f),
            Self::Message { detail, .. } => f.write_str(detail),
        }
    }
}
impl core::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_operation_detail_and_kind_survive_native_boundary() {
        let operation = String::from("list_dir");
        let error = Error::new(
            ErrorKind::PermissionDenied,
            format!("{operation} failed ({})", -13),
        );
        drop(operation);
        assert_eq!(error.kind(), ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "list_dir failed (-13)");
    }
    #[test]
    fn transport_error_keeps_its_kind_and_message() {
        let error: Error =
            crossterm::io::Error::new(ErrorKind::WriteZero, "attached stream unavailable").into();
        assert_eq!(error.kind(), ErrorKind::WriteZero);
        assert_eq!(error.to_string(), "attached stream unavailable");
    }
}
