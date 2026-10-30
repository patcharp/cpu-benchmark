//! DEFLATE compression / decompression of a deterministic text-like corpus.
//! Pure-Rust backend (miniz_oxide) so the same code runs on every architecture.

use super::{CAT_COMP, Kernel, Metric, Worker, always_ok, no_backend};
use crate::stats::Rng;
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use std::io::{Read, Write};

const CORPUS: usize = 1 << 20;

pub fn kernels() -> Vec<Kernel> {
    vec![
        Kernel {
            id: "comp.deflate",
            category: CAT_COMP,
            desc: "DEFLATE level 6 compression of a 1 MiB text-like corpus",
            unit: "MB/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(Deflate::new()),
            check: always_ok,
            backend: no_backend,
        },
        Kernel {
            id: "comp.inflate",
            category: CAT_COMP,
            desc: "DEFLATE decompression back to the 1 MiB corpus",
            unit: "MB/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(Inflate::new()),
            check: always_ok,
            backend: no_backend,
        },
    ]
}

/// Zipf-ish word soup: realistic redundancy (about 3:1 with DEFLATE).
pub fn corpus(len: usize) -> Vec<u8> {
    let mut rng = Rng::new(0xC0DE_0003);
    let vocab: Vec<Vec<u8>> = (0..4000)
        .map(|_| {
            let l = 2 + rng.below(9) as usize;
            (0..l).map(|_| b'a' + rng.below(26) as u8).collect()
        })
        .collect();
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        // Squaring a uniform draw skews picks towards the first words.
        let r = rng.below(1 << 16) as f64 / 65536.0;
        let w = &vocab[(r * r * vocab.len() as f64) as usize];
        out.extend_from_slice(w);
        out.push(match rng.below(24) {
            0 => b'\n',
            1 => b',',
            _ => b' ',
        });
    }
    out.truncate(len);
    out
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut enc = DeflateEncoder::new(Vec::with_capacity(data.len() / 2), Compression::new(6));
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

struct Deflate {
    data: Vec<u8>,
}

impl Deflate {
    fn new() -> Self {
        Deflate { data: corpus(CORPUS) }
    }
}

impl Worker for Deflate {
    fn run(&mut self) -> u64 {
        let out = deflate(std::hint::black_box(&self.data));
        out.len() as u64
    }
    fn work(&self) -> f64 {
        self.data.len() as f64
    }
}

struct Inflate {
    packed: Vec<u8>,
    out: Vec<u8>,
    raw_len: usize,
}

impl Inflate {
    fn new() -> Self {
        let data = corpus(CORPUS);
        Inflate { packed: deflate(&data), out: Vec::with_capacity(data.len()), raw_len: data.len() }
    }
}

impl Worker for Inflate {
    fn run(&mut self) -> u64 {
        self.out.clear();
        DeflateDecoder::new(std::hint::black_box(&self.packed[..]))
            .read_to_end(&mut self.out)
            .unwrap();
        let n = self.out.len();
        (n as u64) << 8 ^ self.out[n / 2] as u64
    }
    fn work(&self) -> f64 {
        self.raw_len as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_ratio() {
        let data = corpus(200_000);
        assert_eq!(data.len(), 200_000);
        let packed = deflate(&data);
        let ratio = data.len() as f64 / packed.len() as f64;
        assert!((1.5..8.0).contains(&ratio), "unrealistic corpus ratio {ratio}");
        let mut back = Vec::new();
        DeflateDecoder::new(&packed[..]).read_to_end(&mut back).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn corpus_is_deterministic() {
        assert_eq!(corpus(10_000), corpus(10_000));
    }
}
