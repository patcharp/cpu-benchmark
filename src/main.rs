mod harness;
mod kernels;
mod model;
mod report;
mod score;
mod serve;
mod stats;
mod sys;
mod sysinfo;

use clap::{Args, Parser, Subcommand, ValueEnum};
use harness::Config;
use kernels::{Kernel, Params};
use model::*;
use report::{compare::CompareOpts, render};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "cpubench", version, about = "Cross-architecture CPU benchmark (x86_64 / aarch64) with JSON, Markdown and HTML reports")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the benchmark suite and write a report (default profile: full)
    Run(RunArgs),
    /// Compare two JSON reports (A = baseline, B = candidate)
    Compare(CompareArgs),
    /// Re-render a saved JSON report as Markdown/HTML
    Report(ReportArgs),
    /// Start a web UI to run benchmarks, watch progress, open reports and compare them
    Serve(ServeArgs),
    /// Show detected hardware and OS information
    Info,
    /// List available benchmark kernels
    List,
}

#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum Profile {
    /// ~1 minute; smoke testing only
    Quick,
    /// ~5 minutes
    Standard,
    /// 7 samples x 2 s per measurement; the only profile meant for decisions
    Full,
}

#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum Mode {
    Both,
    Single,
    Multi,
}

#[derive(Args)]
struct RunArgs {
    #[arg(long, value_enum, default_value = "full")]
    profile: Profile,
    /// Name used in reports and file names (default: hostname)
    #[arg(long)]
    label: Option<String>,
    /// Directory for the JSON / Markdown / HTML report files
    #[arg(long, default_value = "cpubench-results")]
    out: PathBuf,
    /// Worker threads for multi-thread runs (default: all usable CPUs, or the size of --cpus)
    #[arg(long)]
    threads: Option<usize>,
    /// Restrict the run to this CPU list (e.g. "0-63" for one socket) and pin threads to it
    #[arg(long)]
    cpus: Option<String>,
    /// Pin worker threads to the CPUs this process may use
    #[arg(long)]
    pin: bool,
    #[arg(long, value_enum, default_value = "both")]
    mode: Mode,
    /// Only run kernels whose id contains one of these comma-separated strings
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Do not run kernels whose id contains one of these strings (they are listed as skipped)
    #[arg(long, value_delimiter = ',')]
    skip: Vec<String>,
    /// Override seconds per timed sample (makes the profile "custom")
    #[arg(long)]
    sample_secs: Option<f64>,
    /// Override number of timed samples (makes the profile "custom")
    #[arg(long)]
    samples: Option<usize>,
    /// Override warm-up seconds (makes the profile "custom")
    #[arg(long)]
    warmup_secs: Option<f64>,
    /// Total memory the streaming kernels may touch, in MiB (default: max(2 GiB, 4x total LLC), at most 25% of RAM)
    #[arg(long)]
    mem_budget_mb: Option<u64>,
}

#[derive(Args)]
struct CompareArgs {
    /// Baseline report (e.g. the existing x86_64 server)
    a: PathBuf,
    /// Candidate report (e.g. the ARM server)
    b: PathBuf,
    #[arg(long, default_value = "cpubench-results")]
    out: PathBuf,
    /// Price of machine A (any currency, same as --cost-b); enables performance-per-cost
    #[arg(long)]
    cost_a: Option<f64>,
    #[arg(long)]
    cost_b: Option<f64>,
    /// Power draw of machine A in watts (same basis as --watts-b); enables performance-per-watt
    #[arg(long)]
    watts_a: Option<f64>,
    #[arg(long)]
    watts_b: Option<f64>,
}

#[derive(Args)]
struct ServeArgs {
    /// Address to listen on. Loopback by default; for access from other machines use an ssh tunnel
    /// (ssh -L 8080:127.0.0.1:8080 user@host) or bind a non-loopback address (then a token is required)
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: String,
    /// Directory where reports are written and listed
    #[arg(long, default_value = "cpubench-results")]
    out: PathBuf,
    /// Access token (min 8 chars). Generated automatically for non-loopback binds
    #[arg(long)]
    token: Option<String>,
    /// Allow a non-loopback bind WITHOUT authentication (anyone reaching the port can start runs)
    #[arg(long)]
    no_auth: bool,
}

#[derive(Args)]
struct ReportArgs {
    file: PathBuf,
    #[arg(long, default_value = "cpubench-results")]
    out: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Run(a) => run(a),
        Cmd::Compare(a) => compare(a),
        Cmd::Report(a) => rerender(a),
        Cmd::Serve(a) => serve::run(serve::Options { bind: a.bind, out: a.out, token: a.token, no_auth: a.no_auth }),
        Cmd::Info => info(),
        Cmd::List => list(),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type Res<T> = Result<T, String>;

fn io<T>(r: std::io::Result<T>, what: &str) -> Res<T> {
    r.map_err(|e| format!("{what}: {e}"))
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn sanitize(label: &str) -> String {
    let s: String = label.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    if s.is_empty() { "cpubench".into() } else { s }
}

fn build_info() -> BuildInfo {
    #[allow(unused_mut)]
    let mut feats: Vec<&str> = Vec::new();
    macro_rules! feature {
        ($($name:literal),*) => { $( if cfg!(target_feature = $name) { feats.push($name); } )* };
    }
    #[cfg(target_arch = "x86_64")]
    feature!("sse4.2", "avx", "avx2", "fma", "avx512f", "aes", "sha");
    #[cfg(target_arch = "aarch64")]
    feature!("neon", "aes", "sha2", "sha3", "sve", "lse", "crc", "dotprod");
    BuildInfo {
        target: env!("CPUBENCH_TARGET").into(),
        rustflags: env!("CPUBENCH_RUSTFLAGS").into(),
        profile: env!("CPUBENCH_PROFILE").into(),
        opt_level: env!("CPUBENCH_OPT_LEVEL").into(),
        rustc: env!("CPUBENCH_RUSTC").into(),
        libc: if cfg!(target_env = "musl") { "musl".into() } else if cfg!(target_env = "gnu") { "glibc".into() } else { std::env::consts::OS.into() },
        debug_assertions: cfg!(debug_assertions),
        compiled_features: feats.into_iter().map(String::from).collect(),
    }
}

fn selected(k: &Kernel, a: &RunArgs) -> Result<(), String> {
    if !a.only.is_empty() && !a.only.iter().any(|p| k.id.contains(p.as_str())) {
        return Err("not selected by --only".into());
    }
    if let Some(p) = a.skip.iter().find(|p| k.id.contains(p.as_str())) {
        return Err(format!("excluded by --skip {p}"));
    }
    if k.heavy && a.profile == Profile::Quick {
        return Err("heavy kernel; not run in the quick profile".into());
    }
    Ok(())
}

fn result_stub(k: &Kernel, status: &str, note: Option<String>) -> KernelResult {
    KernelResult {
        id: k.id.into(),
        category: k.category.into(),
        description: k.desc.into(),
        unit: k.unit.into(),
        higher_is_better: k.higher_is_better(),
        status: status.into(),
        note,
        libc_sensitive: kernels::libc_sensitive(k.id),
        backend: (k.backend)(),
        single: None,
        multi: None,
        checksum: String::new(),
        checksum_consistent: true,
    }
}

fn describe(m: &Option<Measurement>) -> String {
    m.as_ref().map_or("-".into(), |m| format!("{} (cv {:.1}%)", report::fmt_num(m.median), m.cv_pct))
}

fn write_text(path: &Path, content: &str) -> Res<()> {
    io(fs::write(path, content), &format!("writing {}", path.display()))
}

fn write_all_formats(dir: &Path, stem: &str, doc: &report::Doc) -> Res<Vec<PathBuf>> {
    io(fs::create_dir_all(dir), &format!("creating {}", dir.display()))?;
    let files = [("md", render::markdown(doc)), ("html", render::html(doc))];
    let mut paths = Vec::new();
    for (ext, content) in files {
        let p = dir.join(format!("{stem}.{ext}"));
        write_text(&p, &content)?;
        paths.push(p);
    }
    Ok(paths)
}

fn run(a: RunArgs) -> Res<()> {
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build - numbers are meaningless; use `cargo build --release`");
    }
    let started = now_unix();
    let t_start = Instant::now();
    let system = sysinfo::collect();
    let label = sanitize(&a.label.clone().unwrap_or_else(|| system.hostname.clone()));

    // --- CPU set and thread count
    let allowed = {
        let v = sys::allowed_cpus();
        if v.is_empty() { (0..system.usable_parallelism).collect() } else { v }
    };
    let mut warnings_extra: Vec<String> = Vec::new();
    let (cpus, pinned) = match &a.cpus {
        Some(spec) => {
            let list = sys::parse_cpu_list(spec)?;
            if list.is_empty() {
                return Err("--cpus is empty".into());
            }
            if let Some(bad) = list.iter().find(|c| !allowed.contains(c)) {
                return Err(format!("--cpus: CPU {bad} is not available to this process"));
            }
            (list, true)
        }
        None if a.pin => (allowed.clone(), true),
        None => (Vec::new(), false),
    };
    let threads = a.threads.unwrap_or(if pinned { cpus.len() } else { system.usable_parallelism }).max(1);
    // Physical cores that the multi-thread runs occupy (for per-core comparisons).
    let cores_used = if pinned {
        let used: Vec<usize> = (0..threads).map(|t| cpus[t % cpus.len()]).collect();
        sysinfo::core_count_of(&used).unwrap_or(0)
    } else if system.physical_cores > 0 {
        threads.min(system.physical_cores)
    } else {
        0
    };
    if pinned && threads > cpus.len() {
        warnings_extra.push(format!("{threads} threads requested but only {} CPUs in the pin set: threads share CPUs.", cpus.len()));
    }
    if threads == 1 && a.mode != Mode::Single {
        warnings_extra.push("Only one thread: multi-thread results are omitted (they would equal the single-thread run).".into());
    }

    // --- profile
    let (name, (warm, sample, n)) = match a.profile {
        Profile::Quick => ("quick", (0.25, 0.4, 3)),
        Profile::Standard => ("standard", (0.5, 1.0, 5)),
        Profile::Full => ("full", (1.0, 2.0, 7)),
    };
    let custom = a.sample_secs.is_some() || a.samples.is_some() || a.warmup_secs.is_some();
    let (warm, sample, n) = (a.warmup_secs.unwrap_or(warm), a.sample_secs.unwrap_or(sample), a.samples.unwrap_or(n));
    if n == 0 || sample <= 0.0 || warm < 0.0 {
        return Err("samples must be >= 1 and durations positive".into());
    }
    let profile_name = if custom { "custom" } else { name };
    let cfg = Config {
        warmup: Duration::from_secs_f64(warm),
        sample: Duration::from_secs_f64(sample),
        samples: n,
        pin: pinned.then(|| cpus.clone()),
    };

    let llc = system.llc_total_bytes();
    let default_budget = (2u64 << 30).max(4 * llc);
    let mut budget = a.mem_budget_mb.map_or(default_budget, |m| m << 20);
    if system.mem_total_bytes > 0 && a.mem_budget_mb.is_none() {
        budget = budget.min(system.mem_total_bytes / 4);
    }
    let params = Params { threads, mem_budget: budget, mem_total: system.mem_total_bytes };
    if llc > 0 && budget < 4 * llc {
        warnings_extra.push(format!(
            "Streaming memory budget ({}) is below 4x the total last-level cache ({}); mem.copy/mem.triad may partly measure cache, not DRAM.",
            report::fmt_bytes(budget),
            report::fmt_bytes(llc)
        ));
    }

    eprintln!("cpubench {} | {} | {} | profile {} | {} threads{}", env!("CARGO_PKG_VERSION"), system.cpu_model, system.arch, profile_name, threads, if pinned { " (pinned)" } else { "" });

    // --- run
    let all = kernels::all();
    let total = all.len();
    let mut results: Vec<KernelResult> = Vec::new();
    let mut pin_failures = 0;
    let _ = io(fs::create_dir_all(&a.out), &format!("creating {}", a.out.display()));
    let partial_path = a.out.join(format!("{label}.partial.json"));

    for (i, k) in all.iter().enumerate() {
        let mut res = result_stub(k, STATUS_OK, None);
        if let Err(why) = selected(k, &a) {
            res.status = STATUS_SKIPPED.into();
            res.note = Some(why);
        } else if let Err(why) = (k.check)(&params) {
            res.status = STATUS_UNSUPPORTED.into();
            res.note = Some(why);
        } else {
            if a.mode != Mode::Multi {
                let o = harness::measure(k, 1, &params, &cfg);
                pin_failures += o.pin_failures;
                res.checksum = format!("{:016x}", o.checksum);
                res.checksum_consistent &= o.consistent;
                res.single = Some(stats::summarize(1, &o.values));
            }
            if a.mode != Mode::Single && k.multi && threads > 1 {
                let o = harness::measure(k, threads, &params, &cfg);
                pin_failures += o.pin_failures;
                if res.checksum.is_empty() {
                    res.checksum = format!("{:016x}", o.checksum);
                }
                res.checksum_consistent &= o.consistent;
                res.multi = Some(stats::summarize(threads, &o.values));
            }
            let bad = [&res.single, &res.multi].into_iter().flatten().any(|m| !m.median.is_finite() || m.median <= 0.0);
            if bad {
                res.status = STATUS_FAILED.into();
                res.note = Some("measurement produced a non-positive or non-finite value".into());
            }
        }
        eprintln!(
            "[{:>2}/{total}] {:<16} {:<8} 1T: {:<22} MT: {}",
            i + 1,
            k.id,
            if res.status == STATUS_OK { k.unit.to_string() } else { res.status.to_uppercase() },
            describe(&res.single),
            describe(&res.multi)
        );
        results.push(res);
        // Persist progress so a crash or Ctrl-C still leaves the evidence collected so far.
        // Write-then-rename so a reader (the web UI) never sees half a file.
        let tmp = partial_path.with_extension("json.tmp");
        if fs::write(&tmp, serde_json::to_string(&results).unwrap_or_default()).is_ok() {
            let _ = fs::rename(&tmp, &partial_path);
        }
    }

    if pin_failures > 0 {
        warnings_extra.push(format!("{pin_failures} thread pinning request(s) were refused by the OS; placement was left to the scheduler."));
    }

    let mut report = Report {
        schema: SCHEMA,
        tool_version: env!("CARGO_PKG_VERSION").into(),
        label,
        started_unix: started,
        elapsed_secs: t_start.elapsed().as_secs_f64(),
        profile: profile_name.into(),
        config: RunConfig {
            threads,
            pinned,
            cpus,
            cores_used,
            warmup_secs: warm,
            sample_secs: sample,
            samples: n,
            mem_budget_bytes: budget,
        },
        system,
        build: build_info(),
        scores: score::compute(&results),
        results,
        warnings: Vec::new(),
    };
    report.warnings = report::run::analyze(&report);
    report.warnings.extend(warnings_extra);

    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    let json_path = a.out.join(format!("{}.json", report.label));
    write_text(&json_path, &json)?;
    let _ = fs::remove_file(&partial_path);
    let doc = report::run::build_doc(&report);
    let mut files = vec![json_path];
    files.extend(write_all_formats(&a.out, &report.label, &doc)?);

    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "\n{}", render::text(&doc));
    let _ = writeln!(stdout, "\nReport files:");
    for f in files {
        let _ = writeln!(stdout, "  {}", f.display());
    }
    Ok(())
}

fn load(path: &Path) -> Res<Report> {
    let text = io(fs::read_to_string(path), &format!("reading {}", path.display()))?;
    let r: Report = serde_json::from_str(&text).map_err(|e| format!("{}: not a cpubench report ({e})", path.display()))?;
    if r.schema != SCHEMA {
        return Err(format!("{}: unsupported report schema {} (this tool reads {SCHEMA})", path.display(), r.schema));
    }
    Ok(r)
}

/// Compare two saved reports and write `compare-<a>-vs-<b>.{md,html}` into `out`.
fn compare_files(a: &Path, b: &Path, out: &Path, opts: &CompareOpts) -> Res<(report::Doc, String, Vec<PathBuf>)> {
    let (mut ra, mut rb) = (load(a)?, load(b)?);
    if ra.label == rb.label {
        ra.label = format!("{}-A", ra.label);
        rb.label = format!("{}-B", rb.label);
    }
    let doc = report::compare::build_doc(&ra, &rb, opts);
    let stem = format!("compare-{}-vs-{}", sanitize(&ra.label), sanitize(&rb.label));
    let files = write_all_formats(out, &stem, &doc)?;
    Ok((doc, stem, files))
}

fn compare(a: CompareArgs) -> Res<()> {
    let opts = CompareOpts { cost_a: a.cost_a, cost_b: a.cost_b, watts_a: a.watts_a, watts_b: a.watts_b };
    let (doc, _, files) = compare_files(&a.a, &a.b, &a.out, &opts)?;
    print!("{}", render::text(&doc));
    println!("\nReport files:");
    files.iter().for_each(|f| println!("  {}", f.display()));
    Ok(())
}

fn rerender(a: ReportArgs) -> Res<()> {
    let r = load(&a.file)?;
    let doc = report::run::build_doc(&r);
    let files = write_all_formats(&a.out, &r.label, &doc)?;
    print!("{}", render::text(&doc));
    println!("\nReport files:");
    files.iter().for_each(|f| println!("  {}", f.display()));
    Ok(())
}

fn info() -> Res<()> {
    let s = sysinfo::collect();
    let mut t = report::Table::new(&["Item", "Value"], &[report::Align::Left, report::Align::Left]);
    report::run::system_rows(&s).into_iter().for_each(|(k, v)| t.row([k.to_string(), v]));
    let doc = report::Doc {
        title: "System information".into(),
        subtitle: format!("{} usable CPUs", s.usable_parallelism),
        blocks: vec![
            report::Block::Table(t),
            report::Block::Warnings(sysinfo::missing_feature_notes(&s.arch, &s.features)),
        ],
    };
    print!("{}", render::text(&doc));
    Ok(())
}

fn list() -> Res<()> {
    let mut t = report::Table::new(
        &["Kernel", "Category", "Unit", "Threads", "Description"],
        &[report::Align::Left, report::Align::Left, report::Align::Left, report::Align::Left, report::Align::Left],
    );
    for k in kernels::all() {
        t.row([
            k.id.to_string(),
            k.category.to_string(),
            k.unit.to_string(),
            format!("{}{}", if k.multi { "1 + all" } else { "1 only" }, if k.heavy { " (heavy)" } else { "" }),
            k.desc.to_string(),
        ]);
    }
    print!("{}", render::text(&report::Doc { title: "cpubench kernels".into(), subtitle: String::new(), blocks: vec![report::Block::Table(t)] }));
    Ok(())
}
