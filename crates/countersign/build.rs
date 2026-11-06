//! Builds the mascot icons in `data/icons` into the binary, so the app
//! shows them before it is installed, in the demo and in screenshots, and
//! works out the version the app reports.

use std::process::Command;

fn main() {
    glib_build_tools::compile_resources(
        &["../../data/icons"],
        "../../data/countersign.gresource.xml",
        "countersign.gresource",
    );

    println!("cargo:rerun-if-env-changed=COUNTERSIGN_RELEASE");
    // Git writes to these on every commit, checkout and new tag.
    for path in ["logs/HEAD", "refs/tags"] {
        if let Some(path) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rustc-env=COUNTERSIGN_VERSION={}", version());
}

/// The release workflow sets `COUNTERSIGN_RELEASE` to the tag it builds, and
/// only those binaries report a plain version. Every other build says it is
/// a dev build, and how far past the last release when git can tell.
fn version() -> String {
    if let Some(tag) = std::env::var("COUNTERSIGN_RELEASE").ok().filter(|tag| !tag.is_empty()) {
        return tag.trim_start_matches('v').to_owned();
    }
    let cargo = env!("CARGO_PKG_VERSION");
    match dev_build() {
        Some(build) => format!("{cargo}+dev.{build}"),
        None => format!("{cargo}+dev"),
    }
}

/// Commits since the last release tag and the commit, such as `2.30530ac`,
/// or only the commit in a clone without tags.
fn dev_build() -> Option<String> {
    let Some(described) = git(&["describe", "--tags", "--match", "v*", "--long", "--abbrev=7"]) else {
        return git(&["rev-parse", "--short=7", "HEAD"]);
    };
    let mut parts = described.rsplitn(3, '-');
    let commit = parts.next()?.trim_start_matches('g');
    let commits = parts.next()?;
    Some(format!("{commits}.{commit}"))
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
