//! Sykli CI pipeline for Kulta
//!
//! Run locally: sykli run
//! Or: cargo run --bin sykli --features sykli -- --emit | sykli run -

use sykli::{Condition, Pipeline, Template};

fn main() {
    let mut p = Pipeline::new();

    // === RESOURCES ===
    let src = p.dir(".");
    let cargo_registry = p.cache("cargo-registry");
    let cargo_git = p.cache("cargo-git");
    let target_cache = p.cache("target");

    // === TEMPLATE ===
    // Common Rust container configuration
    let rust = Template::new()
        .container("rust:1.85")
        .mount_dir(&src, "/src")
        .mount_cache(&cargo_registry, "/usr/local/cargo/registry")
        .mount_cache(&cargo_git, "/usr/local/cargo/git")
        .mount_cache(&target_cache, "/src/target")
        .workdir("/src");

    // === TASKS ===

    // Test - run all tests
    let _ = p
        .task("test")
        .from(&rust)
        .run("cargo test --all-features")
        .inputs(&["**/*.rs", "Cargo.toml", "Cargo.lock"]);

    // Lint - run clippy with strict warnings
    let _ = p
        .task("lint")
        .from(&rust)
        .run("cargo clippy --all-targets --all-features -- -D warnings")
        .inputs(&["**/*.rs", "Cargo.toml", "Cargo.lock"]);

    // Format check - verify code formatting
    let _ = p
        .task("fmt")
        .from(&rust)
        .run("cargo fmt -- --check")
        .inputs(&["**/*.rs"]);

    // Build release binary (depends on test, lint, fmt)
    let _ = p
        .task("build")
        .from(&rust)
        .run("cargo build --release --bin kulta")
        .inputs(&["**/*.rs", "Cargo.toml", "Cargo.lock"])
        .output("binary", "target/release/kulta")
        .after(&["test", "lint", "fmt"]);

    // Integration tests with kind cluster
    // Only run on push events (not draft PRs) - requires k8s environment
    let _ = p
        .task("integration-test")
        .container("ghcr.io/sykli/kind-runner:latest")
        .mount(&src, "/src")
        .workdir("/src")
        .run(
            r#"#!/bin/bash
set -e

# Create kind cluster
kind create cluster --name kulta-ci

# Install Gateway API CRDs
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.0.0/standard-install.yaml

# Generate and install Kulta CRD
cargo run --bin gen-crd > /tmp/rollout-crd.json
kubectl apply -f /tmp/rollout-crd.json

# Run controller in background
RUST_LOG=info ./target/release/kulta 2>&1 | tee /tmp/kulta-log.txt &
KULTA_PID=$!
sleep 5

# Create test namespace
kubectl create namespace demo || true

# Create test Rollout
cat <<EOF | kubectl apply -f -
apiVersion: kulta.io/v1alpha1
kind: Rollout
metadata:
  name: test-rollout
  namespace: demo
spec:
  replicas: 3
  selector:
    matchLabels:
      app: test
  template:
    metadata:
      labels:
        app: test
    spec:
      containers:
      - name: nginx
        image: nginx:latest
  strategy:
    canary:
      stableService: test-stable
      canaryService: test-canary
      steps:
      - setWeight: 20
      - setWeight: 50
EOF

# Wait for ReplicaSets
sleep 10

# Verify stable ReplicaSet
STABLE_REPLICAS=$(kubectl get replicaset -n demo -l rollouts.kulta.io/type=stable -o jsonpath='{.items[0].spec.replicas}')
if [ "$STABLE_REPLICAS" != "3" ]; then
  echo "ERROR: Stable ReplicaSet should have 3 replicas, got $STABLE_REPLICAS"
  cat /tmp/kulta-log.txt
  exit 1
fi

# Verify canary ReplicaSet
CANARY_REPLICAS=$(kubectl get replicaset -n demo -l rollouts.kulta.io/type=canary -o jsonpath='{.items[0].spec.replicas}')
if [ "$CANARY_REPLICAS" != "0" ]; then
  echo "ERROR: Canary ReplicaSet should have 0 replicas, got $CANARY_REPLICAS"
  cat /tmp/kulta-log.txt
  exit 1
fi

echo "✅ Integration tests passed!"

# Cleanup
kill $KULTA_PID || true
kind delete cluster --name kulta-ci || true
"#,
        )
        .input_from("build", "binary", "/src/target/release/kulta")
        .when_cond(Condition::event("push").or(Condition::negate(Condition::branch("*"))))
        .timeout(600); // 10 minute timeout

    // Seppo integration tests (alternative k8s testing)
    let _ = p
        .task("seppo-test")
        .from(&rust)
        .run("cargo test --test seppo_integration_test -- --ignored")
        .env("KULTA_RUN_SEPPO_TESTS", "1")
        .inputs(&["tests/seppo_integration_test.rs", "**/*.rs", "Cargo.toml"])
        .after(&["build"]);

    // Stress tests (resource intensive)
    let _ = p
        .task("stress-test")
        .from(&rust)
        .run("cargo test --test stress_test -- --ignored --nocapture")
        .env("KULTA_RUN_STRESS_TESTS", "1")
        .inputs(&["tests/stress_test.rs", "**/*.rs", "Cargo.toml"])
        .after(&["build"]);

    p.emit();
}
