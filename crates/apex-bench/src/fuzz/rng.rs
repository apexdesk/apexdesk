//! A small seeded random generator (xorshift64*): the fuzzer only needs
//! speed and repeatability, not quality.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// A generator seeded from the clock and a salt (the worker number).
    pub fn from_time(salt: u64) -> Rng {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        Rng::new(t ^ salt.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ std::process::id() as u64)
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number in 0..n (0 when n is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }

    /// True with probability 1/n.
    pub fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// A length biased towards small values (most mutations are local).
    pub fn small_len(&mut self, max: usize) -> usize {
        let max = max.max(1);
        match self.below(4) {
            0 => 1,
            1 => 1 + self.below(8.min(max)),
            2 => 1 + self.below(64.min(max)),
            _ => 1 + self.below(max),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeatable_and_bounded() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..100 {
            assert_eq!(a.next(), b.next());
            assert!(a.below(10) < 10);
            b.below(10);
        }
        assert_eq!(a.below(0), 0);
    }
}
