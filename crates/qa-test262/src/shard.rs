//! `--shard N/M`: a test belongs to the shard its path hashes to, so the split is the same on every
//! machine and balanced however the suite grows (a directory never lands on one runner whole).

use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shard {
    /// Zero-based.
    index: u64,
    count: u64,
}

impl Shard {
    /// FNV-1a over the path relative to the test262 root: `DefaultHasher` may change between
    /// Rust releases, and a moving split would shuffle tests between shards.
    pub fn contains(self, path: &str) -> bool {
        fnv1a(path) % self.count == self.index
    }
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl FromStr for Shard {
    type Err = String;

    /// `N/M` with `1 <= N <= M`.
    fn from_str(text: &str) -> Result<Self, String> {
        let invalid = || format!("`{text}` is not a shard: expected N/M with 1 <= N <= M");
        let (n, m) = text.split_once('/').ok_or_else(invalid)?;
        let (n, m) = (
            n.parse::<u64>().map_err(|_| invalid())?,
            m.parse::<u64>().map_err(|_| invalid())?,
        );
        if n == 0 || n > m {
            return Err(invalid());
        }
        Ok(Self {
            index: n - 1,
            count: m,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_n_of_m_and_rejects_the_rest() {
        assert!("2/4".parse::<Shard>().is_ok());
        for bad in ["0/4", "5/4", "1", "a/b", "1/0", "-1/2", ""] {
            assert!(bad.parse::<Shard>().is_err(), "{bad}");
        }
    }

    #[test]
    fn every_path_is_in_exactly_one_shard() {
        let paths: Vec<String> = (0..2000)
            .map(|n| format!("test/built-ins/Array/case-{n}.js"))
            .collect();
        let shards: Vec<Shard> = (1..=7).map(|n| format!("{n}/7").parse().unwrap()).collect();
        let mut sizes = [0usize; 7];
        for path in &paths {
            let owners: Vec<usize> = (0..7).filter(|&i| shards[i].contains(path)).collect();
            assert_eq!(owners.len(), 1, "{path}");
            sizes[owners[0]] += 1;
        }
        // Balanced: no shard is further than 30% from the mean (2000 / 7).
        assert!(sizes.iter().all(|&s| (200..=372).contains(&s)), "{sizes:?}");
    }

    #[test]
    fn the_hash_is_the_published_fnv1a() {
        // Official FNV-1a 64-bit vectors: changing the hash would reshuffle every CI shard.
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a("foobar"), 0x8594_4171_f739_67e8);
    }
}
