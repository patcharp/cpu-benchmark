//! Composite scores. 1000 points = one thread of the reference machine, so a
//! score is only comparable between runs of the same cpubench version. The
//! per-kernel numbers (and the `compare` ratios) are the primary evidence; the
//! scores are a convenience summary and are always printed next to them.

use crate::kernels::CATEGORIES;
use crate::model::{CategoryScore, KernelResult, STATUS_OK, Scores};
use crate::stats::geomean;

pub const REFERENCE_DESC: &str =
    "AMD Ryzen AI 9 HX 370 (Zen 5), one thread, WSL2, cpubench 0.1.0 static musl x86_64 baseline (SSE2) build";

/// Single-thread reference values, in each kernel's own unit.
pub const REFERENCE: &[(&str, f64)] = &[
    ("int.sort", 102.1),
    ("int.sieve", 2619.0),
    ("int.mix", 1119.0),
    ("int.collatz", 1504.0),
    ("fp.vec_f32", 13.02),
    ("fp.matmul_f64", 14.41),
    ("fp.nbody", 334.7),
    ("mem.copy", 34.12),
    ("mem.triad", 33.69),
    ("mem.memcpy_small", 11.9),
    ("mem.lat.32k", 1.022),
    ("mem.lat.256k", 3.133),
    ("mem.lat.2m", 9.169),
    ("mem.lat.16m", 115.8),
    ("mem.lat.128m", 148.5),
    ("mem.lat.512m", 171.8),
    ("crypto.sha256", 2387.0),
    ("crypto.sha512", 1127.0),
    ("crypto.aes256", 21890.2),
    ("comp.deflate", 69.05),
    ("comp.inflate", 199.8),
    ("data.hashmap", 57.83),
    ("data.json", 345.3),
    ("data.alloc", 36.84),
];

pub fn reference(id: &str) -> Option<f64> {
    REFERENCE.iter().find(|(k, _)| *k == id).map(|(_, v)| *v)
}

/// Speed-up of `value` over the reference (>1 is faster than reference).
pub fn relative(higher_is_better: bool, value: f64, reference: f64) -> f64 {
    if higher_is_better { value / reference } else { reference / value }
}

fn ratios<'a>(
    results: &'a [KernelResult],
    category: Option<&'a str>,
    multi: bool,
) -> impl Iterator<Item = f64> + 'a {
    results.iter().filter(move |r| r.status == STATUS_OK && category.is_none_or(|c| r.category == c)).filter_map(
        move |r| {
            let reference = reference(&r.id)?;
            let m = if multi { r.multi.as_ref()? } else { r.single.as_ref()? };
            // Multi-thread is expressed in "reference threads" of throughput;
            // latency kernels have no multi-thread result.
            Some(relative(r.higher_is_better, m.median, reference))
        },
    )
}

pub fn compute(results: &[KernelResult]) -> Scores {
    let score = |cat: Option<&str>, multi: bool| geomean(ratios(results, cat, multi)).map(|g| g * 1000.0);
    Scores {
        single: score(None, false),
        multi: score(None, true),
        categories: CATEGORIES
            .iter()
            .map(|c| CategoryScore { name: c.to_string(), single: score(Some(c), false), multi: score(Some(c), true) })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::summarize;

    fn res(id: &str, cat: &str, hib: bool, st: f64, mt: Option<f64>) -> KernelResult {
        KernelResult {
            id: id.into(),
            category: cat.into(),
            description: String::new(),
            unit: String::new(),
            higher_is_better: hib,
            status: STATUS_OK.into(),
            note: None,
            libc_sensitive: false,
            backend: None,
            single: Some(summarize(1, &[st])),
            multi: mt.map(|m| summarize(8, &[m])),
            checksum: String::new(),
            checksum_consistent: true,
        }
    }

    #[test]
    fn relative_direction() {
        assert_eq!(relative(true, 200.0, 100.0), 2.0);
        assert_eq!(relative(false, 50.0, 100.0), 2.0); // half the latency = twice as good
    }

    #[test]
    fn reference_table_is_complete_and_valid() {
        for k in crate::kernels::all() {
            let r = reference(k.id).unwrap_or_else(|| panic!("no reference value for {}", k.id));
            assert!(r.is_finite() && r > 0.0, "{}", k.id);
        }
    }

    #[test]
    fn unknown_kernels_do_not_distort_scores() {
        let r = vec![res("nonexistent.kernel", "Integer", true, 5.0, Some(9.0))];
        let s = compute(&r);
        assert!(s.single.is_none() && s.multi.is_none());
    }
}
