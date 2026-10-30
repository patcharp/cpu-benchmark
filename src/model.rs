//! Serializable report model. This is the on-disk JSON format; bump `SCHEMA`
//! when changing it incompatibly.

use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Report {
    pub schema: u32,
    pub tool_version: String,
    pub label: String,
    pub started_unix: u64,
    pub elapsed_secs: f64,
    pub profile: String,
    pub config: RunConfig,
    pub system: SysInfo,
    pub build: BuildInfo,
    pub results: Vec<KernelResult>,
    pub scores: Scores,
    pub warnings: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RunConfig {
    pub threads: usize,
    pub pinned: bool,
    pub cpus: Vec<usize>,
    /// Physical cores the multi-thread runs actually occupied (0 = unknown).
    #[serde(default)]
    pub cores_used: usize,
    pub warmup_secs: f64,
    pub sample_secs: f64,
    pub samples: usize,
    pub mem_budget_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct BuildInfo {
    pub target: String,
    pub rustflags: String,
    pub profile: String,
    pub opt_level: String,
    pub rustc: String,
    pub libc: String,
    pub debug_assertions: bool,
    /// SIMD / crypto target features enabled at compile time.
    pub compiled_features: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CacheInfo {
    pub level: u32,
    pub kind: String,
    pub size_bytes: u64,
    /// Number of distinct instances of this cache in the machine.
    pub instances: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct NumaNode {
    pub id: u32,
    pub cpus: String,
    pub mem_bytes: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SysInfo {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub cpu_model: String,
    pub cpu_vendor: String,
    pub machine: String,
    pub sockets: usize,
    pub physical_cores: usize,
    pub logical_cpus: usize,
    pub threads_per_core: usize,
    pub usable_parallelism: usize,
    pub numa: Vec<NumaNode>,
    pub caches: Vec<CacheInfo>,
    pub mem_total_bytes: u64,
    pub page_size: u64,
    pub thp: Option<String>,
    pub max_freq_mhz: Option<f64>,
    pub governor: Option<String>,
    pub virtualization: Option<String>,
    pub cgroup_cpu_limit: Option<f64>,
    /// 1/5/15-minute load average when the run started.
    #[serde(default)]
    pub load_avg: Option<[f64; 3]>,
    pub features: Vec<String>,
}

impl SysInfo {
    /// Total capacity of the last-level cache across the whole machine.
    pub fn llc_total_bytes(&self) -> u64 {
        let top = self
            .caches
            .iter()
            .filter(|c| c.kind != "Instruction")
            .map(|c| c.level)
            .max();
        match top {
            Some(l) => self
                .caches
                .iter()
                .filter(|c| c.level == l && c.kind != "Instruction")
                .map(|c| c.size_bytes * c.instances as u64)
                .sum(),
            None => 0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Measurement {
    pub threads: usize,
    pub median: f64,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub stddev: f64,
    pub cv_pct: f64,
    pub samples: Vec<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct KernelResult {
    pub id: String,
    pub category: String,
    pub description: String,
    pub unit: String,
    pub higher_is_better: bool,
    /// "ok", "skipped", "unsupported" or "failed". A kernel is never dropped
    /// from a report silently: anything not measured says why in `note`.
    pub status: String,
    pub note: Option<String>,
    /// Result depends on libc's allocator/memcpy, not only on the CPU.
    #[serde(default)]
    pub libc_sensitive: bool,
    /// Implementation actually used at runtime, e.g. "hardware (aes)" or
    /// "software fallback". `None` when the kernel has a single code path.
    pub backend: Option<String>,
    pub single: Option<Measurement>,
    pub multi: Option<Measurement>,
    /// Deterministic output digest; must be identical on every architecture.
    pub checksum: String,
    pub checksum_consistent: bool,
}

pub const STATUS_OK: &str = "ok";
pub const STATUS_SKIPPED: &str = "skipped";
pub const STATUS_UNSUPPORTED: &str = "unsupported";
pub const STATUS_FAILED: &str = "failed";

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CategoryScore {
    pub name: String,
    pub single: Option<f64>,
    pub multi: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Scores {
    pub single: Option<f64>,
    pub multi: Option<f64>,
    pub categories: Vec<CategoryScore>,
}
