//! Challenge-response pairing (protocol version 2).
//!
//! The pairing token never crosses the connection. The app sends a random
//! nonce, the extension answers with an HMAC of both nonces under the token,
//! and the app answers with a second HMAC under the same token. Each side
//! thereby proves it knows the token without revealing it, so a listener
//! that is not Cricket learns nothing it can replay, and an extension never
//! takes commands from an impostor that grabbed the port first.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Random challenge: 16 bytes as 32 hex characters.
pub fn random_nonce() -> std::io::Result<String> {
    crate::generate_token()
}

/// What the extension sends to show it holds the token.
pub fn client_proof(token: &str, server_nonce: &str, client_nonce: &str) -> String {
    proof("cricket-v2-client", token, server_nonce, client_nonce)
}

/// What the app sends back to show the same.
pub fn server_proof(token: &str, server_nonce: &str, client_nonce: &str) -> String {
    proof("cricket-v2-server", token, server_nonce, client_nonce)
}

/// Checks the extension's proof in constant time.
pub fn verify_client_proof(
    token: &str,
    server_nonce: &str,
    client_nonce: &str,
    given_hex: &str,
) -> bool {
    let Some(given) = decode_hex(given_hex) else {
        return false;
    };
    // An empty token would make the proof computable by anyone.
    if token.is_empty() {
        return false;
    }
    mac("cricket-v2-client", token, server_nonce, client_nonce)
        .verify_slice(&given)
        .is_ok()
}

fn mac(role: &str, token: &str, server_nonce: &str, client_nonce: &str) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(token.as_bytes()).expect("HMAC takes any key length");
    mac.update(format!("{role}:{server_nonce}:{client_nonce}").as_bytes());
    mac
}

fn proof(role: &str, token: &str, server_nonce: &str, client_nonce: &str) -> String {
    let bytes = mac(role, token, server_nonce, client_nonce)
        .finalize()
        .into_bytes();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    // The extension's WebCrypto code (extension/src/background.ts) must
    // produce these same values; change both together.
    #[test]
    fn proofs_match_the_reference_values() {
        assert_eq!(
            client_proof(TOKEN, "aa11", "bb22"),
            "9549fdae02f39986f382eeb4e7d4ca0a54450d610550a44aee4bd975f7692477"
        );
        assert_eq!(
            server_proof(TOKEN, "aa11", "bb22"),
            "23421a391332138ca22ae551548885df829f7401f8a8fdca28e4e3e80af8f6b9"
        );
    }

    #[test]
    fn a_correct_proof_verifies() {
        let proof = client_proof(TOKEN, "n1", "n2");
        assert!(verify_client_proof(TOKEN, "n1", "n2", &proof));
    }

    #[test]
    fn a_proof_for_other_nonces_or_a_other_token_does_not() {
        let proof = client_proof(TOKEN, "n1", "n2");
        assert!(!verify_client_proof(TOKEN, "n1", "n3", &proof));
        assert!(!verify_client_proof(TOKEN, "n9", "n2", &proof));
        assert!(!verify_client_proof("another-token", "n1", "n2", &proof));
    }

    #[test]
    fn the_two_roles_do_not_produce_the_same_proof() {
        assert_ne!(
            client_proof(TOKEN, "n1", "n2"),
            server_proof(TOKEN, "n1", "n2")
        );
        // A server proof cannot be replayed as a client proof.
        assert!(!verify_client_proof(
            TOKEN,
            "n1",
            "n2",
            &server_proof(TOKEN, "n1", "n2")
        ));
    }

    #[test]
    fn garbage_proofs_are_refused() {
        assert!(!verify_client_proof(TOKEN, "n1", "n2", ""));
        assert!(!verify_client_proof(TOKEN, "n1", "n2", "zz"));
        assert!(!verify_client_proof(TOKEN, "n1", "n2", "abc"));
        assert!(!verify_client_proof(TOKEN, "n1", "n2", "é"));
    }

    #[test]
    fn an_empty_token_never_verifies() {
        let proof = client_proof("", "n1", "n2");
        assert!(!verify_client_proof("", "n1", "n2", &proof));
    }
}
