//! Shared encoding helpers: Crockford base32 and URL-safe base64 (unpadded).

use data_encoding::BASE32_NOPAD;

/// Crockford base32 alphabet: `0123456789ABCDEFGHJKMNPQRSTVWXYZ`.
const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Encode bytes as Crockford base32 (uppercase).
pub(crate) fn crockford_encode(bytes: &[u8]) -> String {
    let standard = BASE32_NOPAD.encode(bytes);
    standard
        .bytes()
        .map(|b| {
            let idx = match b {
                b'A'..=b'Z' => b - b'A',
                b'2'..=b'7' => 26 + (b - b'2'),
                _ => 0,
            };
            CROCKFORD_ALPHABET[idx as usize] as char
        })
        .collect()
}

/// Decode a Crockford base32 string back to bytes.
pub(crate) fn crockford_decode(s: &str) -> Result<Vec<u8>, String> {
    let standard: String = s
        .to_uppercase()
        .chars()
        .map(|c| {
            let idx = CROCKFORD_ALPHABET
                .iter()
                .position(|&b| b == c as u8)
                .ok_or_else(|| format!("invalid crockford char: {c}"))?;
            let standard_char = if idx < 26 {
                (b'A' + idx as u8) as char
            } else {
                (b'2' + (idx - 26) as u8) as char
            };
            Ok(standard_char)
        })
        .collect::<Result<_, String>>()?;
    BASE32_NOPAD
        .decode(standard.as_bytes())
        .map_err(|e| format!("base32 decode: {e}"))
}

/// URL-safe base64, unpadded.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    data_encoding::BASE64URL_NOPAD.encode(bytes)
}

/// Decode URL-safe base64, unpadded.
pub(crate) fn base64_decode(s: &str) -> Result<Vec<u8>, data_encoding::DecodeError> {
    data_encoding::BASE64URL_NOPAD.decode(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crockford_roundtrip() {
        let data = b"hello crab city!";
        let encoded = crockford_encode(data);
        let decoded = crockford_decode(&encoded).unwrap();
        assert_eq!(data.as_slice(), decoded.as_slice());
    }

    #[test]
    fn base64_roundtrip() {
        let data = b"testing base64 encode/decode";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(data.as_slice(), decoded.as_slice());
    }
}
