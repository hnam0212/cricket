//! Debug CLI for the Windows audio backend: shows live sessions and peak
//! levels, and pauses, resumes and fades a chosen app.
//!
//! Cricket cannot hear. This tool exists so a human can check that what the
//! backend reports matches what they hear, and paste the log back.

#[cfg(windows)]
mod probe;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    probe::main()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("cricket-probe only runs on Windows");
}
