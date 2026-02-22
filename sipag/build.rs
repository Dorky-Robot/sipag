fn main() {
    // Capture the short git commit hash at build time for `sipag version`.
    // Falls back to "unknown" when git is unavailable (e.g. CI builds from
    // a tarball or Docker multi-stage builds without the .git directory).
    let git_hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=SIPAG_GIT_HASH={git_hash}");

    // Rerun the build script when the git HEAD pointer changes (new commit
    // or branch switch). We find the actual git dir dynamically so this works
    // for both regular clones and `git worktree add` checkouts.
    let git_dir = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(dir) = git_dir {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/refs/");
    }
}
