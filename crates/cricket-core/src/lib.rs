//! Platform-agnostic core of Cricket.
//!
//! This crate must compile and test without any OS audio API. The engine,
//! settings and the `AudioBackend` / `MediaController` traits land here in
//! later phases.

/// Version of the core crate, shown in the UI so we can tell the web side is
/// really talking to the Rust side.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// One-line description of the core, used by the scaffold's smoke command.
pub fn describe() -> String {
    format!("cricket-core v{}", version())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_includes_version() {
        assert_eq!(describe(), format!("cricket-core v{}", version()));
        assert!(!version().is_empty());
    }
}
