//! Floating-point kernels. Multiply and add are deliberately kept separate
//! (no `mul_add`): without a hardware-FMA target feature `mul_add` falls back
//! to a very slow libm call, and Rust never contracts a*b+c on its own, so the
//! results are bit-identical on every architecture and can be cross-checked.

use super::{CAT_FP, Kernel, Metric, Worker, always_ok, no_backend};
use std::hint::black_box;

pub fn kernels() -> Vec<Kernel> {
    vec![
        Kernel {
            id: "fp.vec_f32",
            category: CAT_FP,
            desc: "Vectorizable f32 multiply-add over 32 lanes held in L1 (peak SIMD width test)",
            unit: "GFLOPS",
            scale: 1e-9,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(VecF32 { iters: 1 << 22 }),
            check: always_ok,
            backend: no_backend,
        },
        Kernel {
            id: "fp.matmul_f64",
            category: CAT_FP,
            desc: "Cache-blocked dense f64 matrix multiply, 256x256 (x4 per run)",
            unit: "GFLOPS",
            scale: 1e-9,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(MatMul::new(256)),
            check: always_ok,
            backend: no_backend,
        },
        Kernel {
            id: "fp.nbody",
            category: CAT_FP,
            desc: "5-body gravity simulation (scalar f64, sqrt + divide latency bound)",
            unit: "Minter/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(NBody { steps: 100_000 }),
            check: always_ok,
            backend: no_backend,
        },
    ]
}

const VL: usize = 32;

struct VecF32 {
    iters: usize,
}

impl Worker for VecF32 {
    fn run(&mut self) -> u64 {
        let mut a = [0f32; VL];
        let mut b = [0f32; VL];
        let mut c = [0f32; VL];
        for i in 0..VL {
            a[i] = 1.0 + i as f32 * 0.01;
            b[i] = 0.999 - i as f32 * 1e-5;
            c[i] = 1e-3 + i as f32 * 1e-6;
        }
        let (b, c) = (black_box(b), black_box(c));
        for _ in 0..self.iters {
            for i in 0..VL {
                a[i] = a[i] * b[i] + c[i];
            }
        }
        black_box(a.iter().fold(0u64, |h, x| h.rotate_left(5) ^ x.to_bits() as u64))
    }
    fn work(&self) -> f64 {
        (self.iters * VL * 2) as f64
    }
}

struct MatMul {
    n: usize,
    a: Vec<f64>,
    b: Vec<f64>,
    c: Vec<f64>,
}

const REPS: usize = 4;
const JB: usize = 64;
const KB: usize = 32;

impl MatMul {
    fn new(n: usize) -> Self {
        assert!(n % JB == 0 && n % KB == 0);
        let a = (0..n * n).map(|i| ((i * 7 + 3) % 101) as f64 * 0.01).collect();
        let b = (0..n * n).map(|i| ((i * 13 + 5) % 97) as f64 * 0.01).collect();
        MatMul { n, a, b, c: vec![0.0; n * n] }
    }

    fn multiply(&mut self) {
        let n = self.n;
        self.c.fill(0.0);
        for jj in (0..n).step_by(JB) {
            for kk in (0..n).step_by(KB) {
                for i in 0..n {
                    let crow = &mut self.c[i * n + jj..i * n + jj + JB];
                    for k in kk..kk + KB {
                        let aik = self.a[i * n + k];
                        let brow = &self.b[k * n + jj..k * n + jj + JB];
                        for j in 0..JB {
                            crow[j] += aik * brow[j];
                        }
                    }
                }
            }
        }
    }
}

impl Worker for MatMul {
    fn run(&mut self) -> u64 {
        for _ in 0..REPS {
            self.multiply();
            black_box(&self.c);
        }
        self.c.iter().fold(0u64, |h, x| h.rotate_left(1) ^ x.to_bits())
    }
    fn work(&self) -> f64 {
        (2 * self.n * self.n * self.n * REPS) as f64
    }
}

#[derive(Clone, Copy)]
struct Body {
    x: [f64; 3],
    v: [f64; 3],
    m: f64,
}

const SOLAR_MASS: f64 = 4.0 * std::f64::consts::PI * std::f64::consts::PI;
const DAYS: f64 = 365.24;

fn bodies() -> [Body; 5] {
    let b = |x, y, z, vx: f64, vy: f64, vz: f64, m: f64| Body {
        x: [x, y, z],
        v: [vx * DAYS, vy * DAYS, vz * DAYS],
        m: m * SOLAR_MASS,
    };
    let mut bs = [
        Body { x: [0.0; 3], v: [0.0; 3], m: SOLAR_MASS },
        b(4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01,
          1.66007664274403694e-03, 7.69901118419740425e-03, -6.90460016972063023e-05,
          9.54791938424326609e-04),
        b(8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01,
          -2.76742510726862411e-03, 4.99852801234917238e-03, 2.30417297573763929e-05,
          2.85885980666130812e-04),
        b(1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01,
          2.96460137564761618e-03, 2.37847173959480950e-03, -2.96589568540237556e-05,
          4.36624404335156298e-05),
        b(1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01,
          2.68067772490389322e-03, 1.62824170038242295e-03, -9.51592254519715870e-05,
          5.15138902046611451e-05),
    ];
    let mut p = [0.0; 3];
    for body in &bs {
        for k in 0..3 {
            p[k] += body.v[k] * body.m;
        }
    }
    for k in 0..3 {
        bs[0].v[k] = -p[k] / SOLAR_MASS;
    }
    bs
}

fn energy(bs: &[Body; 5]) -> f64 {
    let mut e = 0.0;
    for i in 0..5 {
        let v2: f64 = bs[i].v.iter().map(|v| v * v).sum();
        e += 0.5 * bs[i].m * v2;
        for j in i + 1..5 {
            let mut d2 = 0.0;
            for k in 0..3 {
                let d = bs[i].x[k] - bs[j].x[k];
                d2 += d * d;
            }
            e -= bs[i].m * bs[j].m / d2.sqrt();
        }
    }
    e
}

fn advance(bs: &mut [Body; 5], dt: f64) {
    for i in 0..5 {
        for j in i + 1..5 {
            let d = [
                bs[i].x[0] - bs[j].x[0],
                bs[i].x[1] - bs[j].x[1],
                bs[i].x[2] - bs[j].x[2],
            ];
            let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            let mag = dt / (d2 * d2.sqrt());
            let (mi, mj) = (bs[i].m, bs[j].m);
            for k in 0..3 {
                bs[i].v[k] -= d[k] * mj * mag;
                bs[j].v[k] += d[k] * mi * mag;
            }
        }
    }
    for b in bs.iter_mut() {
        for k in 0..3 {
            b.x[k] += dt * b.v[k];
        }
    }
}

struct NBody {
    steps: usize,
}

impl Worker for NBody {
    fn run(&mut self) -> u64 {
        let mut bs = bodies();
        for _ in 0..self.steps {
            advance(&mut bs, 0.01);
        }
        black_box(energy(&bs)).to_bits()
    }
    fn work(&self) -> f64 {
        (self.steps * 10) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nbody_initial_energy_matches_reference() {
        // Reference value from the Computer Language Benchmarks Game.
        let e = energy(&bodies());
        assert!((e - (-0.169075164)).abs() < 1e-9, "{e}");
    }

    #[test]
    fn nbody_energy_after_1000_steps() {
        let mut bs = bodies();
        for _ in 0..1000 {
            advance(&mut bs, 0.01);
        }
        assert!((energy(&bs) - (-0.169087605)).abs() < 1e-9);
    }

    #[test]
    fn blocked_matmul_equals_naive() {
        let mut m = MatMul::new(64 * 2);
        m.multiply();
        let n = m.n;
        for &(i, j) in &[(0, 0), (5, 77), (127, 127), (60, 3)] {
            let mut s = 0.0;
            for k in 0..n {
                s += m.a[i * n + k] * m.b[k * n + j];
            }
            // Blocking changes summation order only in how partial sums are
            // grouped, not their order along k, so the result is exact.
            assert_eq!(m.c[i * n + j], s);
        }
    }

    #[test]
    fn vec_f32_stays_finite() {
        let c = VecF32 { iters: 1 << 16 }.run();
        assert_ne!(c, 0);
    }
}
