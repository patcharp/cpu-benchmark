//! Strict validation of everything that comes from the web form. The server
//! never builds a shell command: it passes a fixed list of arguments to the
//! `cpubench run` child, and every value is checked here first.

use crate::kernels;
use crate::stats::fmt_utc;
use crate::sys::parse_cpu_list;
use serde::Deserialize;

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct RunRequest {
    pub label: Option<String>,
    pub profile: String,
    pub mode: String,
    pub threads: Option<usize>,
    pub cpus: Option<String>,
    pub pin: bool,
    pub only: Option<String>,
    pub skip: Option<String>,
    pub mem_budget_mb: Option<u64>,
}

/// Machine facts the validation needs (so it stays a pure function).
pub struct Env<'a> {
    pub usable_parallelism: usize,
    pub allowed_cpus: &'a [usize],
    pub hostname: &'a str,
    pub now_unix: u64,
    /// Whether a report with this label already exists in the output dir.
    pub label_exists: &'a dyn Fn(&str) -> bool,
}

#[derive(Debug, PartialEq)]
pub struct Validated {
    pub label: String,
    /// Arguments after `run` (excluding `--out`, which the server alone decides).
    pub args: Vec<String>,
}

pub fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
}

/// A report label / file stem that cannot escape the output directory.
pub fn safe_label(s: &str) -> Option<&str> {
    let ok = !s.is_empty()
        && s.len() <= 120
        && s.chars().all(is_name_char)
        && !s.starts_with('.')
        && !s.contains("..");
    ok.then_some(s)
}

/// `<stem>.<ext>` served from /reports/; only html, json and md, never partial files.
pub fn safe_report_file(name: &str) -> Option<(&str, &str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    if !["html", "json", "md"].contains(&ext) || stem.ends_with(".partial") {
        return None;
    }
    safe_label(stem).map(|s| (s, ext))
}

fn compact_ts(secs: u64) -> String {
    fmt_utc(secs).replace(['-', ':'], "").replace(' ', "-")
}

/// Comma-separated kernel selectors; each must match at least one real kernel.
fn clean_list(what: &str, raw: &str) -> Result<Option<String>, String> {
    let tokens: Vec<&str> = raw.split(',').map(str::trim).filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return Ok(None);
    }
    if tokens.len() > 24 {
        return Err(format!("{what}: too many entries"));
    }
    let ids: Vec<&str> = kernels::all().iter().map(|k| k.id).collect();
    for t in &tokens {
        if t.len() > 40 || !t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_') {
            return Err(format!("{what}: '{t}' is not a valid kernel selector"));
        }
        if !ids.iter().any(|id| id.contains(t)) {
            return Err(format!("{what}: no kernel id contains '{t}' (see the kernel list)"));
        }
    }
    Ok(Some(tokens.join(",")))
}

pub fn validate_run(req: &RunRequest, env: &Env) -> Result<Validated, String> {
    let profile = if req.profile.is_empty() { "full" } else { req.profile.as_str() };
    if !["quick", "standard", "full"].contains(&profile) {
        return Err(format!("profile must be quick, standard or full (got '{profile}')"));
    }
    let mode = if req.mode.is_empty() { "both" } else { req.mode.as_str() };
    if !["both", "single", "multi"].contains(&mode) {
        return Err(format!("mode must be both, single or multi (got '{mode}')"));
    }

    let base = req.label.as_deref().map(str::trim).filter(|l| !l.is_empty()).unwrap_or(env.hostname);
    let mut label = crate::sanitize(base);
    if safe_label(&label).is_none() {
        label = "cpubench".into();
    }
    if (env.label_exists)(&label) {
        label = format!("{label}-{}", compact_ts(env.now_unix));
    }

    let mut args: Vec<String> = ["run", "--label", &label, "--profile", profile, "--mode", mode].map(String::from).to_vec();

    if let Some(t) = req.threads {
        if t == 0 || t > env.usable_parallelism {
            return Err(format!("threads must be between 1 and {} (usable CPUs)", env.usable_parallelism));
        }
        args.extend(["--threads".into(), t.to_string()]);
    }
    if let Some(spec) = req.cpus.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let list = parse_cpu_list(spec)?;
        if list.is_empty() {
            return Err("cpus: empty list".into());
        }
        if let Some(bad) = list.iter().find(|c| !env.allowed_cpus.contains(c)) {
            return Err(format!("cpus: CPU {bad} is not available to this process"));
        }
        args.extend(["--cpus".into(), spec.to_string()]);
    } else if req.pin {
        args.push("--pin".into());
    }
    for (flag, what, raw) in [("--only", "only", &req.only), ("--skip", "skip", &req.skip)] {
        if let Some(list) = raw.as_deref().map(|r| clean_list(what, r)).transpose()?.flatten() {
            args.extend([flag.into(), list]);
        }
    }
    if let Some(mb) = req.mem_budget_mb {
        if !(16..=262_144).contains(&mb) {
            return Err("mem_budget_mb must be between 16 and 262144".into());
        }
        args.extend(["--mem-budget-mb".into(), mb.to_string()]);
    }
    Ok(Validated { label, args })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(exists: &'a dyn Fn(&str) -> bool, allowed: &'a [usize]) -> Env<'a> {
        Env { usable_parallelism: 4, allowed_cpus: allowed, hostname: "box", now_unix: 1_782_000_000, label_exists: exists }
    }

    fn req(json: &str) -> RunRequest {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn defaults_to_full_profile_and_hostname_label() {
        let v = validate_run(&req("{}"), &env(&|_| false, &[0, 1, 2, 3])).unwrap();
        assert_eq!(v.label, "box");
        assert_eq!(v.args, ["run", "--label", "box", "--profile", "full", "--mode", "both"]);
    }

    #[test]
    fn existing_label_gets_a_timestamp_suffix() {
        let v = validate_run(&req(r#"{"label":"kunpeng"}"#), &env(&|l| l == "kunpeng", &[0])).unwrap();
        assert_eq!(v.label, "kunpeng-20260621-000000");
    }

    #[test]
    fn hostile_label_is_neutralised() {
        let v = validate_run(&req(r#"{"label":"../../etc/passwd; rm -rf /"}"#), &env(&|_| false, &[0])).unwrap();
        assert!(safe_label(&v.label).is_some(), "{}", v.label);
        assert!(!v.label.contains('/') && !v.label.contains(' '));
    }

    #[test]
    fn rejects_bad_enums_threads_and_cpus() {
        let e = env(&|_| false, &[0, 1, 2, 3]);
        assert!(validate_run(&req(r#"{"profile":"turbo"}"#), &e).unwrap_err().contains("profile"));
        assert!(validate_run(&req(r#"{"mode":"x"}"#), &e).unwrap_err().contains("mode"));
        assert!(validate_run(&req(r#"{"threads":99}"#), &e).unwrap_err().contains("between 1 and 4"));
        assert!(validate_run(&req(r#"{"threads":0}"#), &e).is_err());
        assert!(validate_run(&req(r#"{"cpus":"0-9"}"#), &e).unwrap_err().contains("not available"));
        assert!(validate_run(&req(r#"{"cpus":"zzz"}"#), &e).is_err());
        assert!(validate_run(&req(r#"{"mem_budget_mb":1}"#), &e).is_err());
    }

    #[test]
    fn kernel_selectors_must_match_real_kernels() {
        let e = env(&|_| false, &[0]);
        let ok = validate_run(&req(r#"{"only":"int.sort, crypto.aes256","skip":"mem.lat"}"#), &e).unwrap();
        assert!(ok.args.windows(2).any(|w| w == ["--only", "int.sort,crypto.aes256"]));
        assert!(ok.args.windows(2).any(|w| w == ["--skip", "mem.lat"]));
        assert!(validate_run(&req(r#"{"only":"int.sorrt"}"#), &e).unwrap_err().contains("no kernel"));
        assert!(validate_run(&req(r#"{"only":"$(reboot)"}"#), &e).unwrap_err().contains("not a valid"));
    }

    #[test]
    fn cpus_wins_over_pin_and_args_never_contain_out() {
        let v = validate_run(&req(r#"{"cpus":"0-1","pin":true}"#), &env(&|_| false, &[0, 1])).unwrap();
        assert!(v.args.contains(&"--cpus".to_string()) && !v.args.contains(&"--pin".to_string()));
        assert!(!v.args.iter().any(|a| a == "--out"));
    }

    #[test]
    fn report_file_whitelist() {
        assert_eq!(safe_report_file("kunpeng.html"), Some(("kunpeng", "html")));
        assert_eq!(safe_report_file("compare-a-vs-b.md"), Some(("compare-a-vs-b", "md")));
        for bad in ["../x.json", "a/b.json", "..json", ".hidden.json", "x.exe", "x.partial.json", "x", "", "a..b.html", "x.json%00.html", "a b.json"] {
            assert_eq!(safe_report_file(bad), None, "{bad}");
        }
    }
}
