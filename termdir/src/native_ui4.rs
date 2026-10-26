//! Cell geometry retained for the retired native terminal backend.
use super::*;

const CELL_WIDTH: u32 = 10;

const CELL_HEIGHT: u32 = 20;

fn cell_dimensions(zoom: usize) -> (u32, u32) {
    let percent = ZOOM_LEVELS[zoom] as u32;
    (
        (CELL_WIDTH * percent / 100).max(1),
        (CELL_HEIGHT * percent / 100).max(1),
    )
}

pub(super) fn cell_size(width: u32, height: u32, zoom: usize) -> (u16, u16) {
    let (cw, ch) = cell_dimensions(zoom);
    (
        (width / cw).min(u16::MAX as u32) as u16,
        (height / ch).min(u16::MAX as u32) as u16,
    )
}

pub(super) fn run(_config: Config) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "termdir native text renderer was removed; use --backend=terminal",
    ))
}
