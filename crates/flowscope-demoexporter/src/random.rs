// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! A small random generator with a known state.
//!
//! The flows of a second only depend on the time and on the seed, so an
//! exporter restarted in the middle of a day sends the same traffic. The
//! numbers are not the ones of the Go version, which uses its own generator.

/// SplitMix64.
pub struct Random(u64);

impl Random {
    /// A generator for a time and a seed.
    pub fn new(unix: i64, seed: i64) -> Random {
        // FNV-1 of the text "<time> <seed>", like the Go version.
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in format!("{unix} {seed}").bytes() {
            hash = hash.wrapping_mul(0x100000001b3) ^ byte as u64;
        }
        Random(hash)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    /// A number in [0, 1).
    pub fn float(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A number in [0, n).
    pub fn below(&mut self, n: u32) -> u32 {
        (((self.next_u64() >> 32) * n as u64) >> 32) as u32
    }

    /// A number following the normal distribution (mean 0, deviation 1).
    pub fn normal(&mut self) -> f64 {
        // Box-Muller. The first number must not be 0.
        let u = 1.0 - self.float();
        let v = self.float();
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }

    /// A value of a list. The default value when the list is empty.
    pub fn choose<T: Copy + Default>(&mut self, values: &[T]) -> T {
        match values.len() {
            0 => T::default(),
            1 => values[0],
            n => values[self.below(n as u32) as usize],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable() {
        let mut a = Random::new(1_700_000_000, 0);
        let mut b = Random::new(1_700_000_000, 0);
        let mut c = Random::new(1_700_000_000, 100);
        let (x, y, z) = (a.next_u64(), b.next_u64(), c.next_u64());
        assert_eq!(x, y);
        assert_ne!(x, z);
    }

    #[test]
    fn ranges() {
        let mut r = Random::new(1, 2);
        let mut sum = 0.0;
        for _ in 0..10_000 {
            let f = r.float();
            assert!((0.0..1.0).contains(&f));
            assert!(r.below(7) < 7);
            sum += r.normal();
        }
        assert!((sum / 10_000.0).abs() < 0.05, "mean of the normal distribution: {}", sum / 10_000.0);
        assert_eq!(r.choose::<u16>(&[]), 0);
        assert_eq!(r.choose(&[5u16]), 5);
    }
}
