//! WebSocket bridge between the Cricket app and the browser extension.
//!
//! The only network listener Cricket has (architecture rule 8): bound to
//! 127.0.0.1, refuses connections from web pages, and requires the pairing
//! token before it accepts anything else.

pub mod protocol;
mod server;

pub use server::{Bridge, BrowserInfo, BrowserSource, PeerResolver};

/// The bridge only ever listens on loopback.
pub const BIND_HOST: &str = "127.0.0.1";

/// Port the extension looks for by default.
pub const DEFAULT_PORT: u16 = 47835;

/// A new random pairing token: 32 hex characters.
pub fn generate_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_to_loopback_only() {
        assert_eq!(BIND_HOST, "127.0.0.1");
    }

    #[test]
    fn tokens_are_32_hex_characters_and_differ() {
        let first = generate_token().unwrap();
        let second = generate_token().unwrap();

        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }
}
