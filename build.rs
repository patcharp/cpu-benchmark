//! Captures how the binary was built so that reports from two machines can be
//! checked for comparability (different target-cpu / libc / opt-level make an
//! A/B comparison meaningless).

use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");

    let target = env::var("TARGET").unwrap_or_default();
    let flags = env::var("CARGO_ENCODED_RUSTFLAGS")
        .unwrap_or_default()
        .replace('\u{1f}', " ");
    let profile = env::var("PROFILE").unwrap_or_default();
    let opt_level = env::var("OPT_LEVEL").unwrap_or_default();

    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let rustc_version = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    println!("cargo:rustc-env=CPUBENCH_TARGET={target}");
    println!("cargo:rustc-env=CPUBENCH_RUSTFLAGS={flags}");
    println!("cargo:rustc-env=CPUBENCH_PROFILE={profile}");
    println!("cargo:rustc-env=CPUBENCH_OPT_LEVEL={opt_level}");
    println!("cargo:rustc-env=CPUBENCH_RUSTC={rustc_version}");
}
