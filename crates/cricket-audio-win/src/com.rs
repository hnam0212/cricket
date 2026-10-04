use cricket_core::audio::BackendError;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// Initializes COM on the current thread and balances it on drop.
///
/// Every thread that touches WASAPI or SMTC needs this. Keep it alive longer
/// than any COM object created on the thread (declare it as the last field).
pub(crate) struct ComGuard {
    needs_uninit: bool,
}

impl ComGuard {
    pub(crate) fn new() -> Result<Self, BackendError> {
        // Multithreaded apartment: session notifications are delivered
        // without this thread having to pump a message loop.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_ok() {
            Ok(Self { needs_uninit: true })
        } else if hr == RPC_E_CHANGED_MODE {
            // The thread already runs COM in another mode. COM is usable, we
            // just must not balance an initialization that was not ours.
            Ok(Self {
                needs_uninit: false,
            })
        } else {
            Err(os_error("CoInitializeEx", &hr.into()))
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.needs_uninit {
            unsafe { CoUninitialize() };
        }
    }
}

pub(crate) fn os_error(context: &str, error: &windows::core::Error) -> BackendError {
    BackendError::Os(format!("{context}: {error}"))
}
