//! Integer kernels: sorting, sieve, multiplier-bound mixing, dependent chains.

use super::{CAT_INT, Kernel, Metric, Worker, always_ok, no_backend};
use crate::stats::Rng;
use std::hint::black_box;
use std::sync::OnceLock;

pub fn kernels() -> Vec<Kernel> {
    vec![
        Kernel {
            id: "int.sort",
            category: CAT_INT,
            desc: "Sort 1M random u32 (sort_unstable, 4 MiB working set)",
            unit: "Melem/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(Sort::new(1 << 20)),
            check: always_ok,
            backend: no_backend,
        },
        Kernel {
            id: "int.sieve",
            category: CAT_INT,
            desc: "Sieve of Eratosthenes up to 10M (odd-only, 5 MB array)",
            unit: "Mnum/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(Sieve::new(10_000_000)),
            check: always_ok,
            backend: no_backend,
        },
        Kernel {
            id: "int.mix",
            category: CAT_INT,
            desc: "8 independent splitmix64 streams (64-bit multiply / shift / xor throughput)",
            unit: "Mmix/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(Mix { iters: 1 << 21 }),
            check: always_ok,
            backend: no_backend,
        },
        Kernel {
            id: "int.collatz",
            category: CAT_INT,
            desc: "Collatz steps for 1..=100000 (serial dependent chain, data-dependent branches)",
            unit: "Mstep/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| {
                static STEPS: OnceLock<u64> = OnceLock::new();
                let steps = *STEPS.get_or_init(|| collatz_total(100_000));
                Box::new(Collatz { limit: 100_000, steps })
            },
            check: always_ok,
            backend: no_backend,
        },
    ]
}

struct Sort {
    src: Vec<u32>,
    buf: Vec<u32>,
}

impl Sort {
    fn new(n: usize) -> Self {
        let mut rng = Rng::new(0x5EED_0001);
        let src: Vec<u32> = (0..n).map(|_| rng.next_u64() as u32).collect();
        Sort { buf: vec![0; n], src }
    }
}

impl Worker for Sort {
    fn run(&mut self) -> u64 {
        self.buf.copy_from_slice(&self.src);
        self.buf.sort_unstable();
        let n = self.buf.len();
        black_box(&self.buf);
        (self.buf[0] as u64) << 40 ^ (self.buf[n / 2] as u64) << 20 ^ self.buf[n - 1] as u64
    }
    fn work(&self) -> f64 {
        self.src.len() as f64
    }
}

pub(crate) fn sieve_count(flags: &mut Vec<bool>, n: usize) -> u64 {
    // Index i stands for the odd number 2i + 3.
    let cnt = if n >= 3 { (n - 3) / 2 + 1 } else { 0 };
    flags.clear();
    flags.resize(cnt, false);
    let mut i = 0;
    while (2 * i + 3) * (2 * i + 3) <= n {
        if !flags[i] {
            let p = 2 * i + 3;
            let mut j = (p * p - 3) / 2;
            while j < cnt {
                flags[j] = true;
                j += p;
            }
        }
        i += 1;
    }
    let odd_primes = flags.iter().filter(|&&c| !c).count() as u64;
    odd_primes + (n >= 2) as u64
}

struct Sieve {
    n: usize,
    flags: Vec<bool>,
}

impl Sieve {
    fn new(n: usize) -> Self {
        Sieve { n, flags: Vec::with_capacity(n / 2) }
    }
}

impl Worker for Sieve {
    fn run(&mut self) -> u64 {
        black_box(sieve_count(&mut self.flags, black_box(self.n)))
    }
    fn work(&self) -> f64 {
        self.n as f64
    }
}

const LANES: usize = 8;

#[inline(always)]
fn splitmix(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

struct Mix {
    iters: usize,
}

impl Worker for Mix {
    fn run(&mut self) -> u64 {
        let mut st = [0u64; LANES];
        for (i, s) in st.iter_mut().enumerate() {
            *s = i as u64 * 0x1234_5679 + 1;
        }
        let mut acc = [0u64; LANES];
        for _ in 0..self.iters {
            for l in 0..LANES {
                acc[l] ^= splitmix(&mut st[l]);
            }
        }
        black_box(acc.iter().fold(0, |a, &b| a ^ b.rotate_left(7)))
    }
    fn work(&self) -> f64 {
        (self.iters * LANES) as f64
    }
}

pub(crate) fn collatz_total(limit: u64) -> u64 {
    let mut total = 0u64;
    for n in 1..=limit {
        let mut x = n;
        while x != 1 {
            x = if x & 1 == 0 { x >> 1 } else { 3 * x + 1 };
            total += 1;
        }
    }
    total
}

struct Collatz {
    limit: u64,
    /// Known step count, used as the work amount.
    steps: u64,
}

impl Worker for Collatz {
    fn run(&mut self) -> u64 {
        black_box(collatz_total(black_box(self.limit)))
    }
    fn work(&self) -> f64 {
        self.steps as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sieve_matches_known_prime_counts() {
        let mut f = Vec::new();
        assert_eq!(sieve_count(&mut f, 1), 0);
        assert_eq!(sieve_count(&mut f, 2), 1);
        assert_eq!(sieve_count(&mut f, 100), 25);
        assert_eq!(sieve_count(&mut f, 1_000_000), 78_498);
        assert_eq!(sieve_count(&mut f, 10_000_000), 664_579);
    }

    #[test]
    fn collatz_known_values() {
        // steps(1..=10) = 0,1,7,2,5,8,16,3,19,6
        assert_eq!(collatz_total(10), 67);
        assert_eq!(collatz_total(1), 0);
    }

    #[test]
    fn sort_checksum_is_min_mid_max() {
        let mut s = Sort::new(1 << 12);
        s.run();
        assert!(s.buf.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn mix_changes_with_iterations() {
        let a = Mix { iters: 10 }.run();
        let b = Mix { iters: 11 }.run();
        assert_ne!(a, b);
    }
}
