//! Unpadded Base64URL (RFC 4648 §5) -- mirrors
//! `service/src/evidencegraph_service/b64url.py` exactly. The one text
//! encoding the private-channel/challenge wire protocol uses for
//! secret-bearing byte fields (startup secret, nonce, challenge response,
//! session token).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;

pub fn encode(raw: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(raw)
}

pub fn decode(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    URL_SAFE_NO_PAD.decode(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let raw: Vec<u8> = (0u8..=255).collect();
        assert_eq!(decode(&encode(&raw)).unwrap(), raw);
    }

    #[test]
    fn never_contains_padding() {
        for len in 1..40 {
            let raw = vec![0xABu8; len];
            assert!(!encode(&raw).contains('='));
        }
    }

    #[test]
    fn matches_a_known_python_encoded_value() {
        // `python -c "import base64; print(base64.urlsafe_b64encode(bytes(range(4))).decode())"`
        // -> "AAECAw==" -> stripped of padding: "AAECAw"
        assert_eq!(encode(&[0, 1, 2, 3]), "AAECAw");
    }
}
