//! Windows implementation of the Cricket audio traits.
//!
//! - [`WinAudioBackend`]: WASAPI sessions grouped by app, peak levels,
//!   microphone use, per-app volume.
//! - [`SmtcController`]: pause and resume through the system media transport
//!   controls (SMTC).
//!
//! Both hold COM objects, so create and use them on one thread.

pub mod matching;

#[cfg(windows)]
mod com;
#[cfg(windows)]
mod peer;
#[cfg(windows)]
mod sessions;
#[cfg(windows)]
pub mod shutdown;
#[cfg(windows)]
mod smtc;

#[cfg(windows)]
pub use peer::client_process;
#[cfg(windows)]
pub use sessions::WinAudioBackend;
#[cfg(windows)]
pub use smtc::{MediaSessionInfo, SmtcController};
