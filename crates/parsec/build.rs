//! Bake the version into the binary. `PARSEC_VERSION` in the environment
//! wins (packagers building from a tarball); then `git describe` in a
//! checkout (tag, commits since, short hash, `-dirty`); else the Cargo version.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PARSEC_VERSION");
    let fallback = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let version = std::env::var("PARSEC_VERSION")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["describe", "--tags", "--always", "--dirty"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().trim_start_matches('v').to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(fallback);
    println!("cargo:rustc-env=PARSEC_VERSION={version}");

    // Rebuild when the checkout moves: new commit, new tag, branch switch.
    if let Ok(root) = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .and_then(|o| Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()))
    {
        if !root.is_empty() {
            println!("cargo:rerun-if-changed={root}/HEAD");
            println!("cargo:rerun-if-changed={root}/refs/tags");
            println!("cargo:rerun-if-changed={root}/packed-refs");
        }
    }
}
