//! Memory kernels: STREAM-style bandwidth and a random pointer-chase latency
//! sweep from L1 out to DRAM. Each worker allocates and first-touches its own
//! buffers on the thread that uses them, so NUMA placement follows the thread.

use super::{CAT_MEM, Kernel, Metric, Params, Worker, always_ok, no_backend};
use crate::stats::Rng;
use std::hint::black_box;

const MIB: u64 = 1 << 20;

pub fn kernels() -> Vec<Kernel> {
    let mut v = vec![
        Kernel {
            id: "mem.copy",
            category: CAT_MEM,
            desc: "STREAM-like copy (memcpy) over buffers far larger than the last-level cache",
            unit: "GB/s",
            scale: 1e-9,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |p| Box::new(Stream::new(p, StreamOp::Copy)),
            check: always_ok,
            backend: no_backend,
        },
        Kernel {
            id: "mem.triad",
            category: CAT_MEM,
            desc: "STREAM triad a[i] = b[i] + s*c[i] over buffers far larger than the last-level cache",
            unit: "GB/s",
            scale: 1e-9,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |p| Box::new(Stream::new(p, StreamOp::Triad)),
            check: always_ok,
            backend: no_backend,
        },
        Kernel {
            id: "mem.memcpy_small",
            category: CAT_MEM,
            desc: "Small-block memcpy (8-512 B, unaligned, 16 KiB L1-resident buffers): isolates libc memcpy call cost",
            unit: "GB/s",
            scale: 1e-9,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(SmallCopy::new()),
            check: always_ok,
            backend: no_backend,
        },
    ];
    // (id, bytes, description, heavy)
    macro_rules! lat {
        ($id:literal, $bytes:expr, $desc:literal, $heavy:expr) => {
            Kernel {
                id: $id,
                category: CAT_MEM,
                desc: $desc,
                unit: "ns",
                scale: 1.0,
                metric: Metric::Latency,
                multi: false,
                heavy: $heavy,
                make: |_| Box::new(Chase::new($bytes)),
                check: |p| need_ram($bytes, p),
                backend: no_backend,
            }
        };
    }
    v.push(lat!("mem.lat.32k", 32 * 1024, "Dependent random load latency, 32 KiB footprint (L1)", false));
    v.push(lat!("mem.lat.256k", 256 * 1024, "Dependent random load latency, 256 KiB footprint (L2)", false));
    v.push(lat!("mem.lat.2m", 2 * MIB, "Dependent random load latency, 2 MiB footprint", false));
    v.push(lat!("mem.lat.16m", 16 * MIB, "Dependent random load latency, 16 MiB footprint (L3)", false));
    v.push(lat!("mem.lat.128m", 128 * MIB, "Dependent random load latency, 128 MiB footprint (DRAM)", false));
    v.push(lat!("mem.lat.512m", 512 * MIB, "Dependent random load latency, 512 MiB footprint (DRAM + TLB)", true));
    v
}

/// Refuse to allocate more than a quarter of physical RAM; say so instead of
/// quietly shrinking the test.
fn need_ram(bytes: u64, p: &Params) -> Result<(), String> {
    if p.mem_total > 0 && bytes * 4 > p.mem_total {
        Err(format!(
            "needs {} MiB but machine has only {} MiB RAM (limit: 25%)",
            bytes / MIB,
            p.mem_total / MIB
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum StreamOp {
    Copy,
    Triad,
}

struct Stream {
    op: StreamOp,
    a: Vec<f64>,
    b: Vec<f64>,
    c: Vec<f64>,
}

impl Stream {
    fn new(p: &Params, op: StreamOp) -> Self {
        let per_thread = (p.mem_budget / p.threads.max(1) as u64).clamp(12 * MIB, 384 * MIB);
        let n = (per_thread / 3 / 8) as usize;
        Stream {
            op,
            a: (0..n).map(|i| i as f64).collect(),
            b: (0..n).map(|i| (i % 1024) as f64).collect(),
            c: (0..n).map(|i| 1.0 + (i % 7) as f64).collect(),
        }
    }
}

impl Worker for Stream {
    fn run(&mut self) -> u64 {
        // Checksums sample fixed indices so they do not depend on the buffer
        // size, which varies with --mem-budget-mb and thread count.
        const PROBE: [usize; 3] = [0, 1023, 4095];
        match self.op {
            StreamOp::Copy => {
                self.c.copy_from_slice(&self.a);
                black_box(&self.c);
                PROBE.iter().fold(0, |h, &i| h.rotate_left(7) ^ self.c[i].to_bits())
            }
            StreamOp::Triad => {
                let s = 3.0;
                for ((a, b), c) in self.a.iter_mut().zip(&self.b).zip(&self.c) {
                    *a = *b + s * *c;
                }
                black_box(&self.a);
                PROBE.iter().fold(0, |h, &i| h.rotate_left(7) ^ self.a[i].to_bits())
            }
        }
    }
    fn work(&self) -> f64 {
        let streams = if self.op == StreamOp::Copy { 2 } else { 3 };
        (streams * self.a.len() * 8) as f64
    }
}

/// Many small copies with runtime sizes and unaligned offsets. The sizes are
/// only known at run time, so the compiler must emit real memcpy calls instead
/// of inlining them, which is exactly the libc code path being measured.
struct SmallCopy {
    src: Vec<u8>,
    dst: Vec<u8>,
    /// (src offset, dst offset, length), precomputed so the loop is just copies.
    plan: Vec<(u32, u32, u32)>,
    rounds: usize,
    bytes_per_round: usize,
}

const SMALL_BUF: usize = 16 * 1024;
const SMALL_SIZES: [u32; 12] = [8, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512];

impl SmallCopy {
    fn new() -> Self {
        let mut rng = Rng::new(0x5CA1_AB1E);
        let src: Vec<u8> = (0..SMALL_BUF).map(|_| rng.next_u64() as u8).collect();
        let plan: Vec<(u32, u32, u32)> = (0..4096)
            .map(|_| {
                let len = SMALL_SIZES[rng.below(SMALL_SIZES.len() as u64) as usize];
                let max = SMALL_BUF as u64 - len as u64;
                (rng.below(max) as u32, rng.below(max) as u32, len)
            })
            .collect();
        let bytes_per_round = plan.iter().map(|p| p.2 as usize).sum();
        SmallCopy { src, dst: vec![0; SMALL_BUF], plan, rounds: 256, bytes_per_round }
    }
}

impl Worker for SmallCopy {
    fn run(&mut self) -> u64 {
        for _ in 0..self.rounds {
            for &(so, d_o, len) in &self.plan {
                let (so, d_o, len) = (so as usize, d_o as usize, len as usize);
                self.dst[d_o..d_o + len].copy_from_slice(&self.src[so..so + len]);
            }
            black_box(&self.dst);
        }
        self.dst.chunks(8).fold(0u64, |h, c| h.rotate_left(3) ^ u64::from_le_bytes(c.try_into().unwrap()))
    }
    fn work(&self) -> f64 {
        (self.bytes_per_round * self.rounds) as f64
    }
}

/// Random cyclic pointer chase with one node per 64-byte cache line.
struct Chase {
    next: Vec<u32>,
    pos: u32,
    steps: usize,
}

const LINE_U32: usize = 16;

impl Chase {
    fn new(bytes: u64) -> Self {
        let nodes = (bytes as usize / 64).max(2);
        // Sattolo's algorithm yields a single cycle covering every node.
        let mut perm: Vec<u32> = (0..nodes as u32).collect();
        let mut rng = Rng::new(0xC0FF_EE00 ^ bytes);
        for i in (1..nodes).rev() {
            let j = rng.below(i as u64) as usize;
            perm.swap(i, j);
        }
        let mut next = vec![0u32; nodes * LINE_U32];
        for i in 0..nodes {
            next[i * LINE_U32] = perm[i] * LINE_U32 as u32;
        }
        let steps = match bytes {
            b if b <= 2 * MIB => 2_000_000,
            b if b <= 16 * MIB => 1_000_000,
            _ => 250_000,
        };
        Chase { next, pos: 0, steps }
    }
}

impl Worker for Chase {
    fn run(&mut self) -> u64 {
        let mut p = self.pos as usize;
        for _ in 0..self.steps {
            p = self.next[p] as usize;
        }
        self.pos = p as u32;
        black_box(p) as u64
    }
    fn work(&self) -> f64 {
        self.steps as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chase_is_one_full_cycle() {
        let c = Chase::new(64 * 1000);
        let nodes = c.next.len() / LINE_U32;
        let (mut p, mut seen) = (0usize, 0usize);
        loop {
            p = c.next[p] as usize;
            seen += 1;
            if p == 0 || seen > nodes {
                break;
            }
        }
        assert_eq!(seen, nodes, "permutation must visit every node exactly once");
    }

    #[test]
    fn stream_results() {
        let p = Params { threads: 1, mem_budget: 24 * MIB, mem_total: 0 };
        let mut s = Stream::new(&p, StreamOp::Triad);
        s.run();
        assert_eq!(s.a[10], s.b[10] + 3.0 * s.c[10]);
        let mut s = Stream::new(&p, StreamOp::Copy);
        s.run();
        assert_eq!(s.c[100], 100.0);
        assert_eq!(s.work(), (2 * s.a.len() * 8) as f64);
    }

    #[test]
    fn stream_checksum_is_independent_of_buffer_size() {
        let small = Params { threads: 1, mem_budget: 24 * MIB, mem_total: 0 };
        let large = Params { threads: 1, mem_budget: 96 * MIB, mem_total: 0 };
        for op in [StreamOp::Copy, StreamOp::Triad] {
            let (a, b) = (Stream::new(&small, op).run(), Stream::new(&large, op).run());
            assert_eq!(a, b, "checksum must not depend on --mem-budget-mb");
        }
    }

    #[test]
    fn small_copy_is_deterministic_and_in_bounds() {
        let (mut a, mut b) = (SmallCopy::new(), SmallCopy::new());
        assert_eq!(a.run(), b.run());
        assert!(a.plan.iter().all(|&(s, d, l)| (s + l) as usize <= SMALL_BUF && (d + l) as usize <= SMALL_BUF));
        assert!(a.work() > 1e8);
    }

    #[test]
    fn ram_check_reports_reason() {
        let p = Params { threads: 1, mem_budget: 0, mem_total: 1024 * MIB };
        assert!(need_ram(128 * MIB, &p).is_ok());
        assert!(need_ram(512 * MIB, &p).unwrap_err().contains("25%"));
    }
}
