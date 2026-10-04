//! WebSocket bridge between the Cricket app and the Chrome extension.
//!
//! Placeholder for Phase 0. The server, token pairing and message types
//! arrive in Phase 4.

/// The bridge only ever listens on loopback (architecture rule 8).
pub const BIND_HOST: &str = "127.0.0.1";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_to_loopback_only() {
        assert_eq!(BIND_HOST, "127.0.0.1");
        assert!(cricket_core::describe().starts_with("cricket-core"));
    }
}
