//! Embeds the git commit as `GIT_SHA` for `/healthz`.
//!
//! Order: the `GIT_SHA` environment variable (set by Docker/CI builds, where
//! `.git` is usually absent), then `git rev-parse --short HEAD`, else
//! `unknown`.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=GIT_SHA");
    // Rebuild when the checked-out commit changes. Paths are relative to this
    // crate; missing files are fine (cargo then reruns on every build only if
    // they appear).
    println!("cargo:rerun-if-changed=../../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../../.git/refs/heads");

    let sha = std::env::var("GIT_SHA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(git_sha)
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=GIT_SHA={}", sha.trim());
}

fn git_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
