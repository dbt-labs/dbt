use uuid::Uuid;

/// Parse a UUID from a string (UUID format) or number.
pub fn parse_invocation_id(s: &str) -> Result<Uuid, String> {
    // First try to parse as a UUID string
    if let Ok(uuid) = Uuid::try_parse(s) {
        return Ok(uuid);
    }

    let err_msg = "Must be a valid UUID string or number (up to 2^128 - 1).".to_string();

    // Try to parse as number and convert to UUID
    s.parse::<u128>().map(Uuid::from_u128).map_err(|_| err_msg)
}

/// Parse a span ID from a string (16-character hex) or number.
pub fn parse_parent_span_id(s: &str) -> Result<u64, String> {
    // First try to parse as hex string (with or without 0x prefix)
    let hex_str = s.strip_prefix("0x").unwrap_or(s);
    if let Ok(span_id) = u64::from_str_radix(hex_str, 16) {
        return Ok(span_id);
    }

    // Try to parse as decimal number
    s.parse::<u64>()
        .map_err(|_| "Must be a valid span ID (16-character hex or 64-bit number).".to_string())
}
