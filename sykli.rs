//! Sykli CI definition for Kulta
//!
//! Run with: cargo run --manifest-path /path/to/sykli/sdk/rust/Cargo.toml -- --emit

use sykli::Pipeline;

fn main() {
    let mut p = Pipeline::new();

    // Test
    let _ = p
        .task("test")
        .run("cargo test")
        .inputs(&["**/*.rs", "Cargo.toml", "Cargo.lock"]);

    // Lint
    let _ = p.task("lint").run("cargo clippy -- -D warnings").inputs(&[
        "**/*.rs",
        "Cargo.toml",
        "Cargo.lock",
    ]);

    // Build (depends on test and lint)
    let _ = p
        .task("build")
        .run("cargo build --release")
        .inputs(&["**/*.rs", "Cargo.toml", "Cargo.lock"])
        .output("binary", "target/release/kulta")
        .after(&["test", "lint"]);

    // Integration tests with seppo (requires kind cluster)
    // Run manually: KULTA_RUN_SEPPO_TESTS=1 cargo test --test seppo_integration_test -- --ignored
    let _ = p
        .task("integration-test")
        .run("cargo test --test seppo_integration_test -- --ignored")
        .env("KULTA_RUN_SEPPO_TESTS", "1")
        .inputs(&["tests/seppo_integration_test.rs", "**/*.rs", "Cargo.toml"]);

    // Stress tests (requires kind cluster with resources)
    // Run manually: KULTA_RUN_STRESS_TESTS=1 cargo test --test stress_test -- --ignored --nocapture
    let _ = p
        .task("stress-test")
        .run("cargo test --test stress_test -- --ignored --nocapture")
        .env("KULTA_RUN_STRESS_TESTS", "1")
        .inputs(&["tests/stress_test.rs", "**/*.rs", "Cargo.toml"]);

    p.emit();
}
