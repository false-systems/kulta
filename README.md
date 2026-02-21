# KULTA

**Kubernetes Progressive Delivery Controller**

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Tests](https://img.shields.io/badge/tests-294%2B-green.svg)]()

A Kubernetes controller for progressive delivery, written in Rust. Supports canary, blue-green, A/B testing, and simple rolling updates with Gateway API traffic routing, Prometheus metrics-based rollback, and CDEvents/FALSE Protocol observability.

Part of the [False Systems](https://github.com/false-systems) toolchain.

---

## Features

| Feature | Description |
|---------|-------------|
| **Canary Deployments** | Gradual traffic shifting with configurable steps and pauses |
| **Blue-Green Deployments** | Instant traffic cutover between active and preview environments |
| **A/B Testing** | Header/cookie-based routing with Z-test statistical significance analysis |
| **Simple Rolling Updates** | Standard rollout with observability |
| **Gateway API** | Native HTTPRoute traffic splitting (no service mesh required) |
| **Metrics-Based Rollback** | Automatic rollback via Prometheus (error rate, latency) |
| **CDEvents** | CNCF-standard deployment lifecycle events |
| **FALSE Protocol** | AI-native occurrence emission for AIOps observability |
| **Leader Election** | HA-ready with Kubernetes Lease-based leader election |
| **Time-Based Pauses** | Configurable wait durations between steps |
| **Manual Promotion** | Annotation-based promotion for human-in-the-loop workflows |

---

## Quick Start

```bash
# Clone and build
git clone https://github.com/false-systems/kulta
cd kulta
cargo build --release

# Install Gateway API CRDs
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.0/standard-install.yaml

# Install KULTA CRD
kubectl apply -f deploy/crd.yaml

# Run controller
RUST_LOG=info cargo run
```

**Requirements:** Rust 1.85+, Kubernetes 1.28+, Gateway API v1.0+ CRDs

---

## Deployment Strategies

### Canary

Gradually shift traffic to a new version while monitoring for errors.

```yaml
apiVersion: kulta.io/v1alpha1
kind: Rollout
metadata:
  name: my-app
spec:
  replicas: 5
  selector:
    matchLabels:
      app: my-app
  template:
    metadata:
      labels:
        app: my-app
    spec:
      containers:
      - name: app
        image: myregistry/myapp:v2.0.0
  strategy:
    canary:
      stableService: my-app-stable
      canaryService: my-app-canary
      port: 8080
      steps:
      - setWeight: 20
        pause: { duration: "5m" }
      - setWeight: 50
        pause: { duration: "10m" }
      - setWeight: 80
        pause: {}                    # Wait for manual promotion
      - setWeight: 100
      trafficRouting:
        gatewayAPI:
          httpRoute: my-app-route
      analysis:
        warmupDuration: "1m"
        metrics:
        - name: error-rate
          threshold: 5.0
        - name: latency-p95
          threshold: 500
```

### Blue-Green

Run two identical environments, instant cutover on promotion.

```yaml
strategy:
  blueGreen:
    activeService: my-app-active
    previewService: my-app-preview
    port: 8080
    autoPromotionEnabled: false
    trafficRouting:
      gatewayAPI:
        httpRoute: my-app-route
```

### A/B Testing

Route traffic by header or cookie, evaluate with statistical significance.

```yaml
strategy:
  abTesting:
    variantAService: checkout-control
    variantBService: checkout-experiment
    port: 8080
    maxDuration: "24h"
    variantBMatch:
      header:
        name: X-Variant
        value: B
    trafficRouting:
      gatewayAPI:
        httpRoute: checkout-route
    analysis:
      minDuration: "1h"
      minSampleSize: 1000
      confidenceLevel: 0.95
```

Conclude manually or let statistical analysis determine the winner:
```bash
kubectl annotate rollout my-app kulta.io/conclude-experiment=true
```

### Simple

Standard rolling update with CDEvents observability.

```yaml
strategy:
  simple:
    analysis:
      metrics:
      - name: error-rate
        threshold: 5.0
```

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                         KULTA Controller                              │
│                                                                       │
│  ┌──────────────┐   ┌───────────────────┐   ┌────────────────────┐  │
│  │  Reconciler  │──>│ Strategy Handler  │──>│ ReplicaSet Manager │  │
│  │              │   │ Canary/BG/AB/Simple│  │                    │  │
│  └──────┬───────┘   └────────┬──────────┘   └────────────────────┘  │
│         │                    │                                        │
│         │                    v                                        │
│         │           ┌───────────────────┐   ┌────────────────────┐  │
│         │           │  Traffic Router   │──>│ HTTPRoute Patcher  │  │
│         │           │  (Gateway API)    │   │                    │  │
│         │           └───────────────────┘   └────────────────────┘  │
│         │                                                            │
│         v                                                            │
│  ┌──────────────┐   ┌───────────────────┐   ┌────────────────────┐  │
│  │ Metrics Eval │──>│ Prometheus Client │   │ CDEvents Emitter   │  │
│  │ (Rollback)   │   │ + A/B Z-test      │   │ + FALSE Protocol   │  │
│  └──────────────┘   └───────────────────┘   └────────────────────┘  │
│                                                                       │
│  Clock ─── EventSink ─── MetricsQuerier  (trait-based injection)     │
└──────────────────────────────────────────────────────────────────────┘
```

### Phase State Machine

```
Initializing ──┬──> Progressing (Canary) ──> Paused ──> Completed
               │
               ├──> Preview (Blue-Green) ──────────────> Completed
               │
               ├──> Experimenting (A/B) ──> Concluded ──> Completed
               │
               └──> Completed (Simple)

Any phase can transition to Failed (metrics rollback)
```

---

## Observability

### CDEvents

KULTA emits [CDEvents](https://cdevents.dev/) for deployment pipeline integration:

| Event | Trigger |
|-------|---------|
| `service.deployed` | Rollout started |
| `service.upgraded` | Canary step progressed |
| `service.published` | Rollout completed / experiment concluded |
| `service.rolledback` | Metrics triggered rollback |

### FALSE Protocol

AI-native occurrences for integration with [AHTI](https://github.com/false-systems/ahti) and other False Systems tools:

| Occurrence Type | Trigger |
|----------------|---------|
| `canary.rollout.progressing` | Canary step advanced |
| `bluegreen.rollout.completed` | Blue-green promoted |
| `abtesting.rollout.failed` | A/B experiment failed |
| `rolling.rollout.completed` | Simple rollout done |

Each occurrence includes Error, Reasoning, and History blocks per the FALSE Protocol spec.

---

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level |
| `KULTA_LEADER_ELECTION` | `false` | Enable leader election for HA |
| `KULTA_PROMETHEUS_ADDRESS` | - | Prometheus server URL |
| `KULTA_CDEVENTS_ENABLED` | `false` | Enable CDEvents emission |
| `KULTA_CDEVENTS_SINK_URL` | - | CDEvents HTTP sink URL |
| `KULTA_OCCURRENCE_DIR` | `/tmp/kulta` | FALSE Protocol occurrence output directory |
| `POD_NAME` | hostname | Identifier for leader election |
| `POD_NAMESPACE` | `kulta-system` | Namespace for Lease resource |

### Endpoints

| Port | Endpoint | Purpose |
|------|----------|---------|
| 8080 | `/healthz` | Liveness probe |
| 8080 | `/readyz` | Readiness probe |
| 8080 | `/metrics` | Prometheus metrics |

---

## Development

```bash
cargo build --release       # Build
cargo test                  # Run tests (294+)
cargo clippy -- -D warnings # Lint
cargo fmt                   # Format
```

### Project Structure

```
src/
├── main.rs                          # Bootstrap, health server, leader election
├── crd/
│   └── rollout.rs                   # Rollout CRD definition
├── controller/
│   ├── rollout/                     # Reconciliation (modular)
│   │   ├── reconcile.rs             # Main reconcile loop + Context
│   │   ├── replicaset.rs            # ReplicaSet building + FNV-1a hashing
│   │   ├── status.rs                # Phase state machine
│   │   ├── traffic.rs               # Gateway API HTTPRoute weights
│   │   └── validation.rs            # Rollout spec validation
│   ├── strategies/
│   │   ├── mod.rs                   # RolloutStrategy trait
│   │   ├── canary.rs                # Canary strategy
│   │   ├── blue_green.rs            # Blue-green strategy
│   │   ├── ab_testing.rs            # A/B testing strategy
│   │   └── simple.rs                # Simple rolling update
│   ├── cdevents.rs                  # CDEvents emission (EventSink trait)
│   ├── prometheus.rs                # Prometheus client (MetricsQuerier trait)
│   ├── prometheus_ab.rs             # A/B statistical significance (Z-test)
│   ├── clock.rs                     # Clock trait (SystemClock / MockClock)
│   └── occurrence.rs                # FALSE Protocol occurrences
└── server/
    ├── health.rs                    # /healthz, /readyz
    ├── metrics.rs                   # /metrics (Prometheus)
    ├── leader.rs                    # Kubernetes Lease leader election
    └── shutdown.rs                  # Graceful shutdown
```

### Local Development with Skaffold

```bash
kind create cluster --name kulta-dev
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.0/standard-install.yaml
skaffold dev
```

---

## Comparison

| Feature | KULTA | Argo Rollouts | Flagger |
|---------|-------|---------------|---------|
| Language | Rust | Go | Go |
| Traffic Routing | Gateway API | Istio/NGINX/ALB/Gateway API | Istio/Linkerd/NGINX/Gateway API |
| Service Mesh Required | No | No | No (with Gateway API) |
| A/B Testing | Yes | Yes | Yes |
| CDEvents | Yes | No | No |
| FALSE Protocol | Yes | No | No |

---

## Naming

**Kulta** (Finnish: "gold") — part of the False Systems toolchain:

| Tool | Finnish | Domain |
|------|---------|--------|
| [SYKLI](https://github.com/yairfalse/sykli) | cycle | CI pipelines |
| [NOPEA](https://github.com/false-systems/nopea) | fast | GitOps |
| **KULTA** | gold | Progressive delivery |
| [RAUTA](https://github.com/false-systems/rauta) | iron | Gateway API |
| [AHTI](https://github.com/false-systems/ahti) | water spirit | AIOps correlation |

---

## License

Apache 2.0
