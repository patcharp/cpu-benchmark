//! Hardware / OS inspection. Everything is best effort: a field that cannot be
//! determined stays empty/zero/None and the report prints "unknown" rather
//! than guessing. Parsers take plain text so they can be unit-tested with
//! captured /proc and /sys contents (including ARM, where /proc/cpuinfo has no
//! "model name").

use crate::model::{CacheInfo, NumaNode, SysInfo};
use crate::sys;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::process::Command;

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[derive(Debug, Default, PartialEq)]
pub struct CpuInfo {
    pub model_name: Option<String>,
    pub vendor: Option<String>,
    pub implementer: Option<u32>,
    pub part: Option<u32>,
    pub flags: Vec<String>,
    pub mhz: Option<f64>,
}

fn parse_hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

/// Parse the first processor block of /proc/cpuinfo (x86 and ARM layouts).
pub fn parse_cpuinfo(text: &str) -> CpuInfo {
    let mut ci = CpuInfo::default();
    for line in text.lines() {
        if line.trim().is_empty() && (ci.model_name.is_some() || ci.implementer.is_some()) {
            break; // end of first processor block
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "model name" => ci.model_name = Some(v.to_string()),
            "vendor_id" => ci.vendor = Some(v.to_string()),
            "flags" | "Features" => {
                ci.flags = v.split_whitespace().map(str::to_string).collect()
            }
            "CPU implementer" => ci.implementer = parse_hex(v),
            "CPU part" => ci.part = parse_hex(v),
            "cpu MHz" => ci.mhz = v.parse().ok(),
            _ => {}
        }
    }
    ci
}

pub fn arm_implementer(id: u32) -> Option<&'static str> {
    Some(match id {
        0x41 => "ARM",
        0x42 => "Broadcom",
        0x43 => "Cavium",
        0x46 => "Fujitsu",
        0x48 => "HiSilicon",
        0x50 => "Applied Micro",
        0x51 => "Qualcomm",
        0x61 => "Apple",
        0x6d => "Microsoft",
        0xc0 => "Ampere",
        _ => return None,
    })
}

pub fn arm_core(implementer: u32, part: u32) -> Option<&'static str> {
    Some(match (implementer, part) {
        (0x41, 0xd0c) => "Neoverse N1",
        (0x41, 0xd40) => "Neoverse V1",
        (0x41, 0xd49) => "Neoverse N2",
        (0x41, 0xd4f) => "Neoverse V2",
        (0x41, 0xd0b) => "Cortex-A76",
        (0x41, 0xd08) => "Cortex-A72",
        (0x48, 0xd01) => "TaiShan v110 (Kunpeng 920)",
        (0x48, 0xd02) => "HiSilicon HIP09",
        (0xc0, 0xac3) => "Ampere-1",
        (0xc0, 0xac4) => "Ampere-1a",
        _ => return None,
    })
}

/// "32K" / "1024K" / "2M" / "1G" (sysfs cache sizes) to bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last()? {
        'K' | 'k' => (&s[..s.len() - 1], 1u64 << 10),
        'M' | 'm' => (&s[..s.len() - 1], 1 << 20),
        'G' | 'g' => (&s[..s.len() - 1], 1 << 30),
        _ => (s, 1),
    };
    num.trim().parse::<u64>().ok().map(|n| n * mult)
}

/// First three fields of /proc/loadavg.
pub fn parse_loadavg(text: &str) -> Option<[f64; 3]> {
    let mut it = text.split_whitespace().map(|v| v.parse::<f64>().ok());
    Some([it.next()??, it.next()??, it.next()??])
}

/// "MemTotal:  1234 kB" style value to bytes.
pub fn parse_meminfo_kb(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let rest = l.split_once(key)?.1.trim_start_matches(':').trim();
        rest.trim_end_matches("kB").trim().parse::<u64>().ok().map(|kb| kb * 1024)
    })
}

/// Selected value of "always [madvise] never".
pub fn parse_thp(s: &str) -> Option<String> {
    let a = s.find('[')?;
    let b = s[a..].find(']')? + a;
    Some(s[a + 1..b].to_string())
}

/// cgroup v2 `cpu.max` ("max 100000" or "200000 100000") to a CPU count limit.
pub fn parse_cpu_max(s: &str) -> Option<f64> {
    let mut it = s.split_whitespace();
    let (quota, period) = (it.next()?, it.next()?);
    if quota == "max" {
        return None;
    }
    let (q, p): (f64, f64) = (quota.parse().ok()?, period.parse().ok()?);
    (p > 0.0).then(|| q / p)
}

fn cgroup_cpu_limit() -> Option<f64> {
    if let Some(v) = read("/sys/fs/cgroup/cpu.max").and_then(|s| parse_cpu_max(&s)) {
        return Some(v);
    }
    let q: f64 = read("/sys/fs/cgroup/cpu/cpu.cfs_quota_us")?.parse().ok()?;
    let p: f64 = read("/sys/fs/cgroup/cpu/cpu.cfs_period_us")?.parse().ok()?;
    (q > 0.0 && p > 0.0).then(|| q / p)
}

fn virtualization(flags: &[String], machine: &str) -> Option<String> {
    let mut found = Vec::new();
    if flags.iter().any(|f| f == "hypervisor") {
        found.push("hypervisor flag set".to_string());
    }
    let m = machine.to_lowercase();
    for kw in ["kvm", "vmware", "virtualbox", "qemu", "xen", "amazon ec2", "openstack", "hyper-v", "virtual machine"] {
        if m.contains(kw) {
            found.push(format!("DMI says '{kw}'"));
            break;
        }
    }
    if fs::metadata("/.dockerenv").is_ok() {
        found.push("container (/.dockerenv)".into());
    }
    (!found.is_empty()).then(|| found.join(", "))
}

fn cpu_dir(i: usize, rest: &str) -> String {
    format!("/sys/devices/system/cpu/cpu{i}/{rest}")
}

/// Topology from sysfs: (sockets, physical cores, logical cpus counted).
fn topology(online: &[usize]) -> (usize, usize) {
    let mut cores = BTreeSet::new();
    let mut pkgs = BTreeSet::new();
    for &i in online {
        let pkg = read(&cpu_dir(i, "topology/physical_package_id")).and_then(|s| s.parse::<i64>().ok());
        let core = read(&cpu_dir(i, "topology/core_id")).and_then(|s| s.parse::<i64>().ok());
        let (Some(pkg), Some(core)) = (pkg, core) else { return (0, 0) };
        pkgs.insert(pkg);
        cores.insert((pkg, core));
    }
    (pkgs.len(), cores.len())
}

/// Distinct physical cores among `cpus`; `None` if topology is not exposed.
pub fn core_count_of(cpus: &[usize]) -> Option<usize> {
    let mut cores = BTreeSet::new();
    for &i in cpus {
        let pkg = read(&cpu_dir(i, "topology/physical_package_id"))?.parse::<i64>().ok()?;
        let core = read(&cpu_dir(i, "topology/core_id"))?.parse::<i64>().ok()?;
        cores.insert((pkg, core));
    }
    (!cores.is_empty()).then_some(cores.len())
}

fn caches(online: &[usize]) -> Vec<CacheInfo> {
    // (level, type, shared_cpu_list) identifies one physical cache instance.
    let mut seen: BTreeMap<(u32, String, String), u64> = BTreeMap::new();
    for &i in online {
        for idx in 0..8 {
            let base = cpu_dir(i, &format!("cache/index{idx}"));
            let (Some(level), Some(kind), Some(size)) = (
                read(&format!("{base}/level")).and_then(|s| s.parse::<u32>().ok()),
                read(&format!("{base}/type")),
                read(&format!("{base}/size")).and_then(|s| parse_size(&s)),
            ) else {
                continue;
            };
            let shared = read(&format!("{base}/shared_cpu_list")).unwrap_or_else(|| i.to_string());
            seen.insert((level, kind, shared), size);
        }
    }
    group_caches(seen)
}

fn group_caches(seen: BTreeMap<(u32, String, String), u64>) -> Vec<CacheInfo> {
    let mut grouped: BTreeMap<(u32, String, u64), u32> = BTreeMap::new();
    for ((level, kind, _), size) in seen {
        *grouped.entry((level, kind, size)).or_default() += 1;
    }
    grouped
        .into_iter()
        .map(|((level, kind, size_bytes), instances)| CacheInfo { level, kind, size_bytes, instances })
        .collect()
}

fn numa_nodes() -> Vec<NumaNode> {
    let Ok(dir) = fs::read_dir("/sys/devices/system/node") else { return Vec::new() };
    let mut nodes: Vec<NumaNode> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let id: u32 = name.strip_prefix("node")?.parse().ok()?;
            let base = format!("/sys/devices/system/node/{name}");
            Some(NumaNode {
                id,
                cpus: read(&format!("{base}/cpulist")).unwrap_or_default(),
                mem_bytes: read(&format!("{base}/meminfo"))
                    .and_then(|t| parse_meminfo_kb(&t, "MemTotal")),
            })
        })
        .collect();
    nodes.sort_by_key(|n| n.id);
    nodes
}

fn lscpu_model_name() -> Option<String> {
    let out = Command::new("lscpu").env("LC_ALL", "C").output().ok()?;
    String::from_utf8(out.stdout).ok()?.lines().find_map(|l| {
        let v = l.strip_prefix("Model name:")?.trim();
        (!v.is_empty() && v != "-").then(|| v.to_string())
    })
}

fn os_pretty_name() -> String {
    read("/etc/os-release")
        .and_then(|t| {
            t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string()))
        })
        .unwrap_or_else(|| std::env::consts::OS.to_string())
}

pub fn collect() -> SysInfo {
    let cpuinfo_text = read("/proc/cpuinfo").unwrap_or_default();
    let ci = parse_cpuinfo(&cpuinfo_text);
    let online: Vec<usize> = read("/sys/devices/system/cpu/online")
        .and_then(|s| sys::parse_cpu_list(&s).ok())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| (0..std::thread::available_parallelism().map_or(1, |n| n.get())).collect());

    let machine = [
        read("/sys/class/dmi/id/sys_vendor"),
        read("/sys/class/dmi/id/product_name"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    let machine = if machine.is_empty() {
        read("/proc/device-tree/model").map(|s| s.replace('\0', "")).unwrap_or_default()
    } else {
        machine
    };

    let decoded = ci.implementer.zip(ci.part).and_then(|(i, p)| arm_core(i, p)).map(str::to_string);
    let raw_arm = ci
        .implementer
        .zip(ci.part)
        .map(|(i, p)| format!("ARM implementer 0x{i:02x} part 0x{p:03x}"));
    let cpu_model = ci
        .model_name
        .clone()
        .or_else(lscpu_model_name)
        .map(|m| match &decoded {
            Some(d) if !m.contains(d.as_str()) => format!("{m} [{d} core]"),
            _ => m,
        })
        .or(decoded)
        .or(raw_arm)
        .unwrap_or_else(|| "unknown".to_string());
    let cpu_vendor = ci
        .vendor
        .clone()
        .or_else(|| ci.implementer.and_then(arm_implementer).map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());

    let (sockets, physical_cores) = topology(&online);
    let logical = online.len();
    let max_khz = read(&cpu_dir(online[0], "cpufreq/cpuinfo_max_freq")).and_then(|s| s.parse::<f64>().ok());

    SysInfo {
        hostname: read("/proc/sys/kernel/hostname").unwrap_or_else(|| "unknown".into()),
        os: os_pretty_name(),
        kernel: read("/proc/sys/kernel/osrelease").unwrap_or_default(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_model,
        cpu_vendor,
        virtualization: virtualization(&ci.flags, &machine),
        machine,
        sockets,
        physical_cores,
        logical_cpus: logical,
        threads_per_core: if physical_cores > 0 { logical / physical_cores } else { 0 },
        usable_parallelism: std::thread::available_parallelism().map_or(1, |n| n.get()),
        numa: numa_nodes(),
        caches: caches(&online),
        mem_total_bytes: read("/proc/meminfo").and_then(|t| parse_meminfo_kb(&t, "MemTotal")).unwrap_or(0),
        page_size: sys::page_size(),
        thp: read("/sys/kernel/mm/transparent_hugepage/enabled").and_then(|s| parse_thp(&s)),
        max_freq_mhz: max_khz.map(|k| k / 1000.0).or(ci.mhz),
        governor: read(&cpu_dir(online[0], "cpufreq/scaling_governor")),
        cgroup_cpu_limit: cgroup_cpu_limit(),
        load_avg: read("/proc/loadavg").and_then(|t| parse_loadavg(&t)),
        features: ci.flags,
    }
}

/// Instruction-set features worth showing in a report (the full list is kept in the JSON).
pub fn interesting_features(arch: &str, all: &[String]) -> Vec<String> {
    let want: &[&str] = match arch {
        "x86_64" => &[
            "sse4_2", "avx", "avx2", "fma", "avx512f", "avx512bw", "avx512vl", "avx512_vnni", "amx_tile",
            "aes", "vaes", "pclmulqdq", "sha_ni",
        ],
        "aarch64" => &[
            "asimd", "fp", "aes", "pmull", "sha1", "sha2", "sha3", "sha512", "crc32", "atomics", "asimddp",
            "asimdhp", "bf16", "i8mm", "sve", "sve2",
        ],
        _ => &[],
    };
    want.iter().filter(|w| all.iter().any(|f| f == *w)).map(|s| s.to_string()).collect()
}

/// Features whose *absence* changes benchmark behaviour; reported as findings.
pub fn missing_feature_notes(arch: &str, all: &[String]) -> Vec<String> {
    let has = |f: &str| all.iter().any(|x| x == f);
    let mut notes = Vec::new();
    match arch {
        "x86_64" => {
            if !has("sha_ni") {
                notes.push("CPU has no SHA-NI: crypto.sha256 runs the software implementation.".to_string());
            }
            if !has("avx2") {
                notes.push("CPU has no AVX2: SIMD-dependent kernels run at SSE2 width.".to_string());
            }
        }
        "aarch64" => {
            if !has("sha2") {
                notes.push("CPU has no ARMv8 SHA2: crypto.sha256 runs the software implementation.".to_string());
            }
            if !has("sha3") && !has("sha512") {
                notes.push("CPU has no ARMv8 SHA512 extension: crypto.sha512 runs the software implementation.".to_string());
            }
            if !has("aes") {
                notes.push("CPU has no ARMv8 AES: crypto.aes256 runs the software implementation.".to_string());
            }
            if !has("sve") {
                notes.push("CPU has no SVE: vector kernels use 128-bit NEON only.".to_string());
            }
        }
        _ => {}
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    const X86: &str = "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Xeon(R) Gold 6248 CPU @ 2.50GHz\ncpu MHz\t\t: 2500.000\nflags\t\t: fpu sse4_2 avx avx2 fma avx512f aes hypervisor\n\nprocessor\t: 1\nmodel name\t: SECOND\n";
    const ARM: &str = "processor\t: 0\nBogoMIPS\t: 200.00\nFeatures\t: fp asimd evtstrm aes pmull sha1 sha2 crc32 atomics fphp asimdhp cpuid asimdrdm jscvt fcma dcpop asimddp asimdfhm ssbs\nCPU implementer\t: 0x48\nCPU architecture: 8\nCPU variant\t: 0x1\nCPU part\t: 0xd01\nCPU revision\t: 0\n\nprocessor\t: 1\nCPU part\t: 0xd02\n";

    #[test]
    fn parses_x86_cpuinfo_first_block_only() {
        let ci = parse_cpuinfo(X86);
        assert_eq!(ci.model_name.as_deref(), Some("Intel(R) Xeon(R) Gold 6248 CPU @ 2.50GHz"));
        assert_eq!(ci.vendor.as_deref(), Some("GenuineIntel"));
        assert_eq!(ci.mhz, Some(2500.0));
        assert!(ci.flags.contains(&"avx512f".to_string()));
    }

    #[test]
    fn parses_kunpeng_cpuinfo_without_model_name() {
        let ci = parse_cpuinfo(ARM);
        assert_eq!(ci.model_name, None);
        assert_eq!((ci.implementer, ci.part), (Some(0x48), Some(0xd01)));
        assert_eq!(arm_implementer(0x48), Some("HiSilicon"));
        assert_eq!(arm_core(0x48, 0xd01), Some("TaiShan v110 (Kunpeng 920)"));
        assert!(ci.flags.contains(&"sha2".to_string()));
        assert!(!ci.flags.contains(&"sha3".to_string()));
    }

    #[test]
    fn missing_features_are_called_out() {
        let ci = parse_cpuinfo(ARM);
        let notes = missing_feature_notes("aarch64", &ci.flags);
        assert!(notes.iter().any(|n| n.contains("SHA512")));
        assert!(notes.iter().any(|n| n.contains("SVE")));
        assert!(!notes.iter().any(|n| n.contains("crypto.aes256")));
        let x86 = parse_cpuinfo(X86);
        assert!(missing_feature_notes("x86_64", &x86.flags).iter().any(|n| n.contains("SHA-NI")));
        let shown = interesting_features("aarch64", &ci.flags);
        assert_eq!(&shown[..3], &["asimd", "fp", "aes"]);
    }

    #[test]
    fn size_and_meminfo_parsing() {
        assert_eq!(parse_size("64K"), Some(65_536));
        assert_eq!(parse_size("32M"), Some(32 << 20));
        assert_eq!(parse_size("1G"), Some(1 << 30));
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_size("x"), None);
        assert_eq!(parse_meminfo_kb("MemTotal:       1071619840 kB\nMemFree: 1 kB", "MemTotal"), Some(1071619840 * 1024));
        assert_eq!(parse_meminfo_kb("Node 0 MemTotal:  2048 kB", "MemTotal"), Some(2048 * 1024));
        assert_eq!(parse_thp("always [madvise] never").as_deref(), Some("madvise"));
        assert_eq!(parse_thp("garbage"), None);
    }

    #[test]
    fn loadavg_parsing() {
        assert_eq!(parse_loadavg("3.69 3.41 3.31 2/345 12345\n"), Some([3.69, 3.41, 3.31]));
        assert_eq!(parse_loadavg("garbage"), None);
        assert_eq!(parse_loadavg("1.0 2.0"), None);
    }

    #[test]
    fn cgroup_limit_parsing() {
        assert_eq!(parse_cpu_max("max 100000"), None);
        assert_eq!(parse_cpu_max("200000 100000"), Some(2.0));
        assert_eq!(parse_cpu_max("50000 100000"), Some(0.5));
    }

    #[test]
    fn cache_instances_are_deduplicated() {
        // 4 CPUs, private 64K L1d each, one 8M L3 shared by all.
        let mut seen = BTreeMap::new();
        for c in 0..4u32 {
            seen.insert((1, "Data".to_string(), c.to_string()), 64 << 10);
            seen.insert((3, "Unified".to_string(), "0-3".to_string()), 8 << 20);
        }
        let caches = group_caches(seen);
        let l1 = caches.iter().find(|c| c.level == 1).unwrap();
        let l3 = caches.iter().find(|c| c.level == 3).unwrap();
        assert_eq!((l1.instances, l1.size_bytes), (4, 64 << 10));
        assert_eq!((l3.instances, l3.size_bytes), (1, 8 << 20));
    }

    #[test]
    fn collect_does_not_panic_here() {
        let s = collect();
        assert!(s.logical_cpus >= 1);
        assert!(!s.arch.is_empty());
        assert!(s.page_size >= 4096);
    }
}
