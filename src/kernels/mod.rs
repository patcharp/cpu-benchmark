//! Benchmark kernels. Every kernel is deterministic (fixed seeds, fixed sizes)
//! so the same checksum must come out on every architecture; a mismatch is
//! reported as a correctness problem rather than ignored.

mod compress;
mod crypto;
mod data;
mod fp;
mod int;
mod mem;

pub const CAT_INT: &str = "Integer";
pub const CAT_FP: &str = "Floating point";
pub const CAT_MEM: &str = "Memory";
pub const CAT_CRYPTO: &str = "Crypto";
pub const CAT_COMP: &str = "Compression";
pub const CAT_DATA: &str = "Server workloads";

pub const CATEGORIES: [&str; 6] = [CAT_INT, CAT_FP, CAT_MEM, CAT_CRYPTO, CAT_COMP, CAT_DATA];

/// Sizing information handed to a kernel when a worker is created.
#[derive(Clone, Debug)]
pub struct Params {
    /// Number of worker threads running this kernel concurrently.
    pub threads: usize,
    /// Total bytes the streaming kernels may touch across all threads.
    pub mem_budget: u64,
    /// Physical memory of the machine (0 if unknown).
    pub mem_total: u64,
}

/// One thread's private instance of a kernel. Created on the thread that will
/// run it so memory is first-touched on the right NUMA node.
pub trait Worker: Send {
    /// Execute one unit of work and return a checksum of its result.
    fn run(&mut self) -> u64;
    /// Amount of work (bytes, flops, operations...) done by one `run`.
    fn work(&self) -> f64;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Metric {
    /// work / second, scaled by `Kernel::scale` (higher is better).
    Rate,
    /// seconds per unit of work, in nanoseconds (lower is better).
    Latency,
}

pub struct Kernel {
    pub id: &'static str,
    pub category: &'static str,
    pub desc: &'static str,
    pub unit: &'static str,
    pub scale: f64,
    pub metric: Metric,
    /// Whether an all-threads run is meaningful for this kernel.
    pub multi: bool,
    /// Needs a lot of RAM / time; skipped by the quick profile.
    pub heavy: bool,
    pub make: fn(&Params) -> Box<dyn Worker>,
    /// `Err(reason)` means this machine cannot run the kernel.
    pub check: fn(&Params) -> Result<(), String>,
    /// Which implementation will really run here (hardware vs software).
    pub backend: fn() -> Option<String>,
}

impl Kernel {
    pub fn higher_is_better(&self) -> bool {
        self.metric == Metric::Rate
    }
}

/// Kernels whose speed is dominated by libc code (malloc/free, memcpy), so the
/// number reflects the libc in use as much as the CPU. Measured on one machine:
/// glibc -> musl cost these 10-46% while pure-compute kernels did not move.
pub const LIBC_SENSITIVE: &[&str] =
    &["mem.copy", "mem.memcpy_small", "comp.inflate", "data.hashmap", "data.json", "data.alloc"];

pub fn libc_sensitive(id: &str) -> bool {
    LIBC_SENSITIVE.contains(&id)
}

pub(crate) fn always_ok(_: &Params) -> Result<(), String> {
    Ok(())
}

pub(crate) fn no_backend() -> Option<String> {
    None
}

pub fn all() -> Vec<Kernel> {
    let mut v = Vec::new();
    v.extend(int::kernels());
    v.extend(fp::kernels());
    v.extend(mem::kernels());
    v.extend(crypto::kernels());
    v.extend(compress::kernels());
    v.extend(data::kernels());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn params() -> Params {
        Params { threads: 1, mem_budget: 64 << 20, mem_total: 0 }
    }

    #[test]
    fn libc_sensitive_ids_exist() {
        let ids: HashSet<_> = all().iter().map(|k| k.id).collect();
        assert!(LIBC_SENSITIVE.iter().all(|i| ids.contains(i)), "stale id in LIBC_SENSITIVE");
        assert!(!libc_sensitive("int.sort"));
    }

    #[test]
    fn ids_are_unique_and_categorised() {
        let ks = all();
        let ids: HashSet<_> = ks.iter().map(|k| k.id).collect();
        assert_eq!(ids.len(), ks.len(), "duplicate kernel id");
        assert!(ks.iter().all(|k| CATEGORIES.contains(&k.category)));
        assert!(ks.len() >= 20);
    }

    /// Every cheap kernel must be deterministic: two fresh workers produce the
    /// same checksum, and a second run of one worker is also valid (no panic).
    #[test]
    fn kernels_are_deterministic() {
        let p = params();
        for k in all().iter().filter(|k| !k.heavy && !k.id.starts_with("mem.lat.")) {
            if (k.check)(&p).is_err() {
                continue;
            }
            let (mut a, mut b) = ((k.make)(&p), (k.make)(&p));
            assert_eq!(a.run(), b.run(), "{} is not deterministic", k.id);
            assert!(a.work() > 0.0, "{} reports no work", k.id);
            a.run();
        }
    }
}
