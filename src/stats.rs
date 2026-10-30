//! Sample statistics and small numeric helpers.

use crate::model::Measurement;

pub fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    match n {
        0 => f64::NAN,
        _ if n % 2 == 1 => sorted[n / 2],
        _ => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    }
}

/// Summarise raw samples. The median is the headline number; everything else
/// is kept so a reader can judge how noisy the run was.
pub fn summarize(threads: usize, samples: &[f64]) -> Measurement {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let n = samples.len() as f64;
    let mean = samples.iter().sum::<f64>() / n;
    let var = if samples.len() > 1 {
        samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)
    } else {
        0.0
    };
    let stddev = var.sqrt();
    Measurement {
        threads,
        median: median(&sorted),
        mean,
        min: sorted.first().copied().unwrap_or(f64::NAN),
        max: sorted.last().copied().unwrap_or(f64::NAN),
        stddev,
        cv_pct: if mean != 0.0 { stddev / mean * 100.0 } else { 0.0 },
        samples: samples.to_vec(),
    }
}

/// Geometric mean of strictly positive, finite values; `None` if there are none.
pub fn geomean(values: impl IntoIterator<Item = f64>) -> Option<f64> {
    let (mut sum, mut n) = (0.0, 0usize);
    for v in values {
        if v.is_finite() && v > 0.0 {
            sum += v.ln();
            n += 1;
        }
    }
    (n > 0).then(|| (sum / n as f64).exp())
}

/// Deterministic xorshift64* generator so every machine benchmarks identical data.
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    #[inline]
    pub fn below(&mut self, n: u64) -> u64 {
        ((self.next_u64() >> 32) * n) >> 32
    }
}

/// UTC "YYYY-MM-DD HH:MM:SS" from a unix timestamp (no chrono dependency).
pub fn fmt_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_odd_even() {
        assert_eq!(median(&[1.0, 2.0, 9.0]), 2.0);
        assert_eq!(median(&[1.0, 2.0, 4.0, 9.0]), 3.0);
        assert!(median(&[]).is_nan());
    }

    #[test]
    fn summarize_basic() {
        let m = summarize(4, &[10.0, 12.0, 11.0]);
        assert_eq!(m.threads, 4);
        assert_eq!(m.median, 11.0);
        assert_eq!(m.min, 10.0);
        assert_eq!(m.max, 12.0);
        assert!((m.mean - 11.0).abs() < 1e-12);
        assert!((m.stddev - 1.0).abs() < 1e-12);
        assert!((m.cv_pct - 100.0 / 11.0).abs() < 1e-9);
    }

    #[test]
    fn geomean_ignores_invalid() {
        let g = geomean([1.0, 4.0, f64::NAN, -3.0, 0.0]).unwrap();
        assert!((g - 2.0).abs() < 1e-12);
        assert!(geomean([f64::NAN]).is_none());
    }

    #[test]
    fn rng_is_deterministic() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut r = Rng::new(1);
        assert!((0..1000).all(|_| r.below(10) < 10));
    }

    #[test]
    fn utc_formatting() {
        assert_eq!(fmt_utc(0), "1970-01-01 00:00:00");
        assert_eq!(fmt_utc(1_782_000_000), "2026-06-21 00:00:00");
        assert_eq!(fmt_utc(951_782_400), "2000-02-29 00:00:00");
    }
}
