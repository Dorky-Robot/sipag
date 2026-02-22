use std::process::Command;

fn main() {
    // Capture the short git commit hash at compile time.
    // Falls back to "unknown" if git is not available (e.g., in release tarballs).
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=CARGO_GIT_SHA={hash}");

    // Re-run if the git HEAD changes (e.g., new commit).
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs");
}
