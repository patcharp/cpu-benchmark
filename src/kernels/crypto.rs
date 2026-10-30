//! Crypto throughput via the RustCrypto crates. They pick hardware
//! instructions at runtime (SHA-NI, AES-NI, ARMv8 crypto extensions) and fall
//! back to software otherwise. `backend()` reports which path really runs on
//! this machine, so a software fallback shows up in the report instead of
//! being passed off as a hardware number.

use super::{CAT_CRYPTO, Kernel, Metric, Worker, always_ok};
use crate::stats::Rng;
use aes::Aes256;
use aes::cipher::{Array, Block, BlockCipherEncrypt, KeyInit};
use sha2::{Digest, Sha256, Sha512};
use std::hint::black_box;

const BUF: usize = 256 * 1024;
const REPEAT: usize = 64; // 16 MiB hashed / encrypted per run

pub fn kernels() -> Vec<Kernel> {
    vec![
        Kernel {
            id: "crypto.sha256",
            category: CAT_CRYPTO,
            desc: "SHA-256 over a 16 MiB stream (256 KiB chunks)",
            unit: "MB/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(Hash::<Sha256>::new()),
            check: always_ok,
            backend: sha256_backend,
        },
        Kernel {
            id: "crypto.sha512",
            category: CAT_CRYPTO,
            desc: "SHA-512 over a 16 MiB stream (256 KiB chunks)",
            unit: "MB/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(Hash::<Sha512>::new()),
            check: always_ok,
            backend: sha512_backend,
        },
        Kernel {
            id: "crypto.aes256",
            category: CAT_CRYPTO,
            desc: "AES-256 block encryption, 16 MiB per run (parallel-block API)",
            unit: "MB/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(AesEnc::new()),
            check: always_ok,
            backend: aes_backend,
        },
    ]
}

fn label(hw: bool, hw_name: &str, sw_name: &str) -> Option<String> {
    Some(if hw { format!("hardware ({hw_name})") } else { format!("software fallback ({sw_name})") })
}

pub fn sha256_backend() -> Option<String> {
    #[cfg(target_arch = "x86_64")]
    {
        let hw = std::arch::is_x86_feature_detected!("sha") && std::arch::is_x86_feature_detected!("sse4.1");
        return label(hw, "SHA-NI", "CPU has no SHA-NI");
    }
    #[cfg(target_arch = "aarch64")]
    {
        return label(std::arch::is_aarch64_feature_detected!("sha2"), "ARMv8 SHA2", "CPU has no SHA2 extension");
    }
    #[allow(unreachable_code)]
    Some("software (portable)".into())
}

pub fn sha512_backend() -> Option<String> {
    #[cfg(target_arch = "x86_64")]
    {
        // x86 has no SHA-512 instructions in this crate's reach; it uses AVX2.
        return Some(if std::arch::is_x86_feature_detected!("avx2") {
            "SIMD (AVX2 software implementation)".into()
        } else {
            "software fallback (CPU has no AVX2)".into()
        });
    }
    #[cfg(target_arch = "aarch64")]
    {
        return label(
            std::arch::is_aarch64_feature_detected!("sha3"),
            "ARMv8 SHA512",
            "CPU has no SHA512/SHA3 extension",
        );
    }
    #[allow(unreachable_code)]
    Some("software (portable)".into())
}

pub fn aes_backend() -> Option<String> {
    #[cfg(target_arch = "x86_64")]
    {
        return label(std::arch::is_x86_feature_detected!("aes"), "AES-NI", "CPU has no AES-NI");
    }
    #[cfg(target_arch = "aarch64")]
    {
        return label(std::arch::is_aarch64_feature_detected!("aes"), "ARMv8 AES", "CPU has no AES extension");
    }
    #[allow(unreachable_code)]
    Some("software (portable)".into())
}

fn random_bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    (0..n).map(|_| rng.next_u64() as u8).collect()
}

struct Hash<D: Digest> {
    buf: Vec<u8>,
    _d: std::marker::PhantomData<fn() -> D>,
}

impl<D: Digest> Hash<D> {
    fn new() -> Self {
        Hash { buf: random_bytes(BUF, 0xA5A5_0001), _d: std::marker::PhantomData }
    }
}

impl<D: Digest + Send> Worker for Hash<D> {
    fn run(&mut self) -> u64 {
        let mut h = D::new();
        for _ in 0..REPEAT {
            h.update(black_box(&self.buf));
        }
        let out = h.finalize();
        u64::from_le_bytes(out[..8].try_into().unwrap())
    }
    fn work(&self) -> f64 {
        (BUF * REPEAT) as f64
    }
}

struct AesEnc {
    cipher: Aes256,
    blocks: Vec<Block<Aes256>>,
}

impl AesEnc {
    fn new() -> Self {
        let key: [u8; 32] = core::array::from_fn(|i| (i * 7 + 1) as u8);
        let data = random_bytes(BUF, 0xA5A5_0002);
        let blocks = data
            .chunks_exact(16)
            .map(|c| Array::from(<[u8; 16]>::try_from(c).unwrap()))
            .collect();
        AesEnc { cipher: Aes256::new(&Array::from(key)), blocks }
    }
}

impl Worker for AesEnc {
    fn run(&mut self) -> u64 {
        for _ in 0..REPEAT {
            self.cipher.encrypt_blocks(black_box(&mut self.blocks));
        }
        let b = &self.blocks[0];
        u64::from_le_bytes(b[..8].try_into().unwrap())
    }
    fn work(&self) -> f64 {
        (BUF * REPEAT) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha_known_vectors() {
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(hex(&Sha512::digest(b"abc")).starts_with("ddaf35a193617aba"));
    }

    #[test]
    fn aes256_fips197_vector() {
        // FIPS-197 appendix C.3.
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let pt: [u8; 16] = core::array::from_fn(|i| (i * 0x11) as u8);
        let mut block = Array::from(pt);
        Aes256::new(&Array::from(key)).encrypt_block(&mut block);
        assert_eq!(hex(&block), "8ea2b7ca516745bfeafc49904b496089");
    }

    #[test]
    fn backends_are_reported() {
        for b in [sha256_backend(), sha512_backend(), aes_backend()] {
            assert!(b.is_some_and(|s| !s.is_empty()));
        }
    }

    #[test]
    fn workers_are_deterministic() {
        assert_eq!(Hash::<Sha256>::new().run(), Hash::<Sha256>::new().run());
        assert_eq!(AesEnc::new().run(), AesEnc::new().run());
    }
}
