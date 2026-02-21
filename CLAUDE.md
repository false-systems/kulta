# KULTA: Progressive Delivery Controller

Kubernetes controller for progressive delivery (canary, blue-green, A/B testing, simple) with Gateway API traffic routing, Prometheus rollback, CDEvents + FALSE Protocol observability. Written in Rust.

Part of the [False Systems](https://github.com/false-systems) toolchain.

---

## Architecture

```
controller/
├── rollout.rs              # Thin re-export module
├── rollout/                # Reconciliation (split into submodules)
│   ├── reconcile.rs        # Main reconcile loop + Context struct
│   ├── replicaset.rs       # ReplicaSet building + FNV-1a hashing
│   ├── status.rs           # Phase state machine + status computation
│   ├── traffic.rs          # Gateway API HTTPRoute weight patching
│   └── validation.rs       # Rollout spec validation
├── strategies/
│   ├── mod.rs              # RolloutStrategy trait + select_strategy()
│   ├── canary.rs           # Gradual traffic shifting
│   ├── blue_green.rs       # Instant cutover
│   ├── ab_testing.rs       # Header/cookie routing + Z-test analysis
│   └── simple.rs           # Rolling update
├── cdevents.rs             # EventSink trait + CDEvents emission
├── prometheus.rs           # MetricsQuerier trait + Prometheus client
├── prometheus_ab.rs        # A/B statistical significance (Z-test)
├── clock.rs                # Clock trait (SystemClock / MockClock)
└── occurrence.rs           # FALSE Protocol occurrence emission

server/
├── health.rs               # /healthz, /readyz
├── metrics.rs              # /metrics (Prometheus)
├── leader.rs               # Kubernetes Lease-based leader election
└── shutdown.rs             # Graceful shutdown

crd/
└── rollout.rs              # Rollout CRD definition (v1alpha1)
```

### Key Abstractions

**Strategy Pattern** — each deployment type implements `RolloutStrategy`:
```rust
#[async_trait]
pub trait RolloutStrategy: Send + Sync {
    fn name(&self) -> &'static str;
    async fn reconcile_replicasets(&self, rollout: &Rollout, ctx: &Context) -> Result<(), StrategyError>;
    async fn reconcile_traffic(&self, rollout: &Rollout, ctx: &Context) -> Result<(), StrategyError>;
    fn compute_next_status(&self, rollout: &Rollout, now: DateTime<Utc>) -> RolloutStatus;
    fn supports_metrics_analysis(&self) -> bool;
    fn supports_manual_promotion(&self) -> bool;
}
```

**Trait-based DI** — `Context` holds injected dependencies:
```rust
pub struct Context {
    pub client: kube::Client,
    pub cdevents_sink: Arc<dyn EventSink>,
    pub prometheus_client: Arc<dyn MetricsQuerier>,
    pub clock: Arc<dyn Clock>,
    pub leader_state: Option<LeaderState>,
    pub metrics: Option<SharedMetrics>,
}
```

**Phase State Machine:**
```
Initializing ──┬──> Progressing (Canary) ──> Paused ──> Completed
               ├──> Preview (Blue-Green) ──────────────> Completed
               ├──> Experimenting (A/B) ──> Concluded ──> Completed
               └──> Completed (Simple)
Any phase → Failed (metrics rollback)
```

---

## Rust Rules

1. **No `.unwrap()` in production** — use `?` or `ok_or(ReconcileError::...)?`
2. **No `println!`** — use `tracing::{info, warn, error, debug}`
3. **No string enums** — use `#[derive(Serialize, Deserialize)]` Rust enums
4. **No TODOs or stubs** — complete implementations only

---

## Testing

TDD: RED → GREEN → REFACTOR.

Mock constructors: `Context::new_mock()`, `Context::new_mock_with_leader()`.
Custom mocks: `MockPrometheusClient` (with `enqueue_response`/`enqueue_error` queue), `MockEventSink`, `MockClock`.

Test helpers in `rollout_test.rs`: `create_canary_rollout()`, `create_blue_green_rollout()`, `create_ab_rollout_with_analysis()`, `create_test_context_with_prometheus()`.

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```

---

## Dependencies

| Crate | Purpose |
|-------|---------|
| `kube` 2.0 | Kubernetes API client + controller runtime |
| `k8s-openapi` 0.26 | Kubernetes API types |
| `gateway-api` 0.19 | HTTPRoute types |
| `tokio` 1.x | Async runtime |
| `tracing` | Structured logging |
| `axum` 0.8 | HTTP server |
| `cdevents-sdk` | CDEvents emission |
| `reqwest` | HTTP client |
| `ulid` | ULID generation for FALSE Protocol |
| `chrono` | Timestamp handling |
| `thiserror` | Error types |
| `async-trait` | Async trait support |
