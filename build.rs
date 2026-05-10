use std::process::Command;

fn main() {
    // Embed the short git SHA at build time so the About dialog can show
    // exactly which commit a binary was built from. Falls back to
    // "unknown" when building from a tarball with no .git/ alongside.
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| String::from("unknown"));
    println!("cargo:rustc-env=GIT_SHA={sha}");
    // Re-run only when *something we name* changes. Without at least one
    // existing rerun-if-changed target, cargo falls back to scanning the
    // whole package directory — which breaks tarball / makepkg builds
    // that drop a privileged `pkg/` directory inside the source tree.
    println!("cargo:rerun-if-changed=build.rs");
    if std::path::Path::new(".git/HEAD").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
    }
    if std::path::Path::new(".git/index").exists() {
        println!("cargo:rerun-if-changed=.git/index");
    }
}
