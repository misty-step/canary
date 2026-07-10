//! Stamps the crate's compiled `CANARY_VERSION` from the environment.
//!
//! A release image build resolves the reviewed commit's `git describe` output
//! and threads it through Docker as `--build-arg CANARY_VERSION=...`. The
//! Dockerfile exports it as an `ENV` before `cargo build`, so it lands here as
//! a process environment variable. Local builds and the strict-gate image
//! smoke leave it unset and get the `0.0.0-dev` fallback.

fn main() {
    let version = std::env::var("CANARY_VERSION").unwrap_or_else(|_| "0.0.0-dev".to_string());
    println!("cargo:rustc-env=CANARY_VERSION={version}");
    println!("cargo:rerun-if-env-changed=CANARY_VERSION");
}
