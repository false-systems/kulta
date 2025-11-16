# KULTA: Progressive Delivery Controller with CDEvents

**KULTA = Gold-standard deployments with observable pipelines**

---

## CRITICAL: Project Nature

**THIS IS A FUN LEARNING PROJECT**
- **Goal**: Build a Gateway API-native progressive delivery controller with CDEvents observability
- **Language**: 100% Rust
- **Status**: Just starting - exploring and learning
- **Approach**: Have fun, ship working code, no pressure

---

## PROJECT MISSION

**Mission**: Make progressive delivery simple and observable

**Core Value Proposition:**

**"Progressive delivery without service mesh complexity - fast, transparent, observable via CDEvents"**

**The Differentiators:**
1. **Gateway API-native** - No service mesh required (simpler stack)
2. **CDEvents built-in** - Full pipeline observability (git → deploy → production)
3. **Rust performance** - Fast reconciliation, low memory
4. **Works with RAUTA** - Integrated Rust-based Gateway + CD stack

**Core Philosophy:**
- **Rust for safety and performance** (building on RAUTA knowledge)
- **Gateway API only** (no service mesh complexity)
- **CDEvents for observability** (make CD visible like CI)
- **Fun first** (learning project, not production pressure)

---

## ARCHITECTURE PHILOSOPHY

### The Vision

**KULTA is a progressive delivery controller that:**

```
┌────────────────────────────────────────────────────────┐
│                KULTA Architecture                       │
├────────────────────────────────────────────────────────┤
│                                                        │
│  Kubernetes Controller (Rust)                          │
│  ┌──────────────────────────────────────────────────┐ │
│  │  Rollout Reconciler                              │ │
│  │  - Watches Rollout CRD (Argo Rollouts compatible)│ │
│  │  - Creates canary ReplicaSets                    │ │
│  │  - Manages traffic shifts (10% → 50% → 100%)     │ │
│  │                                                  │ │
│  │  Gateway API Integration                         │ │
│  │  - Updates HTTPRoute weights                     │ │
│  │  - No service mesh required                      │ │
│  │  - Works with RAUTA or any Gateway API impl      │ │
│  │                                                  │ │
│  │  Metrics Analysis                                │ │
│  │  - Queries Prometheus                            │ │
│  │  - Checks error rates, latency                   │ │
│  │  - Auto-rollback on threshold violations         │ │
│  │                                                  │ │
│  │  CDEvents Emission                               │ │
│  │  - deployment.started                            │ │
│  │  - deployment.progressed (weight changes)        │ │
│  │  - deployment.failed (rollback)                  │ │
│  │  - deployment.finished (success)                 │ │
│  └──────────────────────────────────────────────────┘ │
│                                                        │
└────────────────────────────────────────────────────────┘
```

**Why This Architecture:**
- **No service mesh** - Gateway API handles traffic routing
- **Argo Rollouts compatible** - Drop-in replacement (use same CRDs)
- **CDEvents native** - Observability built-in, not bolted-on
- **Rust controller** - Fast, safe, low resource usage

---

## RUST REQUIREMENTS

### Language Requirements
- **THIS IS A RUST PROJECT** - All code in Rust
- **NO GO CODE** - Unlike Argo Rollouts (Go), we use Rust
- **STRONG TYPING ONLY** - No `Box<dyn Any>` or runtime type checking

### Controller Pattern

```rust
use kube::{Api, Client, runtime::controller};
use k8s_openapi::api::apps::v1::{Deployment, ReplicaSet};

struct RolloutController {
    client: Client,
    cdevents_sink: CDEventsSink,
}

async fn reconcile_rollout(
    rollout: Arc<Rollout>,
    ctx: Arc<Context>
) -> Result<Action> {
    match rollout.status.phase {
        Phase::Initializing => {
            // Emit: deployment.started
            emit_cdevent(CDEvent::DeploymentStarted {
                id: rollout.uid,
                git_commit: get_commit_from_annotations(&rollout),
            }).await?;

            // Create canary ReplicaSet
            create_canary_replicaset(&rollout).await?;
        }

        Phase::ProgressingStep { weight } => {
            // Update Gateway API HTTPRoute
            update_http_route_weight(&rollout, weight).await?;

            // Emit: deployment.progressed
            emit_cdevent(CDEvent::DeploymentProgressed {
                id: rollout.uid,
                traffic_weight: weight,
            }).await?;

            // Analyze metrics
            let health = analyze_prometheus_metrics(&rollout).await?;

            if health.is_degraded() {
                // Rollback
                emit_cdevent(CDEvent::DeploymentFailed {
                    id: rollout.uid,
                    reason: health.failure_reason(),
                }).await?;

                rollback(&rollout).await?;
            } else {
                // Advance to next step
                advance_rollout(&rollout).await?;
            }
        }

        Phase::Completed => {
            emit_cdevent(CDEvent::DeploymentFinished {
                id: rollout.uid,
                status: "success",
            }).await?;
        }
    }

    Ok(Action::requeue(Duration::from_secs(30)))
}
```

**NO STUBS. NO TODOs. COMPLETE CODE ONLY.**

---

## TDD Workflow (RED → GREEN → REFACTOR)

**MANDATORY**: All code must follow strict Test-Driven Development

### RED Phase: Write Failing Tests First

```rust
// Step 1: Write test that FAILS (RED)
#[tokio::test]
async fn test_rollout_creates_canary_replicaset() {
    let client = Client::try_default().await.unwrap();

    let rollout = create_test_rollout("my-app", 3, "my-app:v2");

    // Create rollout
    let rollouts: Api<Rollout> = Api::namespaced(client.clone(), "default");
    rollouts.create(&PostParams::default(), &rollout).await.unwrap();

    // Wait for reconciliation
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Verify canary ReplicaSet created
    let rs: Api<ReplicaSet> = Api::namespaced(client.clone(), "default");
    let canary_rs = rs.get("my-app-canary").await.unwrap();

    assert_eq!(canary_rs.spec.unwrap().replicas.unwrap(), 1);
}

// Step 2: Verify test FAILS
// $ cargo test
// # test_rollout_creates_canary_replicaset ... FAILED (RED phase confirmed)
```

### GREEN Phase: Minimal Implementation

```rust
// Step 3: Write MINIMAL code to pass test
async fn create_canary_replicaset(rollout: &Rollout) -> Result<()> {
    let rs: Api<ReplicaSet> = Api::namespaced(
        client.clone(),
        &rollout.namespace().unwrap()
    );

    let canary_rs = ReplicaSet {
        metadata: ObjectMeta {
            name: Some(format!("{}-canary", rollout.name())),
            namespace: rollout.namespace(),
            owner_references: Some(vec![rollout.controller_owner_ref(&())]),
            ..Default::default()
        },
        spec: Some(ReplicaSetSpec {
            replicas: Some(1),  // Start with 1 replica
            selector: rollout.spec.selector.clone(),
            template: rollout.spec.template.clone(),
        }),
        ..Default::default()
    };

    rs.create(&PostParams::default(), &canary_rs).await?;
    Ok(())
}

// Step 4: Verify tests PASS
// $ cargo test
// # test_rollout_creates_canary_replicaset ... ok (GREEN phase confirmed)
```

### TDD Checklist

- [ ] **RED**: Write failing test first
- [ ] **RED**: Verify compilation fails or test fails
- [ ] **GREEN**: Write minimal implementation
- [ ] **GREEN**: Verify all tests pass
- [ ] **REFACTOR**: Add edge cases, improve design
- [ ] **REFACTOR**: Verify tests still pass
- [ ] **Commit**: `git add . && git commit -m "feat: ..."` (incremental commits)

---

## REALISTIC SCOPE

### What KULTA IS

**A learning project for progressive delivery**
- Explore Kubernetes controllers (building on RAUTA)
- Learn CDEvents integration
- Practice Gateway API manipulation
- Have fun with Rust async

**A simpler alternative to Argo Rollouts**
- Gateway API-native (no service mesh)
- CDEvents built-in (observable by default)
- Argo Rollouts API-compatible (easy migration)

### What KULTA IS NOT

**Not trying to replace Argo Rollouts**
- Argo is mature, battle-tested
- KULTA is focused on simplicity + observability
- Different target: teams using Gateway API

**Not a production requirement**
- This is a fun project
- No timeline, no pressure
- Learning comes first

---

## DEFINITION OF DONE

A feature is complete when:

- [ ] Design sketched (doesn't need docs/)
- [ ] Rust tests passing
- [ ] TDD workflow followed (RED → GREEN → REFACTOR)
- [ ] Works in kind cluster
- [ ] Commit message explains what/why

**NO STUBS. NO TODOs. COMPLETE CODE OR NOTHING.**

---

## DEVELOPMENT ROADMAP

### V1: Basic Canary (Fun Weekend Project)

**Goal**: Manually controlled canary rollout

- [ ] Define basic Rollout CRD (simplified Argo API)
- [ ] Watch Rollout resources
- [ ] Create canary ReplicaSet
- [ ] Update HTTPRoute weights manually (kubectl)
- [ ] Test with RAUTA in kind cluster

**Scope**: Just prove the concept works

### V2: Automated Progression

**Goal**: Automatic traffic shifting

- [ ] Define rollout steps (10%, 50%, 100%)
- [ ] Automatic weight progression
- [ ] Pause duration support
- [ ] Manual pause/resume

**Scope**: Basic automation

### V3: Metrics Analysis

**Goal**: Auto-rollback on errors

- [ ] Prometheus client integration
- [ ] Query error rates, latency
- [ ] Threshold checking
- [ ] Automatic rollback

**Scope**: Safety features

### V4: CDEvents Integration

**Goal**: Full pipeline observability

- [ ] CDEvents SDK integration
- [ ] Emit deployment events
- [ ] Link to git commits (annotations/labels)
- [ ] Works with CDviz

**Scope**: The differentiator

---

## COMPETITIVE POSITIONING

**vs Argo Rollouts:**
- Argo: Mature, feature-rich, Go-based
- KULTA: Simple, Gateway API-native, CDEvents built-in, Rust-based

**vs Flagger:**
- Flagger: Requires service mesh
- KULTA: Gateway API only

**The Pitch:**
> "KULTA: Progressive delivery for teams using Gateway API. Simple (no service mesh), fast (Rust), observable (CDEvents). Works perfectly with RAUTA."

---

## FINAL MANIFESTO

**KULTA is a fun learning project for building progressive delivery in Rust.**

**We're building:**
- Simple progressive delivery (no service mesh)
- Gateway API-native (works with RAUTA)
- CDEvents observability (make CD visible)
- Learning Rust controllers (building on RAUTA)

**We're NOT building:**
- Production requirement (this is for fun)
- Argo Rollouts replacement (learn from Argo, simplify for Gateway API)
- Enterprise features (keep it simple)

**Learn. Build. Have Fun.**

---

**Gold-standard deployments with observable pipelines.**
