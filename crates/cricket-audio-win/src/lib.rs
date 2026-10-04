//! Windows implementation of the Cricket audio traits.
//!
//! Placeholder for Phase 0. WASAPI session enumeration, peak meters, session
//! volume and SMTC control arrive in Phase 1.

/// Name of this backend, for logs and diagnostics.
pub fn backend_name() -> &'static str {
    "windows"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_against_core() {
        assert_eq!(backend_name(), "windows");
        assert!(cricket_core::describe().starts_with("cricket-core"));
    }
}
