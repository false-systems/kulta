# KULTA

**Kubernetes Progressive Delivery Controller - Learning Project**

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.83%2B-orange.svg)](https://www.rust-lang.org)
[![Tests](https://img.shields.io/badge/tests-294%2B-green.svg)]()

A Kubernetes controller for progressive delivery, written in Rust. Built to learn Kubernetes controllers, Rust async, and progressive delivery patterns.

**This is a learning project** - I'm building it to understand:
- How Kubernetes controllers work (kube-rs)
- Rust async programming (tokio)
- Progressive delivery patterns (canary, blue-green)
- Gateway API traffic routing
- CDEvents for observability

**Current Status**: Core features working, actively being developed.

---

## Features

| Feature | Description |
|---------|-------------|
| **Canary Deployments** | Gradual traffic shifting (0% -> 20% -> 50% -> 100%) with configurable steps |
| **Blue-Green Deployments** | Instant traffic cutover between two full environments |
| **A/B Testing** | Statistical significance analysis (Z-test) with header/cookie-based routing |
| **Simple Rolling Updates** | Standard Kubernetes rolling update with observability |
| **Gateway API Traffic Routing** | Native HTTPRoute weight-based traffic splitting (no service mesh required) |
| **Metrics-Based Rollback** | Automatic rollback via Prometheus (error rate, latency thresholds) |
| **CDEvents Observability** | CNCF-standard deployment events for pipeline integration |
| **FALSE Protocol** | AI-native occurrence emission for AIOps tooling (AHTI/Kerto) |
| **Leader Election** | HA-ready with Kubernetes Lease-based leader election |
| **Time-Based Pauses** | Configurable wait durations between steps ("5m", "30s") |
| **Manual Promotion** | Annotation-based promotion for indefinite pauses |

---

## Quick Start

```bash
# Clone and build
git clone https://github.com/yairfalse/kulta
cd kulta
cargo build --release

# Install Gateway API CRDs (required)
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.0/standard-install.yaml

# Install KULTA CRD
kubectl apply -f deploy/crd.yaml

# Run controller (local development)
RUST_LOG=info cargo run

# Or deploy to cluster
kubectl apply -f deploy/
```

**Requirements:**
- Rust 1.85+
- Kubernetes 1.28+
- Gateway API v1.0+ CRDs installed
- A Gateway API implementation (Envoy Gateway, NGINX Gateway Fabric, Contour, etc.)

---

## Deployment Strategies

### Canary Deployment

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
      steps:
      - setWeight: 20
        pause: { duration: "5m" }    # 20% traffic for 5 minutes
      - setWeight: 50
        pause: { duration: "10m" }   # 50% traffic for 10 minutes
      - setWeight: 80
        pause: {}                    # 80% traffic, wait for manual promotion
      - setWeight: 100               # Complete rollout
      trafficRouting:
        gatewayAPI:
          httpRoute: my-app-route
      analysis:
        warmupDuration: "1m"
        metrics:
        - name: error-rate
          threshold: 5.0             # Rollback if error rate > 5%
        - name: latency-p95
          threshold: 500             # Rollback if p95 latency > 500ms
```

**Manual promotion** (for indefinite pauses):
```bash
kubectl annotate rollout my-app kulta.io/promote=true
```

### Blue-Green Deployment

Run two identical environments, instant cutover on promotion.

```yaml
apiVersion: kulta.io/v1alpha1
kind: Rollout
metadata:
  name: my-app
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
      - name: app
        image: myregistry/myapp:v2.0.0
  strategy:
    blueGreen:
      activeService: my-app-active
      previewService: my-app-preview
      autoPromotionEnabled: false    # Require manual promotion
      trafficRouting:
        gatewayAPI:
          httpRoute: my-app-route
```

**Promotion:**
```bash
# Test preview environment first
curl -H "Host: preview.my-app.example.com" http://gateway-ip/

# Promote when ready
kubectl annotate rollout my-app kulta.io/promote=true
```

### Simple Rolling Update

Standard Kubernetes rolling update with CDEvents observability.

```yaml
apiVersion: kulta.io/v1alpha1
kind: Rollout
metadata:
  name: my-app
spec:
  replicas: 3
  selector:
    matchLabels:
      app: my-app
  template:
    # ... pod template
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
┌─────────────────────────────────────────────────────────────────────────┐
│                         KULTA Controller                                 │
│                                                                          │
│  ┌────────────────┐    ┌─────────────────┐    ┌───────────────────┐    │
│  │   Reconciler   │───>│ Strategy Handler│───>│ ReplicaSet Manager│    │
│  │                │    │ (Canary/BG/Simple)   │                    │    │
│  └───────┬────────┘    └────────┬────────┘    └───────────────────┘    │
│          │                      │                                        │
│          │                      v                                        │
│          │             ┌─────────────────┐    ┌───────────────────┐    │
│          │             │ Traffic Router  │───>│ HTTPRoute Patcher │    │
│          │             │ (Gateway API)   │    │                   │    │
│          │             └─────────────────┘    └───────────────────┘    │
│          │                                                              │
│          v                                                              │
│  ┌────────────────┐    ┌─────────────────┐    ┌───────────────────┐    │
│  │ Metrics Eval   │───>│ Prometheus Client│   │ CDEvents Emitter  │    │
│  │ (Rollback)     │    │                 │    │ (Observability)   │    │
│  └────────────────┘    └─────────────────┘    └───────────────────┘    │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    v
┌─────────────────────────────────────────────────────────────────────────┐
│                         Kubernetes Cluster                               │
│                                                                          │
│  ┌─────────────┐   ┌─────────────┐   ┌─────────────┐   ┌────────────┐  │
│  │   Rollout   │   │ ReplicaSets │   │  HTTPRoute  │   │   Lease    │  │
│  │    (CRD)    │   │(stable+canary)  │  (weights)  │   │ (leader)   │  │
│  └─────────────┘   └─────────────┘   └─────────────┘   └────────────┘  │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

### Strategy Pattern

KULTA uses the Strategy Pattern for deployment types, making it easy to add new strategies:

```rust
pub trait RolloutStrategy: Send + Sync {
    fn name(&self) -> &'static str;
    async fn reconcile_replicasets(&self, ...) -> Result<(), StrategyError>;
    async fn reconcile_traffic(&self, ...) -> Result<(), StrategyError>;
    fn compute_next_status(&self, ...) -> RolloutStatus;
    fn supports_metrics_analysis(&self) -> bool;
    fn supports_manual_promotion(&self) -> bool;
}
```

### Phase State Machine

```
                     ┌──────────────┐
                     │ Initializing │
                     └──────┬───────┘
                            │
        ┌───────────────────┼───────────────────┐
        v                   v                   v
 ┌─────────────┐    ┌─────────────┐     ┌─────────────┐
 │ Progressing │    │   Preview   │     │  Completed  │
 │  (Canary)   │    │(Blue-Green) │     │  (Simple)   │
 └──────┬──────┘    └──────┬──────┘     └─────────────┘
        │                  │
        v                  │ promote
 ┌─────────────┐           │
 │   Paused    │<──────────┘
 └──────┬──────┘
        │ timeout / manual promote
        v
 ┌─────────────┐          ┌─────────────┐
 │  Completed  │          │   Failed    │ (metrics rollback)
 └─────────────┘          └─────────────┘
```

---

## Traffic Routing

KULTA uses **Gateway API** for traffic management - no service mesh required.

```
            ┌─────────────┐
            │   Gateway   │
            └──────┬──────┘
                   │
            ┌──────▼──────┐
            │  HTTPRoute  │  <── KULTA patches weights
            │   80 / 20   │
            └──────┬──────┘
       ┌───────────┴───────────┐
       v                       v
┌─────────────┐         ┌─────────────┐
│ stable-svc  │         │ canary-svc  │
│    (80%)    │         │    (20%)    │
└──────┬──────┘         └──────┬──────┘
       │                       │
┌──────▼──────┐         ┌──────▼──────┐
│  stable-rs  │         │  canary-rs  │
│   4 pods    │         │   1 pod     │
└─────────────┘         └─────────────┘
```

**Why Gateway API over Service Mesh?**
- **Simpler**: No sidecars, no proxy injection
- **Transparent**: `kubectl get httproute` shows traffic splits
- **Standard**: Official Kubernetes SIG-Network API
- **Lightweight**: Lower resource overhead

---

## Metrics-Based Rollback

Configure automatic rollback based on Prometheus metrics:

```yaml
analysis:
  warmupDuration: "1m"        # Wait before evaluating metrics
  failurePolicy: Pause        # Pause | Continue | Rollback
  metrics:
  - name: error-rate
    threshold: 5.0            # Percentage (5xx / total * 100)
  - name: latency-p95
    threshold: 500            # Milliseconds
```

**Supported Metrics:**

| Metric | PromQL Template |
|--------|-----------------|
| `error-rate` | `sum(rate(http_requests_total{status=~"5..",rollout="X",revision="Y"}[2m])) / sum(rate(http_requests_total{rollout="X",revision="Y"}[2m])) * 100` |
| `latency-p95` | `histogram_quantile(0.95, rate(http_request_duration_seconds_bucket{rollout="X",revision="Y"}[2m]))` |

**Environment Variables:**
```bash
KULTA_PROMETHEUS_ADDRESS=http://prometheus:9090
```

---

## CDEvents Observability

KULTA emits [CDEvents](https://cdevents.dev/) (CNCF standard) for deployment pipeline integration:

| Event | Trigger |
|-------|---------|
| `service.deployed` | Rollout started |
| `service.upgraded` | Canary step progressed |
| `service.published` | Rollout completed successfully |
| `service.rolledback` | Metrics triggered rollback |

**Configuration:**
```bash
KULTA_CDEVENTS_ENABLED=true
KULTA_CDEVENTS_SINK_URL=http://event-collector:8080/events
```

**Event Payload:**
```json
{
  "type": "dev.cdevents.service.deployed.0.3.0",
  "source": "https://kulta.io",
  "subject": {
    "content": {
      "artifactId": "myregistry/myapp:v2.0.0",
      "environment": { "id": "default/my-app" }
    }
  },
  "customData": {
    "kulta": {
      "strategy": "canary",
      "step": { "index": 1, "total": 4, "traffic_weight": 50 },
      "decision": { "reason": "step_advanced" }
    }
  }
}
```

---

## High Availability

KULTA supports multi-replica deployment with leader election:

```bash
KULTA_LEADER_ELECTION=true
POD_NAME=kulta-controller-abc123      # From Downward API
POD_NAMESPACE=kulta-system            # From Downward API
```

Uses Kubernetes Lease resources for distributed consensus:
- 15-second lease TTL
- 5-second renewal interval
- Automatic failover on leader death

---

## Configuration Reference

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level (trace, debug, info, warn, error) |
| `KULTA_LEADER_ELECTION` | `false` | Enable leader election for HA |
| `KULTA_PROMETHEUS_ADDRESS` | - | Prometheus server URL |
| `KULTA_CDEVENTS_ENABLED` | `false` | Enable CDEvents emission |
| `KULTA_CDEVENTS_SINK_URL` | - | CDEvents HTTP sink URL |
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

### Build & Test

```bash
# Build
cargo build --release

# Run tests (294+ test cases)
cargo test

# Run with verbose output
cargo test -- --nocapture

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt
```

### Local Development with Skaffold

```bash
# Create Kind cluster
kind create cluster --name kulta-dev

# Install Gateway API
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.0/standard-install.yaml

# Start dev loop (auto-rebuild on changes)
skaffold dev
```

### Project Structure

```
kulta/
├── src/
│   ├── main.rs                     # Entry point, controller bootstrap
│   ├── lib.rs                      # Library exports
│   ├── crd/
│   │   └── rollout.rs              # Rollout CRD definition
│   ├── controller/
│   │   ├── rollout.rs              # Main reconciliation logic
│   │   ├── strategies/
│   │   │   ├── mod.rs              # Strategy trait + selection
│   │   │   ├── canary.rs           # Canary implementation
│   │   │   ├── blue_green.rs       # Blue-green implementation
│   │   │   └── simple.rs           # Simple rolling update
│   │   ├── cdevents.rs             # CDEvents emission
│   │   └── prometheus.rs           # Prometheus metrics client
│   └── server/
│       ├── health.rs               # Health endpoints
│       ├── metrics.rs              # Prometheus /metrics
│       ├── leader.rs               # Leader election
│       └── shutdown.rs             # Graceful shutdown
├── deploy/
│   ├── crd.yaml                    # CustomResourceDefinition
│   ├── controller.yaml             # Deployment + Service
│   └── rbac.yaml                   # ServiceAccount, Role, RoleBinding
└── examples/
    └── *.yaml                      # Example Rollout manifests
```

---

## Tech Stack

| Component | Purpose |
|-----------|---------|
| [kube](https://kube.rs) | Kubernetes API client + controller runtime |
| [tokio](https://tokio.rs) | Async runtime |
| [gateway-api](https://gateway-api.sigs.k8s.io) | Traffic routing types |
| [cdevents-sdk](https://cdevents.dev) | CDEvents emission |
| [axum](https://github.com/tokio-rs/axum) | HTTP server for health endpoints |
| [tracing](https://tracing.rs) | Structured logging |
| [thiserror](https://github.com/dtolnay/thiserror) | Error types |

---

## Comparison

| Feature | KULTA | Argo Rollouts | Flagger |
|---------|-------|---------------|---------|
| Language | Rust | Go | Go |
| Traffic Routing | Gateway API | Istio/NGINX/ALB/Gateway API | Istio/Linkerd/NGINX/Gateway API |
| Service Mesh Required | No | No | No (with Gateway API) |
| CDEvents | Yes | No | No |
| Metrics Analysis | Prometheus | Prometheus/Datadog/etc | Prometheus/Datadog/etc |
| Blue-Green | Yes | Yes | Yes |
| Canary | Yes | Yes | Yes |
| A/B Testing | Yes | Yes | Yes |

---

## Naming

**Kulta** (Finnish: "gold") - Part of a Finnish tool naming theme:
- **RAUTA** (iron) - Gateway API controller
- **KULTA** (gold) - Progressive delivery controller

---

## License

Apache 2.0

---

**Learning Rust. Learning K8s. Building tools.**
