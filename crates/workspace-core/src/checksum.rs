//! BLAKE3 content integrity digests and constant-time comparison helpers.

use crate::error::CoreError;

/// Compute the lowercase 64-char hex BLAKE3 digest of `raw_bytes`.
pub fn compute_blake3_hex(raw_bytes: &[u8]) -> String {
    blake3::hash(raw_bytes).to_hex().to_string()
}

/// Verify that `expected_hex` is a valid 64-character hex BLAKE3 digest of `raw_bytes`.
pub fn verify_blake3_hex(raw_bytes: &[u8], expected_hex: &str) -> Result<(), CoreError> {
    if expected_hex.len() != 64 || !expected_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CoreError::InvalidMessage {
            field: "checksum",
            reason: "must be a 64-character hexadecimal BLAKE3 digest".to_string(),
        });
    }
    let actual_hex = blake3::hash(raw_bytes).to_hex();
    let expected_lower = expected_hex.to_ascii_lowercase();

    if !constant_time_eq(actual_hex.as_bytes(), expected_lower.as_bytes()) {
        return Err(CoreError::ChecksumMismatch);
    }
    Ok(())
}

/// Constant-time slice equality check.
pub fn constant_time_eq(lhs: &[u8], rhs: &[u8]) -> bool {
    if lhs.len() != rhs.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&a, &b) in lhs.iter().zip(rhs.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}
