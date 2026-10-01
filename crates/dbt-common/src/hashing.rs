pub fn code_hash(code: &str) -> String {
    // Compute a 16-byte (128-bit) BLAKE3 hash and return as hex string
    let hash = blake3::hash(code.as_bytes());
    // Truncate to 16 bytes and encode as hex
    hex::encode(&hash.as_bytes()[..16])
}
