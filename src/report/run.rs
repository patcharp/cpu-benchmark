//! Single-run report: findings analysis and `Doc` construction.

use super::{Align, Block, Doc, Table, fmt_bytes, fmt_num, or_unknown};
use crate::kernels::CATEGORIES;
use crate::model::{Report, STATUS_OK, STATUS_SKIPPED, SysInfo};
use crate::score;
use crate::stats::fmt_utc;
use crate::sysinfo::{interesting_features, missing_feature_notes};

/// CV above this is called out: the number is not stable enough to trust blindly.
pub const NOISY_CV_PCT: f64 = 5.0;
/// A run started with load above this many runnable tasks per logical CPU counts as "busy".
pub const IDLE_LOAD_PER_CPU: f64 = 0.25;
/// Kernels that allocate so often that a global allocator lock decides their multi-thread speed.
const MALLOC_LOCK_BOUND: &[&str] = &["data.json", "data.alloc"];

/// Everything about the run a reader should know before trusting the numbers.
pub fn analyze(r: &Report) -> Vec<String> {
    // Ordered by importance: how/where it ran first, per-kernel details next,
    // and the (usually long) list of noisy measurements as a single last item.
    let mut w = Vec::new();
    let s = &r.system;
    if r.build.debug_assertions {
        w.push("This is a DEBUG build; results are meaningless. Build with --release.".to_string());
    }
    if r.profile != "full" {
        w.push(format!(
            "Profile '{}' is a short run for development/smoke testing. Use the 'full' profile for any decision.",
            r.profile
        ));
    }
    if let Some([l1, l5, l15]) = s.load_avg {
        if l1.max(l5) >= IDLE_LOAD_PER_CPU * s.logical_cpus as f64 {
            w.push(format!(
                "The machine was not idle when the run started (load average {l1:.2} / {l5:.2} / {l15:.2} on {} logical CPUs): other processes competed for CPU and memory, so scaling efficiency and absolute numbers include that interference.",
                s.logical_cpus
            ));
        }
    }
    if let Some(v) = &s.virtualization {
        w.push(format!("Running virtualized/containerized ({v}); numbers include hypervisor effects."));
    }
    if let Some(l) = s.cgroup_cpu_limit {
        w.push(format!("cgroup limits this process to {l:.2} CPUs."));
    }
    if s.usable_parallelism < s.logical_cpus {
        w.push(format!(
            "Process may use only {} of {} logical CPUs (affinity/cgroup restriction).",
            s.usable_parallelism, s.logical_cpus
        ));
    }
    if let Some(g) = s.governor.as_deref().filter(|g| *g != "performance") {
        w.push(format!("CPU frequency governor is '{g}', not 'performance'; clocks may ramp and vary during the run."));
    }
    // Nodes that share the same CPU list (e.g. emulated NUMA on a Raspberry Pi) carry no locality.
    let distinct_node_cpus: std::collections::BTreeSet<&str> =
        s.numa.iter().filter(|n| !n.cpus.is_empty()).map(|n| n.cpus.as_str()).collect();
    if distinct_node_cpus.len() > 1 && !r.config.pinned {
        w.push(format!(
            "{} NUMA nodes with different CPU sets are present and threads are not pinned: placement is left to the OS. Use --cpus to restrict a run to one node/socket.",
            distinct_node_cpus.len()
        ));
    } else if s.numa.len() > 1 && distinct_node_cpus.len() <= 1 {
        w.push(format!(
            "{} NUMA nodes are reported but all share the same CPUs (emulated nodes); there is no real memory locality to control.",
            s.numa.len()
        ));
    }
    if s.page_size > 4096 {
        w.push(format!(
            "Kernel page size is {}; TLB behaviour (large-footprint memory latency) differs from 4 KiB-page systems.",
            fmt_bytes(s.page_size)
        ));
    }
    if s.sockets == 0 {
        w.push("CPU topology (sockets/cores) is not exposed by this system; core counts are unknown.".to_string());
    }
    if r.build.libc == "musl" {
        let ids: Vec<&str> = r.results.iter().filter(|k| k.libc_sensitive && k.status == STATUS_OK).map(|k| k.id.as_str()).collect();
        if !ids.is_empty() {
            w.push(format!(
                "Built against musl: {} depend on libc malloc/memcpy, so they reflect musl's allocator and memcpy (typically slower than glibc, and tuned differently per architecture), not only the CPU. For real-world numbers use a glibc build on the machine's own OS.",
                ids.join(", ")
            ));
            let locked: Vec<&str> =
                r.results.iter().filter(|k| k.multi.is_some() && MALLOC_LOCK_BOUND.contains(&k.id.as_str())).map(|k| k.id.as_str()).collect();
            if !locked.is_empty() {
                w.push(format!(
                    "musl's malloc takes a global lock when several threads run, so the multi-thread results of {} partly measure lock contention and scale far worse than the CPU would.",
                    locked.join(", ")
                ));
            }
        }
    }
    w.extend(missing_feature_notes(&s.arch, &s.features));
    let mut noisy = Vec::new();
    for k in &r.results {
        if k.status != STATUS_OK && k.status != STATUS_SKIPPED {
            w.push(format!("{}: {} - {}", k.id, k.status.to_uppercase(), k.note.as_deref().unwrap_or("no reason recorded")));
        }
        if let Some(b) = k.backend.as_deref().filter(|b| b.starts_with("software")) {
            w.push(format!("{}: running a {b}, not hardware acceleration.", k.id));
        }
        if !k.checksum_consistent {
            w.push(format!("{}: worker threads produced different checksums (non-deterministic result).", k.id));
        }
        for (label, m) in [("1T", &k.single), ("MT", &k.multi)] {
            if let Some(m) = m.as_ref().filter(|m| m.cv_pct > NOISY_CV_PCT) {
                noisy.push(format!("{} {label} {:.1}%", k.id, m.cv_pct));
            }
        }
    }
    let skipped: Vec<String> = r
        .results
        .iter()
        .filter(|k| k.status == STATUS_SKIPPED)
        .map(|k| format!("{} ({})", k.id, k.note.as_deref().unwrap_or("no reason recorded")))
        .collect();
    if !skipped.is_empty() {
        w.push(format!("{} kernel(s) were not measured: {}.", skipped.len(), skipped.join("; ")));
    }
    if !noisy.is_empty() {
        w.push(format!(
            "{} measurement(s) are noisy (coefficient of variation above {NOISY_CV_PCT:.0}% between samples): {}. Read these with the +/- CV% in mind; raw samples are in the JSON.",
            noisy.len(),
            noisy.join(", ")
        ));
    }
    w
}

fn kv(rows: Vec<(&str, String)>) -> Block {
    let mut t = Table::new(&["Item", "Value"], &[Align::Left, Align::Left]);
    rows.into_iter().for_each(|(k, v)| t.row([k.to_string(), v]));
    Block::Table(t)
}

pub fn system_rows(s: &SysInfo) -> Vec<(&'static str, String)> {
    let caches: Vec<String> = s
        .caches
        .iter()
        .map(|c| {
            let kind = match c.kind.as_str() {
                "Data" => "d",
                "Instruction" => "i",
                _ => "",
            };
            format!("L{}{kind} {} x{}", c.level, fmt_bytes(c.size_bytes), c.instances)
        })
        .collect();
    let numa: Vec<String> = s
        .numa
        .iter()
        .map(|n| format!("node{}: cpus {}{}", n.id, n.cpus, n.mem_bytes.map_or(String::new(), |m| format!(" ({})", fmt_bytes(m)))))
        .collect();
    vec![
        ("CPU", s.cpu_model.clone()),
        ("Vendor / architecture", format!("{} / {}", s.cpu_vendor, s.arch)),
        ("Machine", if s.machine.is_empty() { "unknown".into() } else { s.machine.clone() }),
        (
            "Topology",
            if s.sockets == 0 {
                format!("{} logical CPUs (sockets/cores unknown)", s.logical_cpus)
            } else {
                format!(
                    "{} socket(s), {} physical cores, {} logical CPUs ({} thread(s)/core)",
                    s.sockets, s.physical_cores, s.logical_cpus, s.threads_per_core
                )
            },
        ),
        ("NUMA", if numa.is_empty() { "unknown".into() } else { numa.join("; ") }),
        ("Caches", if caches.is_empty() { "unknown".into() } else { caches.join(", ") }),
        ("Total last-level cache", if s.llc_total_bytes() == 0 { "unknown".into() } else { fmt_bytes(s.llc_total_bytes()) }),
        ("Memory", fmt_bytes(s.mem_total_bytes)),
        ("Page size / THP", format!("{} / {}", fmt_bytes(s.page_size), or_unknown(s.thp.clone()))),
        ("Max frequency", s.max_freq_mhz.map_or("unknown".into(), |f| format!("{f:.0} MHz"))),
        ("Governor", or_unknown(s.governor.clone())),
        ("Virtualization", s.virtualization.clone().unwrap_or_else(|| "none detected".into())),
        ("OS / kernel", format!("{} / {}", s.os, s.kernel)),
        ("ISA features", interesting_features(&s.arch, &s.features).join(" ")),
    ]
}

pub fn build_rows(r: &Report) -> Vec<(&'static str, String)> {
    let b = &r.build;
    vec![
        ("cpubench version", r.tool_version.clone()),
        ("Target / libc", format!("{} / {}", b.target, b.libc)),
        ("rustc", b.rustc.clone()),
        ("Profile / opt-level", format!("{} / {}", b.profile, b.opt_level)),
        ("RUSTFLAGS", if b.rustflags.is_empty() { "(none)".into() } else { b.rustflags.clone() }),
        ("Compile-time SIMD features", if b.compiled_features.is_empty() { "baseline only".into() } else { b.compiled_features.join(" ") }),
    ]
}

fn category_table(r: &Report, cat: &str) -> Table {
    let mt_threads = r.config.threads;
    let mut t = Table::new(
        &["Kernel", "Unit", "1 thread", "CV%", &format!("{mt_threads} threads"), "CV%", "Speed-up", "Efficiency", "Backend / status"],
        &[Align::Left, Align::Left, Align::Right, Align::Right, Align::Right, Align::Right, Align::Right, Align::Right, Align::Left],
    );
    for k in r.results.iter().filter(|k| k.category == cat) {
        let (st, mt) = (k.single.as_ref(), k.multi.as_ref());
        let cell = |v: Option<f64>| v.map_or("-".to_string(), fmt_num);
        let speedup = st.zip(mt).filter(|_| k.higher_is_better).map(|(s, m)| m.median / s.median);
        let eff = speedup.map(|x| x / mt.map_or(1, |m| m.threads) as f64 * 100.0);
        let mut tail = k.backend.clone().unwrap_or_default();
        if k.libc_sensitive {
            tail = format!("{tail} [libc-sensitive]").trim().to_string();
        }
        if k.status != STATUS_OK {
            tail = format!("{} - {}", k.status.to_uppercase(), k.note.as_deref().unwrap_or(""));
        } else if mt.is_none() && k.single.is_some() && k.higher_is_better && mt_threads > 1 {
            tail = format!("{tail} (single-thread by design)").trim().to_string();
        }
        t.row([
            k.id.clone(),
            k.unit.clone(),
            cell(st.map(|m| m.median)),
            st.map_or("-".into(), |m| format!("{:.1}", m.cv_pct)),
            cell(mt.map(|m| m.median)),
            mt.map_or("-".into(), |m| format!("{:.1}", m.cv_pct)),
            speedup.map_or("-".into(), |x| format!("{x:.1}x")),
            eff.map_or("-".into(), |e| format!("{e:.0}%")),
            tail,
        ]);
    }
    t
}

pub fn build_doc(r: &Report) -> Doc {
    let mut blocks = Vec::new();
    if !r.warnings.is_empty() {
        blocks.push(Block::Heading("Findings (read before using the numbers)".into()));
        blocks.push(Block::Warnings(r.warnings.clone()));
    }

    blocks.push(Block::Heading("Summary scores".into()));
    let mut t = Table::new(
        &["Category", "1 thread", &format!("{} thread{}", r.config.threads, if r.config.threads == 1 { "" } else { "s" })],
        &[Align::Left, Align::Right, Align::Right],
    );
    let sc = |v: Option<f64>| v.map_or("-".to_string(), |x| format!("{x:.0}"));
    t.row(["OVERALL".to_string(), sc(r.scores.single), sc(r.scores.multi)]);
    r.scores.categories.iter().for_each(|c| t.row([c.name.clone(), sc(c.single), sc(c.multi)]));
    blocks.push(Block::Table(t));
    blocks.push(Block::Para(format!(
        "Score = geometric mean of per-kernel speed relative to a reference (1000 = one thread of: {}). The multi-thread score is aggregate throughput measured in units of that one reference thread, so it grows with core count. Scores only compare runs of the same cpubench version; the per-kernel numbers below are the primary evidence.",
        score::REFERENCE_DESC
    )));

    for cat in CATEGORIES {
        if r.results.iter().any(|k| k.category == cat) {
            blocks.push(Block::Heading(format!("{cat} kernels")));
            blocks.push(Block::Table(category_table(r, cat)));
        }
    }
    blocks.push(Block::Para(
        "Values are the median of the timed samples; CV% is the coefficient of variation between samples. Speed-up = all-thread / 1-thread throughput; Efficiency = speed-up / thread count (100% = perfect linear scaling; SMT siblings and shared memory bandwidth lower it). Latency rows are lower-is-better and single-thread only.".into(),
    ));

    blocks.push(Block::Heading("System".into()));
    blocks.push(kv(system_rows(&r.system)));
    blocks.push(Block::Heading("Build and run configuration".into()));
    let c = &r.config;
    let mut rows = build_rows(r);
    rows.push(("Profile", format!("{} ({} samples x {:.1}s, {:.1}s warm-up discarded)", r.profile, c.samples, c.sample_secs, c.warmup_secs)));
    rows.push((
        "Threads",
        format!(
            "{} worker thread(s) for multi-thread runs; {}",
            c.threads,
            if c.pinned { format!("pinned to CPUs {}", cpu_summary(&c.cpus)) } else { "not pinned".into() }
        ),
    ));
    rows.push(("Streaming memory budget", fmt_bytes(c.mem_budget_bytes)));
    rows.push(("Wall time", format!("{:.0} s", r.elapsed_secs)));
    blocks.push(kv(rows));

    blocks.push(Block::Heading("Correctness checksums".into()));
    blocks.push(Block::Para(
        "Every kernel is deterministic. These digests must be identical on every machine and architecture; `compare` flags any difference.".into(),
    ));
    let mut t = Table::new(&["Kernel", "Checksum", "Threads agree"], &[Align::Left, Align::Left, Align::Left]);
    r.results
        .iter()
        .filter(|k| k.status == STATUS_OK)
        .for_each(|k| t.row([k.id.clone(), k.checksum.clone(), if k.checksum_consistent { "yes" } else { "NO" }.to_string()]));
    blocks.push(Block::Table(t));

    Doc {
        title: format!("CPU Benchmark Report - {}", r.label),
        subtitle: format!("{} | {} | {} UTC | cpubench {}", r.system.cpu_model, r.system.arch, fmt_utc(r.started_unix), r.tool_version),
        blocks,
    }
}

fn cpu_summary(cpus: &[usize]) -> String {
    match (cpus.first(), cpus.last()) {
        (Some(a), Some(b)) if cpus.len() > 1 => format!("{a}..{b} ({} cpus)", cpus.len()),
        (Some(a), _) => a.to_string(),
        _ => "-".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use crate::report::testutil::sample_report;

    #[test]
    fn analyze_reports_hidden_gotchas() {
        let mut r = sample_report();
        r.results[0].backend = Some("software fallback (no SHA-NI)".into());
        r.results[1].status = STATUS_UNSUPPORTED.into();
        r.results[1].note = Some("needs more RAM".into());
        let w = analyze(&r).join("\n");
        assert!(w.contains("quick") && w.contains("full"), "non-final profile must be flagged");
        assert!(w.contains("software fallback"), "{w}");
        assert!(w.contains("UNSUPPORTED - needs more RAM"));
        assert!(w.contains("noisy"), "20% spread must be called out");
        assert!(w.contains("64 KiB"), "large page size noted");
        assert!(w.contains("SHA512") && w.contains("SVE"), "missing ISA features listed");
    }

    #[test]
    fn busy_machine_and_emulated_numa_are_reported_correctly() {
        let mut r = sample_report();
        r.system.load_avg = Some([3.7, 3.4, 3.3]);
        r.system.logical_cpus = 4;
        r.system.numa = (0..8).map(|id| NumaNode { id, cpus: "0-3".into(), mem_bytes: None }).collect();
        let w = analyze(&r).join("\n");
        assert!(w.contains("not idle") && w.contains("3.70"), "{w}");
        assert!(w.contains("emulated nodes"), "{w}");
        assert!(!w.contains("different CPU sets"), "fake NUMA must not tell the user to pin per node");
        r.system.numa = vec![
            NumaNode { id: 0, cpus: "0-31".into(), mem_bytes: None },
            NumaNode { id: 1, cpus: "32-63".into(), mem_bytes: None },
        ];
        r.system.load_avg = Some([0.1, 0.1, 0.1]);
        let w = analyze(&r).join("\n");
        assert!(w.contains("2 NUMA nodes with different CPU sets") && !w.contains("not idle"), "{w}");
    }

    #[test]
    fn noisy_measurements_collapse_into_one_last_finding() {
        let r = sample_report(); // int.a MT has a 20% spread
        let w = analyze(&r);
        assert_eq!(w.iter().filter(|x| x.contains("noisy")).count(), 1);
        assert!(w.last().unwrap().contains("int.a MT"), "noisy summary should come last");
    }

    #[test]
    fn musl_lock_contention_is_called_out_for_malloc_heavy_kernels() {
        let mut r = sample_report();
        r.build.libc = "musl".into();
        r.results[0].id = "data.json".into();
        r.results[0].libc_sensitive = true;
        let w = analyze(&r).join("\n");
        assert!(w.contains("global lock") && w.contains("results of data.json partly"), "{w}");
        assert!(!w.contains("data.alloc"), "single-thread-only kernel must not be listed as multi-thread");
        r.build.libc = "glibc".into();
        assert!(!analyze(&r).join("\n").contains("global lock"));
    }

    #[test]
    fn doc_contains_efficiency_and_checksums() {
        let mut r = sample_report();
        r.warnings = analyze(&r);
        let doc = build_doc(&r);
        let text = super::super::render::text(&doc);
        assert!(text.contains("7.0x"), "speed-up 700/100");
        assert!(text.contains("88%"), "efficiency 7.0/8");
        assert!(text.contains("0000000000000001"));
        assert!(text.contains("Findings"));
    }
}
