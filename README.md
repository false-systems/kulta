# KULTA

**Kubernetes Progressive Delivery Controller - Early Stages Learning Project**

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

---

## What is This?

A Kubernetes controller for progressive delivery (canary rollouts) - **built to learn Rust and K8s controllers**.

This is an **early-stage learning project**. I'm building it to understand:
- How Kubernetes controllers work (kube-rs)
- Rust async programming (tokio)
- Progressive delivery patterns
- Gateway API routing

**Current Status**: Basic canary rollout functionality working, actively being developed.

---

## What Actually Works

**Implemented (as of now)**:
- ✅ Custom Resource Definition (Rollout CRD)
- ✅ Controller reconciliation loop
- ✅ ReplicaSet management (stable + canary)
- ✅ Gateway API HTTPRoute traffic splitting
- ✅ Automatic step progression through canary stages
- ✅ Time-based pause durations (`pause: { duration: "5m" }`)
- ✅ Manual promotion support (indefinite pauses)
- ✅ 36 unit tests passing

**Example Rollout**:
```yaml
apiVersion: kulta.io/v1alpha1
kind: Rollout
metadata:
  name: my-app
  namespace: default
spec:
  replicas: 3
  selector:
    matchLabels:
      app: my-app
  template:
    metadata:
      labels:
        app: my-app
    spec:
      containers:
      - name: nginx
        image: nginx:1.25
  strategy:
    canary:
      stableService: my-app-stable
      canaryService: my-app-canary
      steps:
      - setWeight: 20   # Start with 20% canary traffic
        pause:
          duration: "5m"  # Wait 5 minutes
      - setWeight: 50   # Move to 50% canary traffic
        pause: {}       # Indefinite pause (manual promotion required)
      - setWeight: 100  # Complete rollout
      trafficRouting:
        gatewayAPI:
          httpRoute: my-app-route
```

**Manual promotion** (for indefinite pauses):
```bash
kubectl annotate rollout my-app kulta.io/promote=true
```

**What happens**:
1. Controller creates stable and canary ReplicaSets
2. Updates HTTPRoute weights (80/20 split)
3. Waits 5 minutes
4. Updates HTTPRoute weights (50/50 split)
5. Waits for manual promotion
6. Updates HTTPRoute weights (0/100 - fully canary)
7. Rollout complete

---

## Not Yet Implemented

**Planned but not built**:
- Prometheus metrics analysis
- Automated rollback on errors
- Blue-green deployments
- Health checking integration
- CDEvents emission

These are learning goals, not promises. I'm building features as I learn the concepts.

---

## Quick Start

```bash
# Clone repo
git clone https://github.com/yairfalse/kulta
cd kulta

# Build
cargo build --release

# Generate CRD
cargo run --bin gen-crd > /tmp/rollout-crd.yaml

# Install CRD in your cluster
kubectl apply -f /tmp/rollout-crd.yaml

# Install Gateway API CRDs (required)
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.0/standard-install.yaml

# Run controller locally (requires KUBECONFIG)
RUST_LOG=info cargo run --bin kulta

# Apply example rollout
kubectl apply -f examples/basic-rollout.yaml
```

**Requirements**:
- Rust 1.75+
- Kubernetes cluster (kind/minikube/real cluster)
- Gateway API CRDs
- A Gateway API implementation (like RAUTA, or any other)

---

## Why Gateway API?

I'm using Gateway API for traffic routing instead of a service mesh because:
- **Simpler**: Just HTTPRoute weight changes, no sidecars
- **Transparent**: `kubectl get httproute` shows traffic splits
- **Standard**: K8s sig-network official API

---

## Architecture

### How It Works

```
┌─────────────────────────────────────────────────────────┐
│  Kubernetes Cluster                                     │
│                                                         │
│  ┌─────────────────────────────────────────────────┐   │
│  │  User applies Rollout YAML                      │   │
│  └──────────────────┬──────────────────────────────┘   │
│                     │                                   │
│                     ↓                                   │
│  ┌─────────────────────────────────────────────────┐   │
│  │  KULTA Controller (this project)                │   │
│  │  ┌───────────────────────────────────────────┐  │   │
│  │  │  Reconciliation Loop                      │  │   │
│  │  │  1. Read Rollout spec                     │  │   │
│  │  │  2. Create/update stable ReplicaSet       │  │   │
│  │  │  3. Create/update canary ReplicaSet       │  │   │
│  │  │  4. Update HTTPRoute weights (20/80)      │  │   │
│  │  │  5. Check if pause duration elapsed       │  │   │
│  │  │  6. Progress to next step (50/50)         │  │   │
│  │  │  7. Wait for manual promotion             │  │   │
│  │  │  8. Final step (100/0 - fully canary)     │  │   │
│  │  └───────────────────────────────────────────┘  │   │
│  └──────────────────┬──────────────────────────────┘   │
│                     │                                   │
│                     ↓                                   │
│  ┌─────────────────────────────────────────────────┐   │
│  │  Gateway API HTTPRoute                          │   │
│  │  - stable backend (80% → 50% → 0%)              │   │
│  │  - canary backend (20% → 50% → 100%)            │   │
│  └──────────────────┬──────────────────────────────┘   │
│                     │                                   │
│                     ↓                                   │
│  ┌─────────────────────────────────────────────────┐   │
│  │  Gateway API Implementation                     │   │
│  │  (RAUTA, Envoy Gateway, etc.)                   │   │
│  │  - Routes traffic based on weights              │   │
│  └──────────────────┬──────────────────────────────┘   │
│                     │                                   │
│                     ↓                                   │
│  ┌─────────────────────────────────────────────────┐   │
│  │  Pods                                           │   │
│  │  ┌──────────────┐  ┌──────────────┐            │   │
│  │  │ Stable Pods  │  │ Canary Pods  │            │   │
│  │  │ (old version)│  │ (new version)│            │   │
│  │  └──────────────┘  └──────────────┘            │   │
│  └─────────────────────────────────────────────────┘   │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

### Component Interaction

```
Rollout CRD
    │
    │ defines canary strategy
    │
    ↓
KULTA Controller
    │
    ├─→ ReplicaSets (stable + canary)
    │   │
    │   └─→ Pods with labels
    │       - rollouts.kulta.io/type=stable
    │       - rollouts.kulta.io/type=canary
    │
    └─→ HTTPRoute (Gateway API)
        │
        ├─→ backendRefs[0]: stable service (weight: 80 → 50 → 0)
        └─→ backendRefs[1]: canary service (weight: 20 → 50 → 100)
```

---

## Development

### Build and Test

```bash
# Build
cargo build

# Run all tests
cargo test

# Format
cargo fmt

# Lint
cargo clippy
```

### TDD Workflow

I'm following Test-Driven Development:
1. Write failing test (RED)
2. Minimal implementation (GREEN)
3. Refactor
4. Commit

Check `docs/design/` for implementation plans.

---

## Tech Stack

- **tokio** - Async runtime
- **kube-rs** - Kubernetes API client
- **gateway-api** - Gateway API CRD types
- **chrono** - Timestamp handling for pauses
- **serde** - Serialization

---

## Naming

**Kulta** (Finnish: "gold") - Part of my Finnish tool naming theme:
- **RAUTA** (iron) - Gateway API routing
- **KULTA** (gold) - Progressive delivery

---

## Current Focus

Right now I'm working on:
- Getting time-based pauses working reliably
- Testing with real deployments in kind
- Understanding controller patterns better

This is a **learning project** - I'm figuring things out as I go. Code quality will improve as I learn more Rust and K8s patterns.

---

## License

Apache 2.0

---

**Learning Rust. Learning K8s. Building tools.** 🦀
