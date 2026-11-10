// src/bin/dusum/inodedup.rs
//
// Bounded-memory hardlink dedup for dusum.
//
// The previous implementation kept a HashSet<Vec<u8>> of "dev-ino" strings,
// which costs a heap allocation plus ~24 bytes of Vec header per file and
// grows without limit — around 60-80 bytes per entry, enough to OOM a 30 GB
// machine at the billion-file mark. This table instead stores only a 64-bit
// fingerprint of each inode string in one flat pre-sized Vec, allocated from
// the line count dusum already knows, so memory is reserved up front and the
// common case never rehashes:
//
//   1B files -> ~1.25B slots x 8 bytes ~= 10 GB, flat, no per-entry allocs
//
// Two accepted trade-offs at this scale:
// - 64-bit fingerprints can collide (~3% chance of any collision over 1B
//   keys); a collision counts one file's disk_size as linked_size once.
// - Linear probing at <= 8/9 load keeps the probe loop cache-friendly.

/// FxHash-style fingerprint over the raw inode bytes ("dev-ino").
/// Zero is reserved as the empty-slot sentinel and remapped to one.
fn fingerprint(bytes: &[u8]) -> u64 {
    const SEED: u64 = 0x517cc1b727220a95;
    let mut h = SEED;
    let (chunks, remainder) = bytes.as_chunks::<8>();
    for chunk in chunks {
        let word = u64::from_le_bytes(*chunk);
        h = (h.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
    for byte in remainder {
        h = (h.rotate_left(5) ^ u64::from(*byte)).wrapping_mul(SEED);
    }
    if h == 0 {
        1
    } else {
        h
    }
}

pub struct InodeDedup {
    /// One slot per entry: 0 = empty, otherwise a fingerprint.
    slots: Vec<u64>,
    used: usize,
}

impl InodeDedup {
    /// Pre-size for `expected` insertions plus 25% headroom so the table
    /// stays below 8/9 load and never grows when the input matches the
    /// expectation (dusum passes the counted data lines).
    pub fn with_expected(expected: usize) -> Self {
        let capacity = expected
            .saturating_add(expected / 4)
            .saturating_add(64)
            .max(64);
        Self {
            slots: vec![0u64; capacity],
            used: 0,
        }
    }

    /// Record `bytes` and report whether this is its first sighting.
    /// Mirrors `HashSet::insert` return semantics.
    pub fn insert_seen(&mut self, bytes: &[u8]) -> bool {
        let fp = fingerprint(bytes);
        let len = self.slots.len();
        let mut i = (fp as usize) % len;
        loop {
            match self.slots[i] {
                0 => {
                    self.slots[i] = fp;
                    self.used += 1;
                    if self.used * 9 >= len * 8 {
                        // Only reachable when the input exceeded the
                        // pre-sized expectation; keep correctness anyway.
                        self.grow();
                    }
                    return true;
                }
                v if v == fp => return false,
                _ => i = (i + 1) % len,
            }
        }
    }

    /// Reserved bytes of the slot table, for startup diagnostics.
    pub fn footprint(&self) -> usize {
        self.slots.len() * 8
    }

    /// Double the table and re-place the existing fingerprints (unchanged,
    /// so no rehashing of source bytes is needed).
    fn grow(&mut self) {
        let new_len = self.slots.len().saturating_mul(2);
        let mut new_slots = vec![0u64; new_len];
        for &fp in &self.slots {
            if fp == 0 {
                continue;
            }
            let mut i = (fp as usize) % new_len;
            while new_slots[i] != 0 {
                i = (i + 1) % new_len;
            }
            new_slots[i] = fp;
        }
        self.slots = new_slots;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sighting_true_repeat_false() {
        let mut t = InodeDedup::with_expected(16);
        assert!(t.insert_seen(b"2051-987654321"));
        assert!(!t.insert_seen(b"2051-987654321"));
        assert!(t.insert_seen(b"2051-987654322"));
        assert!(!t.insert_seen(b"2051-987654322"));
    }

    #[test]
    fn distinct_inodes_stay_distinct() {
        let mut t = InodeDedup::with_expected(1024);
        for i in 0..1000u32 {
            let key = format!("{}-{}", i, i * 7919);
            assert!(t.insert_seen(key.as_bytes()), "first insert of {key}");
        }
        for i in 0..1000u32 {
            let key = format!("{}-{}", i, i * 7919);
            assert!(!t.insert_seen(key.as_bytes()), "repeat insert of {key}");
        }
    }

    #[test]
    fn growth_preserves_membership() {
        // Tiny pre-size forces several grow() cycles.
        let mut t = InodeDedup::with_expected(0);
        for i in 0..500u32 {
            assert!(t.insert_seen(format!("{i}-{}", i * 104729).as_bytes()));
        }
        for i in 0..500u32 {
            assert!(!t.insert_seen(format!("{i}-{}", i * 104729).as_bytes()));
        }
    }

    #[test]
    fn fingerprint_never_zero() {
        // A zero fingerprint would be read as an empty slot.
        let inputs: [&[u8]; 5] = [b"", b"0", b"0-0", b"1-1", b"123456789012345678"];
        for input in inputs {
            assert_ne!(fingerprint(input), 0);
        }
    }

    #[test]
    fn fingerprint_spreads_values() {
        // Sequential inode strings must not land on the same fingerprint or
        // the probe chains would degenerate.
        let a = fingerprint(b"2051-1000000001");
        let b = fingerprint(b"2051-1000000002");
        let c = fingerprint(b"2051-1000000003");
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn footprint_matches_slots() {
        let t = InodeDedup::with_expected(1000);
        // 1000 + 250 + 64 = 1314 slots -> 10512 bytes
        assert_eq!(t.footprint(), 1314 * 8);
    }
}
