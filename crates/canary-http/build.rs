//! Stamps the crate's compiled `CANARY_VERSION` from the environment.
//!
//! The release pipeline (`.github/workflows/deploy.yml`) resolves the
//! deployed commit's `git describe` output on the CI runner (where full tag
//! history is available) and threads it through the Docker build as
//! `--build-arg CANARY_VERSION=...`. The Dockerfile exports it as an `ENV`
//! before `cargo build`, so it lands here as a process environment variable.
//! Builds outside that pipeline (local `cargo build`, the strict-gate image
//! smoke test) leave it unset and get the `0.0.0-dev` fallback.

fn main() {
    let version = std::env::var("CANARY_VERSION").unwrap_or_else(|_| "0.0.0-dev".to_string());
    println!("cargo:rustc-env=CANARY_VERSION={version}");
    println!("cargo:rerun-if-env-changed=CANARY_VERSION");
}
