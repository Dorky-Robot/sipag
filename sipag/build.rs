fn main() {
    // Capture the short git commit hash at build time so `sipag version`
    // can print it alongside the Cargo version (e.g. "sipag 2.0.0 (944155d)").
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=SIPAG_GIT_SHA={sha}");

    // Re-run when the HEAD changes (new commit or branch switch).
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
}
