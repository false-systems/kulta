# KULTA: Progressive Delivery Controller

**A learning project for Kubernetes progressive delivery with CDEvents observability**

---

## PROJECT STATUS

**What's Implemented:**
- Canary deployments with step-based traffic shifting
- Blue-green deployments with instant cutover
- Simple rolling updates with observability
- Gateway API HTTPRoute traffic management
- Prometheus metrics-based automated rollback
- CDEvents emission (service.deployed, upgraded, published, rolledback)
- Leader election for HA deployments
- 168+ test cases

**Code Stats:**
- ~10,000 lines of Rust
- Strategy Pattern for deployment types
- No `.unwrap()` in production code
- Proper error types throughout

---

## ARCHITECTURE OVERVIEW

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         KULTA Controller                                 │
│                                                                          │
│  main.rs                                                                 │
│  └── Bootstrap: tracing, health server, leader election, controller     │
│                                                                          │
│  controller/                                                             │
│  ├── rollout.rs        # Main reconciliation loop                       │
│  ├── strategies/                                                         │
│  │   ├── mod.rs        # RolloutStrategy trait + select_strategy()     │
│  │   ├── canary.rs     # Gradual traffic shifting                       │
│  │   ├── blue_green.rs # Instant cutover                                │
│  │   └── simple.rs     # Rolling update                                 │
│  ├── cdevents.rs       # CloudEvents emission                           │
│  └── prometheus.rs     # Metrics evaluation                             │
│                                                                          │
│  server/                                                                 │
│  ├── health.rs         # /healthz, /readyz                              │
│  ├── metrics.rs        # /metrics (Prometheus)                          │
│  ├── leader.rs         # Kubernetes Lease-based leader election         │
│  └── shutdown.rs       # Graceful shutdown handling                     │
│                                                                          │
│  crd/                                                                    │
│  └── rollout.rs        # Rollout CRD definition                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### Strategy Pattern

The core abstraction - each deployment type implements this trait:

```rust
#[async_trait]
pub trait RolloutStrategy: Send + Sync {
    fn name(&self) -> &'static str;
    async fn reconcile_replicasets(&self, rollout: &Rollout, ctx: &Context) -> Result<(), StrategyError>;
    async fn reconcile_traffic(&self, rollout: &Rollout, ctx: &Context) -> Result<(), StrategyError>;
    fn compute_next_status(&self, rollout: &Rollout) -> RolloutStatus;
    fn supports_metrics_analysis(&self) -> bool;
    fn supports_manual_promotion(&self) -> bool;
}
```

### Phase State Machine

```
Initializing ──┬──> Progressing (Canary) ──> Paused ──> Completed
               │
               ├──> Preview (Blue-Green) ──────────────> Completed
               │
               └──> Completed (Simple)

Any phase can transition to Failed (metrics rollback)
```

---

## RUST REQUIREMENTS

### Absolute Rules

1. **No `.unwrap()` in production** - Use `?` or proper error handling
2. **No `println!`** - Use `tracing::{info, warn, error, debug}`
3. **No string enums** - Use proper Rust enums with `#[derive(Serialize, Deserialize)]`
4. **No TODOs or stubs** - Complete implementations only

### Error Handling Pattern

```rust
// BAD
let name = rollout.metadata.name.unwrap();

// GOOD
let name = rollout.metadata.name.as_ref()
    .ok_or(ReconcileError::MissingName)?;
```

### Tracing Pattern

```rust
// BAD
println!("Reconciling: {}", name);

// GOOD
info!(rollout = ?name, namespace = ?ns, "Reconciling rollout");
warn!(error = ?e, "Failed to patch HTTPRoute (non-fatal)");
error!(error = ?e, "Reconciliation failed");
```

---

## TDD WORKFLOW

**RED → GREEN → REFACTOR** - Always.

### RED: Write Failing Test First

```rust
#[tokio::test]
async fn test_canary_creates_both_replicasets() {
    let rollout = create_canary_rollout(3, Some(20));
    let ctx = Context::new_mock();

    // This will FAIL until we implement it
    strategy.reconcile_replicasets(&rollout, &ctx).await.unwrap();

    // Verify both ReplicaSets exist
    // ...
}
```

### GREEN: Minimal Implementation

Write just enough code to make the test pass. No more.

### REFACTOR: Clean Up

Extract helpers, improve naming, add edge cases. Tests must still pass.

### Commit

```bash
git commit -m "feat: implement canary ReplicaSet creation

- Create stable RS with (100 - weight)% replicas
- Create canary RS with weight% replicas
- Tests: test_canary_creates_both_replicasets passing"
```

---

## KEY PATTERNS IN THE CODEBASE

### Idempotent Reconciliation

Every operation is safe to call multiple times:

```rust
pub async fn ensure_replicaset_exists(
    rs_api: &Api<ReplicaSet>,
    rs: &ReplicaSet,
    replicas: i32,
) -> Result<(), ReconcileError> {
    match rs_api.get(rs_name).await {
        Ok(existing) => {
            // Already exists - scale if needed
            if current_replicas != replicas {
                rs_api.patch(...).await?;
            }
        }
        Err(kube::Error::Api(err)) if err.code == 404 => {
            // Not found - create it
            rs_api.create(...).await?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
```

### Status-Driven State

The Rollout status is the source of truth:

```rust
pub struct RolloutStatus {
    pub phase: Option<Phase>,           // Current lifecycle stage
    pub current_step_index: Option<i32>, // Which canary step
    pub current_weight: Option<i32>,     // Traffic percentage
    pub pause_start_time: Option<String>, // RFC3339 timestamp
    // ...
}
```

### Gateway API Traffic Routing

Patch HTTPRoute weights directly:

```rust
let patch = serde_json::json!({
    "spec": {
        "rules": [{
            "backendRefs": [
                { "name": stable_svc, "weight": 80 },
                { "name": canary_svc, "weight": 20 }
            ]
        }]
    }
});
httproute_api.patch(route_name, &PatchParams::default(), &Patch::Merge(&patch)).await?;
```

### CDEvents Emission

Emit standard CloudEvents for observability:

```rust
// Detect transition and emit appropriate event
if is_initialization {
    emit service.deployed
} else if is_step_progression {
    emit service.upgraded
} else if is_rollback {
    emit service.rolledback
} else if is_completion {
    emit service.published
}
```

---

## VERIFICATION CHECKLIST

Before every commit:

```bash
# Format
cargo fmt

# Lint (treat warnings as errors)
cargo clippy -- -D warnings

# Tests
cargo test

# No unwrap in production
grep -r "\.unwrap()" src/ --include="*.rs" | grep -v "_test.rs" | grep -v "#\[test\]"

# No println in production
grep -r "println!" src/ --include="*.rs" | grep -v "_test.rs"
```

---

## COMMON TASKS

### Adding a New Strategy

1. Create `src/controller/strategies/new_strategy.rs`
2. Implement `RolloutStrategy` trait
3. Add to `select_strategy()` in `mod.rs`
4. Add tests
5. Update CRD if needed

### Adding a New Metric Type

1. Add query builder in `prometheus.rs`
2. Add to `evaluate_metric()` match
3. Add tests with mock responses
4. Document in README

### Adding a New CDEvent

1. Add builder function in `cdevents.rs`
2. Add transition detection in `emit_status_change_event()`
3. Add tests verifying event emission

---

## FILE LOCATIONS

| What | Where |
|------|-------|
| Main entry point | `src/main.rs` |
| Rollout CRD | `src/crd/rollout.rs` |
| Reconciliation logic | `src/controller/rollout.rs` |
| Strategy trait | `src/controller/strategies/mod.rs` |
| Canary strategy | `src/controller/strategies/canary.rs` |
| Blue-green strategy | `src/controller/strategies/blue_green.rs` |
| CDEvents | `src/controller/cdevents.rs` |
| Prometheus client | `src/controller/prometheus.rs` |
| Leader election | `src/server/leader.rs` |
| Health endpoints | `src/server/health.rs` |
| K8s manifests | `deploy/` |
| Example rollouts | `examples/` |

---

## DEPENDENCIES

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
| `chrono` | Timestamp parsing |
| `thiserror` | Error types |
| `async-trait` | Async trait support |

---

## AGENT INSTRUCTIONS

When working on this codebase:

1. **Read first** - Understand existing patterns before changing
2. **TDD always** - Write failing test, implement, refactor
3. **No shortcuts** - Proper error handling, tracing, docs
4. **Small commits** - One logical change per commit
5. **Run checks** - `cargo fmt && cargo clippy && cargo test`

**This is a learning project** - code quality matters, but we're exploring and experimenting. Ask questions if unclear.
