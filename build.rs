use std::{fs, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Ok(head) = fs::read_to_string(".git/HEAD")
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        println!("cargo:rerun-if-changed=.git/{reference}");
    }

    let commit = std::env::var("GIT_COMMIT").ok().or_else(|| {
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|commit| commit.trim().to_string())
    });
    println!(
        "cargo:rustc-env=GIT_COMMIT={}",
        commit
            .filter(|commit| !commit.is_empty())
            .as_deref()
            .unwrap_or("unknown")
    );
}
