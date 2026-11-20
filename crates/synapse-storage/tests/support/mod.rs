//! Shared helpers for synapse-storage integration tests.

/// Incompressible xorshift bytes. CF_KV caps are enforced against RocksDB's
/// on-disk live-data estimate, so a compressible fill never reaches them (#34).
/// `seed` must be non-zero: a zero xorshift state yields all-zero bytes.
pub fn synthetic_bytes(seed: u64, length: usize) -> Vec<u8> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}
