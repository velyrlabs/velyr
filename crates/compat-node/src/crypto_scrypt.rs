//! scrypt (RFC 7914) on top of aws-lc-rs PBKDF2-HMAC-SHA256. Parameters are validated, with
//! checked arithmetic, before any buffer is allocated.

use crate::crypto_hash::pbkdf2;

/// Node's default `maxmem`.
pub const DEFAULT_MAXMEM: u64 = 32 * 1024 * 1024;

pub fn scrypt(
    password: &[u8],
    salt: &[u8],
    n: u64,
    r: u64,
    p: u64,
    maxmem: u64,
    length: usize,
) -> Result<Vec<u8>, String> {
    let invalid = || "Invalid scrypt params".to_string();
    if n < 2 || !n.is_power_of_two() || r == 0 || p == 0 || length > 1 << 30 {
        return Err(invalid());
    }
    // RFC 7914: r * p < 2^30, and N < 2^(16 * r).
    if r.checked_mul(p).is_none_or(|rp| rp >= 1 << 30) || (r < 4 && n >= 1 << (16 * r)) {
        return Err(invalid());
    }
    // V needs 128 * r * N bytes and B another 128 * r * p; Node adds a little slack too.
    let memory = 128u64
        .checked_mul(r)
        .and_then(|row| row.checked_mul(n.checked_add(p)?.checked_add(2)?))
        .ok_or_else(invalid)?;
    if memory > maxmem {
        return Err("Invalid scrypt params: memory limit exceeded".into());
    }
    let (n, r, p) = (n as usize, r as usize, p as usize);
    let row = 128 * r;
    let mut blocks = pbkdf2("sha256", password, salt, 1, p * row)?;
    let mut scratch = vec![0u32; 32 * r * n];
    for block in blocks.chunks_exact_mut(row) {
        ro_mix(block, &mut scratch, n, r);
    }
    pbkdf2("sha256", password, &blocks, 1, length)
}

fn ro_mix(block: &mut [u8], scratch: &mut [u32], n: usize, r: usize) {
    let words = 32 * r;
    let mut x: Vec<u32> = block
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect();
    let mut y = vec![0u32; words];
    for i in 0..n {
        scratch[i * words..(i + 1) * words].copy_from_slice(&x);
        block_mix(&mut x, &mut y, r);
    }
    for _ in 0..n {
        // Integerify: the low word of the last 64-byte block; N is a power of two.
        let j = (x[(2 * r - 1) * 16] as usize) & (n - 1);
        for (word, saved) in x.iter_mut().zip(&scratch[j * words..(j + 1) * words]) {
            *word ^= saved;
        }
        block_mix(&mut x, &mut y, r);
    }
    for (chunk, word) in block.chunks_exact_mut(4).zip(&x) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
}

fn block_mix(x: &mut [u32], y: &mut [u32], r: usize) {
    let mut tail = [0u32; 16];
    tail.copy_from_slice(&x[(2 * r - 1) * 16..2 * r * 16]);
    for i in 0..2 * r {
        for (t, word) in tail.iter_mut().zip(&x[i * 16..(i + 1) * 16]) {
            *t ^= word;
        }
        salsa20_8(&mut tail);
        // Even blocks first, then odd ones.
        let at = (i / 2 + (i % 2) * r) * 16;
        y[at..at + 16].copy_from_slice(&tail);
    }
    x.copy_from_slice(y);
}

fn salsa20_8(block: &mut [u32; 16]) {
    let mut x = *block;
    for _ in 0..4 {
        for (a, b, c, d) in [
            (4, 0, 12, 7),
            (8, 4, 0, 9),
            (12, 8, 4, 13),
            (0, 12, 8, 18),
            (9, 5, 1, 7),
            (13, 9, 5, 9),
            (1, 13, 9, 13),
            (5, 1, 13, 18),
            (14, 10, 6, 7),
            (2, 14, 10, 9),
            (6, 2, 14, 13),
            (10, 6, 2, 18),
            (3, 15, 11, 7),
            (7, 3, 15, 9),
            (11, 7, 3, 13),
            (15, 11, 7, 18),
            (1, 0, 3, 7),
            (2, 1, 0, 9),
            (3, 2, 1, 13),
            (0, 3, 2, 18),
            (6, 5, 4, 7),
            (7, 6, 5, 9),
            (4, 7, 6, 13),
            (5, 4, 7, 18),
            (11, 10, 9, 7),
            (8, 11, 10, 9),
            (9, 8, 11, 13),
            (10, 9, 8, 18),
            (12, 15, 14, 7),
            (13, 12, 15, 9),
            (14, 13, 12, 13),
            (15, 14, 13, 18),
        ] {
            x[a] ^= x[b].wrapping_add(x[c]).rotate_left(d);
        }
    }
    for (word, mixed) in block.iter_mut().zip(x) {
        *word = word.wrapping_add(mixed);
    }
}
