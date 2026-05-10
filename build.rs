use std::process::Command;

fn main() {
    // Embed the short git SHA at build time so the About dialog can show
    // exactly which commit a binary was built from. Resolution order:
    //   1. `GIT_SHA` env var (CI sets this so packaging builds don't
    //      need .git/ inside the build tree at all).
    //   2. `git rev-parse --short HEAD` if available.
    //   3. literal "unknown" — used by tarball builds.
    let sha = std::env::var("GIT_SHA")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| String::from("unknown"));
    println!("cargo:rustc-env=GIT_SHA={sha}");
    // Always emit at least one rerun-if-changed so cargo doesn't fall
    // back to scanning the entire package directory — that scan trips
    // on makepkg's privileged `pkg/` subdir during pacman builds.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=GIT_SHA");
    if std::path::Path::new(".git/HEAD").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
    }
    if std::path::Path::new(".git/index").exists() {
        println!("cargo:rerun-if-changed=.git/index");
    }
}
