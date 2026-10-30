//! A/B comparison of two reports. Ratios are always "B relative to A" and
//! oriented so that > 1.00x means B is better, for lower-is-better metrics too.

use super::run::{build_rows, system_rows};
use super::{Align, Block, Doc, Table, fmt_num, fmt_ratio};
use crate::kernels::CATEGORIES;
use crate::model::{KernelResult, Report, STATUS_OK, STATUS_SKIPPED};
use crate::stats::geomean;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct CompareOpts {
    pub cost_a: Option<f64>,
    pub cost_b: Option<f64>,
    pub watts_a: Option<f64>,
    pub watts_b: Option<f64>,
}

/// Ratio of B to A, > 1 meaning B is better.
pub fn ratio(higher_is_better: bool, a: f64, b: f64) -> f64 {
    if higher_is_better { b / a } else { a / b }
}

struct Pair<'a> {
    a: &'a KernelResult,
    b: &'a KernelResult,
    st: Option<f64>,
    mt: Option<f64>,
}

fn pairs<'a>(a: &'a Report, b: &'a Report) -> Vec<Pair<'a>> {
    let bmap: BTreeMap<&str, &KernelResult> = b.results.iter().map(|k| (k.id.as_str(), k)).collect();
    a.results
        .iter()
        .filter_map(|ka| {
            let kb = *bmap.get(ka.id.as_str())?;
            let ok = ka.status == STATUS_OK && kb.status == STATUS_OK;
            let r = |x: &Option<crate::model::Measurement>, y: &Option<crate::model::Measurement>| {
                x.as_ref().zip(y.as_ref()).filter(|_| ok).map(|(x, y)| ratio(ka.higher_is_better, x.median, y.median))
            };
            Some(Pair { a: ka, b: kb, st: r(&ka.single, &kb.single), mt: r(&ka.multi, &kb.multi) })
        })
        .collect()
}

/// Physical cores the multi-thread run used; falls back to threads if the
/// topology was not exposed (then "per core" really means "per thread").
fn cores(r: &Report) -> f64 {
    let c = &r.config;
    (if c.cores_used > 0 { c.cores_used } else { c.threads }).max(1) as f64
}

pub fn comparability(a: &Report, b: &Report) -> Vec<String> {
    let mut w = Vec::new();
    if a.tool_version != b.tool_version {
        w.push(format!("Different cpubench versions ({} vs {}); kernels may differ. Do not compare.", a.tool_version, b.tool_version));
    }
    if a.build.libc != b.build.libc {
        w.push(format!(
            "Different libc ({} vs {}): memcpy/malloc implementations differ, which skews memory and allocator results. Build both with the same libc.",
            a.build.libc, b.build.libc
        ));
    }
    if a.build.opt_level != b.build.opt_level || a.build.profile != b.build.profile {
        w.push("Builds use different optimisation settings.".to_string());
    }
    if a.profile != b.profile {
        w.push(format!("Different profiles ({} vs {}); sample counts/durations differ.", a.profile, b.profile));
    }
    for (name, r) in [(&a.label, a), (&b.label, b)] {
        if r.build.rustflags.contains("target-cpu=native") {
            w.push(format!("{name}: built with target-cpu=native, so each machine ran code compiled for its own instruction set (best-case per CPU, not an identical-binary comparison)."));
        }
    }
    if a.system.arch == b.system.arch && a.build.compiled_features != b.build.compiled_features {
        w.push("Same architecture but different compile-time SIMD features; results are not like-for-like.".to_string());
    }
    if a.config.threads != b.config.threads {
        w.push(format!(
            "Multi-thread runs used different thread counts ({} vs {}); total-throughput ratios reflect machine size as well as per-core speed (see the per-core rows).",
            a.config.threads, b.config.threads
        ));
    }
    if a.config.mem_budget_bytes != b.config.mem_budget_bytes {
        w.push("Streaming memory budgets differ between the runs.".to_string());
    }
    let ids = |r: &Report| r.results.iter().map(|k| k.id.clone()).collect::<Vec<_>>();
    for id in ids(a).iter().filter(|i| !ids(b).contains(i)) {
        w.push(format!("{id} exists only in {}'s report.", a.label));
    }
    for id in ids(b).iter().filter(|i| !ids(a).contains(i)) {
        w.push(format!("{id} exists only in {}'s report.", b.label));
    }
    for p in pairs(a, b) {
        if p.a.status == STATUS_OK && p.b.status == STATUS_OK && p.a.checksum != p.b.checksum {
            w.push(format!(
                "CHECKSUM MISMATCH in {}: {} vs {}. The machines computed different results - investigate before trusting this kernel.",
                p.a.id, p.a.checksum, p.b.checksum
            ));
        }
    }
    w
}

pub fn build_doc(a: &Report, b: &Report, o: &CompareOpts) -> Doc {
    let ps = pairs(a, b);
    let mut blocks = Vec::new();

    let mut warnings = comparability(a, b);
    warnings.extend(a.warnings.iter().map(|w| format!("[{}] {w}", a.label)));
    warnings.extend(b.warnings.iter().map(|w| format!("[{}] {w}", b.label)));
    if !warnings.is_empty() {
        blocks.push(Block::Heading("Findings (read before using the numbers)".into()));
        blocks.push(Block::Warnings(warnings));
    }

    // ---- summary
    let geo = |cat: Option<&str>, multi: bool| {
        geomean(ps.iter().filter(|p| cat.is_none_or(|c| p.a.category == c)).filter_map(|p| if multi { p.mt } else { p.st }))
    };
    let (st_all, mt_all) = (geo(None, false), geo(None, true));
    blocks.push(Block::Heading(format!("Summary: {} relative to {}", b.label, a.label)));
    blocks.push(Block::Para(format!(
        "Geometric mean of per-kernel ratios; above 1.00x means {} is faster. Single-thread compares one core/thread; multi-thread compares total throughput of the whole machine ({} threads vs {} threads).",
        b.label, b.config.threads, a.config.threads
    )));
    let mut t = Table::new(&["Metric", &a.label, &b.label, &format!("{} vs {}", b.label, a.label)], &[Align::Left, Align::Right, Align::Right, Align::Right]);
    let score = |x: Option<f64>| x.map_or("-".to_string(), |v| format!("{v:.0}"));
    t.row(["Single-thread score".to_string(), score(a.scores.single), score(b.scores.single), st_all.map_or("-".into(), fmt_ratio)]);
    t.row(["Multi-thread score (whole machine)".to_string(), score(a.scores.multi), score(b.scores.multi), mt_all.map_or("-".into(), fmt_ratio)]);
    for cat in CATEGORIES {
        let (s, m) = (geo(Some(cat), false), geo(Some(cat), true));
        if s.is_some() || m.is_some() {
            t.row([
                format!("  {cat}: 1 thread / whole machine"),
                String::new(),
                String::new(),
                format!("{} / {}", s.map_or("-".into(), fmt_ratio), m.map_or("-".into(), fmt_ratio)),
            ]);
        }
    }
    if let Some(m) = mt_all {
        t.row([
            "Multi-thread per physical core used".to_string(),
            format!("{} cores", cores(a)),
            format!("{} cores", cores(b)),
            fmt_ratio(m * cores(a) / cores(b)),
        ]);
        t.row([
            "Multi-thread per logical CPU".to_string(),
            format!("{} CPUs", a.config.threads),
            format!("{} CPUs", b.config.threads),
            fmt_ratio(m * a.config.threads as f64 / b.config.threads.max(1) as f64),
        ]);
        if let (Some(ca), Some(cb)) = (o.cost_a, o.cost_b) {
            t.row(["Whole-machine performance per cost".to_string(), fmt_num(ca), fmt_num(cb), fmt_ratio(m * ca / cb)]);
        }
        if let (Some(wa), Some(wb)) = (o.watts_a, o.watts_b) {
            t.row(["Whole-machine performance per watt".to_string(), format!("{} W", fmt_num(wa)), format!("{} W", fmt_num(wb)), fmt_ratio(m * wa / wb)]);
        }
    }
    blocks.push(Block::Table(t));
    if o.cost_a.is_some() != o.cost_b.is_some() || o.watts_a.is_some() != o.watts_b.is_some() {
        blocks.push(Block::Warnings(vec!["Cost or watts was given for only one machine; the corresponding ratio is omitted.".into()]));
    }
    if o.watts_a.is_some() || o.cost_a.is_some() {
        blocks.push(Block::Para(
            "Cost and watts are user-supplied inputs (not measured). Watts should be comparable figures such as full-load wall power or total TDP; cost in the same currency.".into(),
        ));
    }

    // ---- bars
    let bars = |multi: bool| -> Vec<(String, f64)> {
        ps.iter().filter_map(|p| (if multi { p.mt } else { p.st }).map(|r| (p.a.id.clone(), r))).collect()
    };
    blocks.push(Block::Heading("Per-kernel ratios".into()));
    blocks.push(Block::Bars { title: format!("1 thread: {} vs {}", b.label, a.label), items: bars(false) });
    blocks.push(Block::Bars { title: format!("Whole machine: {} vs {}", b.label, a.label), items: bars(true) });

    // ---- detail table
    blocks.push(Block::Heading("Detail".into()));
    let mut t = Table::new(
        &["Kernel", "Unit", &format!("{} 1T", a.label), &format!("{} 1T", b.label), "Ratio", &format!("{} MT", a.label), &format!("{} MT", b.label), "Ratio", "Notes"],
        &[Align::Left, Align::Left, Align::Right, Align::Right, Align::Right, Align::Right, Align::Right, Align::Right, Align::Left],
    );
    let med = |k: &KernelResult, multi: bool| (if multi { &k.multi } else { &k.single }).as_ref().map(|m| fmt_num(m.median)).unwrap_or_else(|| "-".into());
    let both_skipped = |p: &Pair| p.a.status == STATUS_SKIPPED && p.b.status == STATUS_SKIPPED;
    let hidden = ps.iter().filter(|p| both_skipped(p)).count();
    for p in ps.iter().filter(|p| !both_skipped(p)) {
        let mut notes = Vec::new();
        for (name, k) in [(&a.label, p.a), (&b.label, p.b)] {
            if k.status != STATUS_OK {
                notes.push(format!("{name}: {} ({})", k.status.to_uppercase(), k.note.as_deref().unwrap_or("")));
            }
            if let Some(be) = &k.backend {
                notes.push(format!("{name}: {be}"));
            }
            if k.libc_sensitive {
                notes.push(format!("{name}: libc-sensitive ({})", if name == &a.label { &a.build.libc } else { &b.build.libc }));
            }
        }
        t.row([
            p.a.id.clone(),
            p.a.unit.clone(),
            med(p.a, false),
            med(p.b, false),
            p.st.map_or("-".into(), fmt_ratio),
            med(p.a, true),
            med(p.b, true),
            p.mt.map_or("-".into(), fmt_ratio),
            notes.join("; "),
        ]);
    }
    blocks.push(Block::Table(t));
    if hidden > 0 {
        blocks.push(Block::Para(format!("{hidden} kernel(s) skipped in both reports are not shown (see the findings above for the reasons).")));
    }

    // ---- systems
    blocks.push(Block::Heading("Systems".into()));
    let mut t = Table::new(&["Item", &a.label, &b.label], &[Align::Left, Align::Left, Align::Left]);
    for ((k, va), (_, vb)) in system_rows(&a.system).into_iter().zip(system_rows(&b.system)) {
        t.row([k.to_string(), va, vb]);
    }
    for ((k, va), (_, vb)) in build_rows(a).into_iter().zip(build_rows(b)) {
        t.row([k.to_string(), va, vb]);
    }
    t.row(["Threads used (multi-thread runs)".to_string(), a.config.threads.to_string(), b.config.threads.to_string()]);
    blocks.push(Block::Table(t));

    Doc {
        title: format!("CPU Comparison - {} vs {}", a.label, b.label),
        subtitle: format!("{} ({}) vs {} ({}) | ratios are {} relative to {}", a.system.cpu_model, a.system.arch, b.system.cpu_model, b.system.arch, b.label, a.label),
        blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::testutil::sample_report;
    use crate::report::render;

    fn pair() -> (Report, Report) {
        let a = sample_report();
        let mut b = sample_report();
        b.label = "arm".into();
        b.results[0].single.as_mut().unwrap().median = 150.0; // 1.5x faster throughput
        b.results[0].multi.as_mut().unwrap().median = 1400.0; // 2x
        b.results[1].single.as_mut().unwrap().median = 10.0; // latency doubled = 0.5x
        (a, b)
    }

    #[test]
    fn ratio_orientation() {
        assert_eq!(ratio(true, 100.0, 150.0), 1.5);
        assert_eq!(ratio(false, 5.0, 10.0), 0.5);
    }

    #[test]
    fn latency_is_inverted_in_comparison() {
        let (a, b) = pair();
        let ps = pairs(&a, &b);
        assert_eq!(ps[0].st, Some(1.5));
        assert_eq!(ps[0].mt, Some(2.0));
        assert_eq!(ps[1].st, Some(0.5));
        assert_eq!(ps[1].mt, None);
    }

    #[test]
    fn checksum_mismatch_and_libc_are_flagged() {
        let (mut a, mut b) = pair();
        b.results[0].checksum = "deadbeef".into();
        a.build.libc = "gnu".into();
        b.build.libc = "musl".into();
        let w = comparability(&a, &b).join("\n");
        assert!(w.contains("CHECKSUM MISMATCH in int.a"));
        assert!(w.contains("Different libc"));
    }

    #[test]
    fn non_ok_kernels_are_excluded_from_ratios_but_listed() {
        let (a, mut b) = pair();
        b.results[0].status = "unsupported".into();
        b.results[0].note = Some("no such feature".into());
        let ps = pairs(&a, &b);
        assert_eq!(ps[0].st, None);
        let text = render::text(&build_doc(&a, &b, &CompareOpts::default()));
        assert!(text.contains("UNSUPPORTED (no such feature)"), "{text}");
    }

    #[test]
    fn cost_and_watts_rows_use_whole_machine_ratio() {
        let (a, mut b) = pair();
        b.config.threads = 16;
        let o = CompareOpts { cost_a: Some(1000.0), cost_b: Some(2000.0), watts_a: Some(300.0), watts_b: Some(150.0) };
        let doc = build_doc(&a, &b, &o);
        let text = render::text(&doc);
        // MT geomean ratio is 2.0 (only one MT kernel): per cost 2.0*1000/2000 = 1.00x, per watt 2.0*300/150 = 4.00x
        assert!(text.contains("performance per cost") && text.contains("1.00x"), "{text}");
        assert!(text.contains("performance per watt") && text.contains("4.00x"), "{text}");
    }
}
