//! Lets a console program finish cleanly on Ctrl+C instead of being killed.
//!
//! Cricket lowers another app's volume while it fades. Dying in the middle
//! of a fade would leave that app quiet, and Windows remembers the level.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cricket_core::audio::BackendResult;
use windows::core::BOOL;
use windows::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT};

use crate::com::os_error;

static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Starts catching Ctrl+C, Ctrl+Break and the console window being closed.
/// After this, poll [`requested`] and exit when it turns true.
pub fn install() -> BackendResult<()> {
    unsafe { SetConsoleCtrlHandler(Some(handler), true) }
        .map_err(|e| os_error("install Ctrl+C handler", &e))
}

pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}

unsafe extern "system" fn handler(ctrl_type: u32) -> BOOL {
    REQUESTED.store(true, Ordering::SeqCst);
    if ctrl_type != CTRL_C_EVENT && ctrl_type != CTRL_BREAK_EVENT {
        // Window closed, logoff or shutdown: Windows ends the process as
        // soon as this handler returns, so give the main loop a moment to
        // put the volume back.
        std::thread::sleep(Duration::from_secs(1));
    }
    BOOL(1)
}
