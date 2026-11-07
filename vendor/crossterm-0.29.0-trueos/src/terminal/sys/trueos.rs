#[allow(unused_imports)]
use crate::alloc_prelude::*;
use crate::io;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::terminal::WindowSize;

// TRUEOS attached input already delivers terminal bytes. Raw mode is the
// application's terminal ownership declaration, not a termios line discipline.
static RAW_MODE: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_raw_mode_enabled() -> bool { RAW_MODE.load(Ordering::Acquire) }

pub(crate) fn enable_raw_mode() -> io::Result<()> {
    crate::trueos::claim()?;
    RAW_MODE.store(true, Ordering::Release);
    Ok(())
}

pub(crate) fn disable_raw_mode() -> io::Result<()> {
    RAW_MODE.store(false, Ordering::Release);
    Ok(())
}

pub(crate) fn window_size() -> io::Result<WindowSize> {
    let Some((_, columns, rows)) = crate::trueos::surface()? else {
        return Err(io::Error::new(io::ErrorKind::NotConnected, "TRUEOS terminal is not attached"));
    };
    Ok(WindowSize {
        columns: columns.min(u32::from(u16::MAX)) as u16,
        rows: rows.min(u32::from(u16::MAX)) as u16,
        width: 0,
        height: 0,
    })
}

pub(crate) fn size() -> io::Result<(u16, u16)> {
    window_size().map(|size| (size.columns, size.rows))
}

#[cfg(feature = "events")]
pub fn supports_keyboard_enhancement() -> io::Result<bool> { Ok(false) }

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::alloc_prelude::*;
    use super::*;
    use crate::trueos::test_support::{SERIAL, STATE};

    #[test]
    fn raw_mode_requires_a_real_lease_and_geometry_uses_its_surface() {
        let _serial = SERIAL.lock().unwrap();
        disable_raw_mode().unwrap();
        STATE.lock().unwrap().claim_status = -3;
        assert!(enable_raw_mode().is_err());
        assert!(!is_raw_mode_enabled());
        {
            let mut state = STATE.lock().unwrap();
            state.claim_status = 0;
            state.surface = (2, 70000, 42);
        }
        enable_raw_mode().unwrap();
        enable_raw_mode().unwrap();
        assert!(is_raw_mode_enabled());
        assert_eq!(size().unwrap(), (u16::MAX, 42));
        let window = window_size().unwrap();
        assert_eq!((window.width, window.height), (0, 0));
        STATE.lock().unwrap().surface_status = -4;
        assert_eq!(window_size().unwrap_err().kind(), io::ErrorKind::NotConnected);
        disable_raw_mode().unwrap();
        assert!(!is_raw_mode_enabled());
        let mut state = STATE.lock().unwrap();
        state.surface_status = 0; state.surface = (1, 120, 40);
    }
}
