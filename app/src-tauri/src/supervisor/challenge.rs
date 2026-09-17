//! D-025 domain-separated HMAC-SHA-256 startup identity challenge (S1-04)
//! -- Tauri's (parent) half. Mirrors
//! `service/src/evidencegraph_service/challenge.py` byte-for-byte; that
//! module's docstring is the canonical description of the exact byte
//! construction:
//!
//! ```text
//! message  = DOMAIN_SEPARATOR + 0x00 + endpoint_bytes + 0x00 + nonce_bytes
//! response = HMAC-SHA-256(key=startup_secret_bytes, msg=message)
//! ```
//!
//! Tauri generates the nonce, POSTs it to the child's `/__startup/
//! challenge` route, and -- unlike the Python side, which only computes a
//! response -- is the party that actually *verifies* the response it gets
//! back, in constant time, against its own independently-held copy of the
//! secret and endpoint.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Fixed ASCII domain separator. Must match
/// `evidencegraph_service.challenge.DOMAIN_SEPARATOR` exactly.
pub const DOMAIN_SEPARATOR: &[u8] = b"EvidenceGraph-S1-04-startup-challenge-v1";
const FIELD_SEPARATOR: u8 = 0x00;

/// The exact string both sides must byte-for-byte agree on for a given
/// bound endpoint. Matches `evidencegraph_service.challenge.
/// canonical_endpoint`.
pub fn canonical_endpoint(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

fn message_bytes(endpoint: &str, nonce: &[u8]) -> Vec<u8> {
    let mut message =
        Vec::with_capacity(DOMAIN_SEPARATOR.len() + 1 + endpoint.len() + 1 + nonce.len());
    message.extend_from_slice(DOMAIN_SEPARATOR);
    message.push(FIELD_SEPARATOR);
    message.extend_from_slice(endpoint.as_bytes());
    message.push(FIELD_SEPARATOR);
    message.extend_from_slice(nonce);
    message
}

/// Computes the expected response for a `(secret, endpoint, nonce)`
/// triple. Only this module's own tests call it directly (as a
/// known-good fixture to feed into [`verify`]); non-test code always
/// goes through `verify` instead, which is why this is dead code outside
/// `#[cfg(test)]`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn expected_response(secret: &[u8], endpoint: &str, nonce: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(secret).expect("HMAC-SHA-256 accepts a key of any length");
    mac.update(&message_bytes(endpoint, nonce));
    mac.finalize().into_bytes().to_vec()
}

/// Constant-time verification (`hmac::Mac::verify_slice` is
/// constant-time by construction). Rejects a response of the wrong
/// length exactly as it rejects an incorrect one -- both simply fail to
/// verify, with no distinguishable early return.
pub fn verify(secret: &[u8], endpoint: &str, nonce: &[u8], response: &[u8]) -> bool {
    let mut mac =
        HmacSha256::new_from_slice(secret).expect("HMAC-SHA-256 accepts a key of any length");
    mac.update(&message_bytes(endpoint, nonce));
    mac.verify_slice(response).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
    const HOST: &str = "127.0.0.1";
    const PORT: u16 = 54321;

    #[test]
    fn canonical_endpoint_format() {
        assert_eq!(canonical_endpoint("127.0.0.1", 51823), "127.0.0.1:51823");
    }

    #[test]
    fn verify_accepts_a_correctly_computed_response() {
        let endpoint = canonical_endpoint(HOST, PORT);
        let nonce = b"0123456789abcdef";
        let response = expected_response(SECRET, &endpoint, nonce);
        assert!(verify(SECRET, &endpoint, nonce, &response));
    }

    #[test]
    fn verify_rejects_wrong_secret() {
        let endpoint = canonical_endpoint(HOST, PORT);
        let nonce = b"0123456789abcdef";
        let response = expected_response(SECRET, &endpoint, nonce);
        assert!(!verify(
            b"a-completely-different-secret-value",
            &endpoint,
            nonce,
            &response
        ));
    }

    #[test]
    fn verify_rejects_wrong_endpoint() {
        let endpoint = canonical_endpoint(HOST, PORT);
        let nonce = b"0123456789abcdef";
        let response = expected_response(SECRET, &endpoint, nonce);
        let other_endpoint = canonical_endpoint(HOST, PORT + 1);
        assert!(!verify(SECRET, &other_endpoint, nonce, &response));
    }

    #[test]
    fn verify_rejects_wrong_nonce() {
        let endpoint = canonical_endpoint(HOST, PORT);
        let response = expected_response(SECRET, &endpoint, b"0123456789abcdef");
        assert!(!verify(SECRET, &endpoint, b"fedcba9876543210", &response));
    }

    #[test]
    fn verify_rejects_truncated_response() {
        let endpoint = canonical_endpoint(HOST, PORT);
        let nonce = b"0123456789abcdef";
        let response = expected_response(SECRET, &endpoint, nonce);
        assert!(!verify(
            SECRET,
            &endpoint,
            nonce,
            &response[..response.len() - 1]
        ));
    }

    #[test]
    fn verify_rejects_empty_response() {
        let endpoint = canonical_endpoint(HOST, PORT);
        assert!(!verify(SECRET, &endpoint, b"0123456789abcdef", &[]));
    }

    #[test]
    fn field_separators_prevent_field_splicing_ambiguity() {
        // Without the 0x00 separators, endpoint="127.0.0.1:1" + nonce="23"
        // could collide with endpoint="127.0.0.1:12" + nonce="3". With
        // them, these must never produce the same response.
        let nonce_a = b"23";
        let nonce_b = b"3";
        let response_a = expected_response(SECRET, "127.0.0.1:1", nonce_a);
        let response_b = expected_response(SECRET, "127.0.0.1:12", nonce_b);
        assert_ne!(response_a, response_b);
    }
}
