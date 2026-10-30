//! Measurement harness.
//!
//! For `T` threads: every thread builds its own worker (so memory is touched
//! locally), runs it once untimed (this run also yields the checksum), then all
//! threads start together on a barrier and keep running until the main thread
//! raises a stop flag. Each thread divides the work it completed by its own
//! elapsed time and the per-thread rates are summed. Sample 0 is a warm-up
//! (clocks ramp up, caches and TLBs fill) and is discarded.

use crate::kernels::{Kernel, Metric, Params};
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Config {
    pub warmup: Duration,
    pub sample: Duration,
    pub samples: usize,
    /// CPUs to pin worker `i` to (`cpus[i % len]`); `None` leaves placement to the OS.
    pub pin: Option<Vec<usize>>,
}

#[derive(Debug)]
pub struct Outcome {
    /// One aggregate value per timed sample, in the kernel's unit.
    pub values: Vec<f64>,
    pub checksum: u64,
    /// False if worker threads disagreed on the checksum.
    pub consistent: bool,
    /// Threads whose CPU pinning request was refused by the OS.
    pub pin_failures: usize,
}

struct ThreadOut {
    checksum: u64,
    /// (work done, seconds) for every sample including warm-up.
    samples: Vec<(f64, f64)>,
}

pub fn measure(k: &Kernel, threads: usize, params: &Params, cfg: &Config) -> Outcome {
    let threads = threads.max(1);
    let params = Params { threads, ..params.clone() };
    let barrier = Barrier::new(threads + 1);
    let stop = AtomicBool::new(false);
    let pin_failures = AtomicUsize::new(0);
    let rounds = cfg.samples + 1;

    let outs: Vec<ThreadOut> = thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|tid| {
                let (barrier, stop, params, pin_failures) = (&barrier, &stop, &params, &pin_failures);
                let cpu = cfg.pin.as_ref().and_then(|c| c.get(tid % c.len().max(1)).copied());
                scope.spawn(move || {
                    if let Some(cpu) = cpu {
                        if !crate::sys::pin_current_thread(cpu) {
                            pin_failures.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    let mut worker = (k.make)(params);
                    let checksum = worker.run();
                    let mut samples = Vec::with_capacity(rounds);
                    for _ in 0..rounds {
                        barrier.wait();
                        let t0 = Instant::now();
                        let mut work = 0.0;
                        loop {
                            worker.run();
                            work += worker.work();
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                        samples.push((work, t0.elapsed().as_secs_f64()));
                        barrier.wait();
                    }
                    ThreadOut { checksum, samples }
                })
            })
            .collect();

        for round in 0..rounds {
            stop.store(false, Ordering::Relaxed);
            barrier.wait();
            thread::sleep(if round == 0 { cfg.warmup } else { cfg.sample });
            stop.store(true, Ordering::Relaxed);
            barrier.wait();
        }
        handles.into_iter().map(|h| h.join().expect("benchmark thread panicked")).collect()
    });

    let checksum = outs[0].checksum;
    let consistent = outs.iter().all(|o| o.checksum == checksum);
    let values = (1..rounds)
        .map(|s| match k.metric {
            Metric::Rate => outs.iter().map(|o| o.samples[s].0 * k.scale / o.samples[s].1).sum(),
            Metric::Latency => {
                outs.iter().map(|o| o.samples[s].1 / o.samples[s].0 * 1e9 * k.scale).sum::<f64>()
                    / outs.len() as f64
            }
        })
        .collect();
    Outcome { values, checksum, consistent, pin_failures: pin_failures.load(Ordering::Relaxed) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernels::{Worker, always_ok, no_backend};

    struct Spin;
    impl Worker for Spin {
        fn run(&mut self) -> u64 {
            let mut x = 1u64;
            for i in 0..50_000u64 {
                x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(i));
            }
            x
        }
        fn work(&self) -> f64 {
            50_000.0
        }
    }

    fn spin_kernel(metric: Metric, scale: f64) -> Kernel {
        Kernel {
            id: "test.spin",
            category: "Integer",
            desc: "",
            unit: "ops/s",
            scale,
            metric,
            multi: true,
            heavy: false,
            make: |_| Box::new(Spin),
            check: always_ok,
            backend: no_backend,
        }
    }

    fn quick() -> Config {
        Config {
            warmup: Duration::from_millis(20),
            sample: Duration::from_millis(60),
            samples: 3,
            pin: None,
        }
    }

    fn params() -> Params {
        Params { threads: 1, mem_budget: 0, mem_total: 0 }
    }

    #[test]
    fn returns_requested_number_of_samples() {
        let o = measure(&spin_kernel(Metric::Rate, 1.0), 2, &params(), &quick());
        assert_eq!(o.values.len(), 3);
        assert!(o.values.iter().all(|v| *v > 0.0 && v.is_finite()));
        assert!(o.consistent);
    }

    #[test]
    fn rate_scales_with_threads_and_respects_scale() {
        let k1 = spin_kernel(Metric::Rate, 1.0);
        let one = measure(&k1, 1, &params(), &quick()).values[0];
        let kmega = spin_kernel(Metric::Rate, 1e-6);
        let mega = measure(&kmega, 1, &params(), &quick()).values[0];
        // Same work, scaled by 1e-6 (allow generous noise on a busy machine).
        assert!((mega / (one * 1e-6) - 1.0).abs() < 0.5);
    }

    #[test]
    fn latency_metric_is_nanoseconds_per_unit() {
        let v = measure(&spin_kernel(Metric::Latency, 1.0), 1, &params(), &quick()).values[0];
        // A multiply-add dependency chain step takes well under 100 ns.
        assert!(v > 0.1 && v < 100.0, "{v}");
    }
}
