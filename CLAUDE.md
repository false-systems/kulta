# KULTA: Progressive Delivery Controller with CDEvents

**KULTA = Gold-standard deployments with observable pipelines**

---

## CRITICAL: Project Nature

**THIS IS A FUN LEARNING PROJECT**
- **Goal**: Build a Gateway API-native progressive delivery controller with CDEvents observability
- **Language**: 100% Rust
- **Status**: Just starting - exploring and learning

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

---

## ARCHITECTURE PHILOSOPHY

### The Core Insight

**THE CORE INSIGHT**: Without status tracking and history, you're just reacting to changes.
With proper state management, you're building deployment intelligence.

**The Rollout Status IS the brain:**
- **Spec**: Desired state (what user wants)
- **Reconcile**: Drive toward desired state
- **ReplicaSets**: Actuate the deployment
- **HTTPRoute**: Control traffic
- **Status**: Track reality (what actually happened)
- **CDEvents**: Broadcast the story

**Every feature must consider:**
1. What status fields track this?
2. What CDEvents describe this?
3. How do we recover from failure?
4. What does time-travel query show?

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
- **CDEvents native** - Observability built-in, not bolted-on
- **Rust controller** - Fast, safe, low resource usage

---

## RUST REQUIREMENTS

### Language Requirements
- **THIS IS A RUST PROJECT** - All code in Rust
- **NO GO CODE** - Unlike Argo Rollouts (Go), we use Rust
- **STRONG TYPING ONLY** - No `Box<dyn Any>` or runtime type checking

---

## ⛔ RUST CODE QUALITY - INSTANT REJECTION

### No .unwrap() in Production - Ever

❌ **BANNED:**
```rust
let name = rollout.metadata.name.unwrap();
```

✅ **REQUIRED:**
```rust
let name = rollout.metadata.name.as_ref()
    .ok_or(ReconcileError::MissingName)?;
```

### No println! in Production - Ever

❌ **BANNED:**
```rust
println!("Reconciling: {}", name);
```

✅ **REQUIRED:**
```rust
use tracing::info;
info!(rollout = ?name, "Reconciling");
```

### No String Enums - Ever

❌ **BANNED:**
```rust
phase: Some("Progressing".to_string())
```

✅ **REQUIRED:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Phase {
    Initializing,
    Progressing,
    Paused,
    Completed,
    Failed,
}

phase: Some(Phase::Progressing)
```

---

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

We only work in very short commits, very fast intervals.
NO hardcoded values
Lean mother fucking code
FAST like Hell
NO SHORTCUTS, WE PUT EFFORT for BEST PRACTICES!


### TDD Checklist

- [ ] **RED**: Write failing test first
- [ ] **RED**: Verify compilation fails or test fails
- [ ] **GREEN**: Write minimal implementation
- [ ] **GREEN**: Verify all tests pass
- [ ] **REFACTOR**: Add edge cases, improve design
- [ ] **REFACTOR**: Verify tests still pass
- [ ] **Commit**: `git add . && git commit -m "feat: ..."` (incremental commits)

---
We only work in very short commits

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


---

## DEFINITION OF DONE

A feature is complete when:

- [ ] Design sketched (doesn't need docs/)
- [ ] Rust tests passing
- [ ] TDD workflow followed (RED → GREEN → REFACTOR)
- [ ] Works in kind cluster
- [ ] Commit message explains what/why

**NO STUBS. NO TODOs. COMPLETE CODE OR NOTHING.**

MANDATORY WORKFLOW:
1. Read relevant source files (use view/cat commands)
2. Read existing tests (understand expected behavior)
3. Understand current architecture (don't break patterns)
4. Propose implementation (get human approval)
5. Write code (follow existing style)


2. Follow TDD Strictly (RED → GREEN → REFACTOR)
Agents must follow the TDD workflow explicitly:
rust// PHASE 1: RED (Agent writes failing test)
#[tokio::test]
async fn test_reconcile_updates_status() {
    let rollout = create_test_rollout();
    reconcile(rollout.clone(), ctx).await.unwrap();
    
    let updated = get_rollout("test-rollout").await.unwrap();
    assert_eq!(updated.status.unwrap().phase, Some("Progressing".to_string()));
}

// Agent confirms: "Test written, should FAIL. Run cargo test to verify RED phase."

// PHASE 2: GREEN (Agent implements minimal code)
// In reconcile():
let status = RolloutStatus {
    phase: Some("Progressing".to_string()),
    replicas: rollout.spec.replicas,
    ..Default::default()
};

// Agent confirms: "Implementation added. Run cargo test to verify GREEN phase."

// PHASE 3: REFACTOR (Agent improves)
// Add error handling, edge cases, better naming
3. No Stubs, No TODOs
Agents must write complete, working code:
BAD (Agent generates stub):
rustasync fn emit_cdevent(rollout: &Rollout) -> Result<()> {
    // TODO: Implement CDEvents emission
    todo!("CDEvents not implemented yet")
}
GOOD (Agent implements or asks):
rust// Option 1: Agent implements if clear
async fn emit_cdevent(rollout: &Rollout) -> Result<()> {
    let event = CloudEvent::new()
        .event_type("dev.cdevents.deployment.started.v1")
        .source("kulta-controller")
        .build()?;
    
    HTTP_CLIENT.post(&CDEVENTS_SINK_URL)
        .json(&event)
        .send().await?;
    Ok(())
}

// Option 2: Agent asks if unclear
// "I need clarification: Should CDEvents be sent via HTTP POST?"
4. Provide Complete Context in Responses
When implementing features, agents must:
Show the full implementation:
rust// GOOD: Complete, copy-pasteable code
pub async fn reconcile(
    rollout: Arc<Rollout>,
    ctx: Arc<Context>,
) -> Result<Action, ReconcileError> {
    let namespace = rollout.metadata.namespace.as_ref()
        .ok_or_else(|| ReconcileError::MissingNamespace)?;

    let rs_api: Api<ReplicaSet> = Api::namespaced(ctx.client.clone(), namespace);
    let stable_rs = build_replicaset(&rollout, "stable", rollout.spec.replicas);
    
    // ... complete implementation, not snippets
}
Explain what changed:
Changes made:
1. Added namespace validation (line 5-6)
2. Created ReplicaSet API client (line 8)
3. Built stable ReplicaSet (line 9)

Files modified:
- src/controller/rollout.rs (reconcile function)

Tests to run:
- cargo test test_reconcile_creates_stable_replicaset
5. Incremental Commits Matter
Agents should suggest commit messages:
bash# After implementing stable ReplicaSet creation:
git commit -m "feat: implement stable ReplicaSet creation in reconcile

- Add namespace validation
- Create ReplicaSet if missing (404 check)
- Proper error handling for API errors
- Tests: test_reconcile_creates_stable_replicaset passing"

AI AGENT TDD WORKFLOW
When human requests a feature, agent MUST follow this exact sequence:
Step 1: UNDERSTAND
- Read relevant source files
- Check existing tests
- Ask clarifying questions if unclear

Step 2: RED (Write Failing Test)
Agent: "I'll write the test first. Here's the failing test..."
[Provides complete test code]
Agent: "This should FAIL. Run cargo test to verify RED phase."

Step 3: GREEN (Minimal Implementation)
Agent: "Here's the minimal implementation to make the test pass..."
[Provides complete implementation]
Agent: "This should PASS. Run cargo test to verify GREEN phase."

Step 4: REFACTOR (Improve Code)
Agent: "Now let's improve the implementation..."
[Adds error handling, edge cases, better naming]
Agent: "Tests should still pass. Run cargo test to verify."

Step 5: COMMIT
Agent: "Ready to commit. Suggested message:
feat: add status updates to reconcile

- Updates phase to Progressing
- Sets replicas count from ReplicaSet
- Tests passing"

ERROR HANDLING PATTERNS
Agents must use proper Rust error handling:
BAD:
rustasync fn reconcile() -> Result<Action> {
    let rs = get_replicaset().await.unwrap(); // NEVER unwrap in production
    Ok(Action::requeue(Duration::from_secs(300)))
}
GOOD:
rustasync fn reconcile() -> Result<Action, ReconcileError> {
    let rs = match get_replicaset().await {
        Ok(rs) => rs,
        Err(kube::Error::Api(err)) if err.code == 404 => {
            // Not found, create it
            create_replicaset().await?
        }
        Err(e) => return Err(ReconcileError::KubeError(e)),
    };
    Ok(Action::requeue(Duration::from_secs(300)))
}
Error types must be explicit:
rust#[derive(Debug, Error)]
pub enum ReconcileError {
    #[error("Kubernetes API error: {0}")]
    KubeError(#[from] kube::Error),
    
    #[error("Rollout missing namespace")]
    MissingNamespace,
    
    #[error("CDEvents emission failed: {0}")]
    CDEventsError(String),
}

CODE STYLE REQUIREMENTS
Agents must follow these Rust patterns:
1. Use strong types, not stringly-typed code:
BAD:
rustlet phase = "Progressing"; // String literal
rollout.status.phase = Some(phase.to_string());
GOOD:
rust#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Phase {
    Initializing,
    Progressing,
    Paused,
    Completed,
    Failed,
}

rollout.status.phase = Some(Phase::Progressing);
2. Prefer explicit over implicit:
BAD:
rustlet name = rollout.metadata.name.unwrap(); // What if None?
GOOD:
rustlet name = rollout.metadata.name.as_ref()
    .ok_or(ReconcileError::MissingName)?;
3. Use tracing, not println:
BAD:
rustprintln!("Reconciling rollout: {}", name);
GOOD:
rustuse tracing::{info, warn, error};

info!(rollout = ?name, "Reconciling rollout");
warn!(rs_name = ?rs_name, "ReplicaSet not found, creating");
error!(error = ?err, "Failed to create ReplicaSet");

TESTING REQUIREMENTS
Agents must write tests that:
1. Test one thing clearly:
BAD (tests multiple things):
rust#[tokio::test]
async fn test_reconcile() {
    // Tests ReplicaSet creation AND status updates AND CDEvents
    // Hard to debug when it fails
}
GOOD (focused tests):
rust#[tokio::test]
async fn test_reconcile_creates_stable_replicaset() {
    // Only tests ReplicaSet creation
}

#[tokio::test]
async fn test_reconcile_updates_status() {
    // Only tests status updates
}

#[tokio::test]
async fn test_reconcile_emits_cdevents() {
    // Only tests CDEvents emission
}
2. Use descriptive test names:
BAD: test_reconcile()
GOOD: test_reconcile_creates_stable_replicaset_when_missing()
3. Follow AAA pattern (Arrange, Act, Assert):
rust#[tokio::test]
async fn test_reconcile_creates_canary_when_stable_exists() {
    // ARRANGE: Set up test state
    let rollout = create_test_rollout();
    let stable_rs = create_stable_replicaset(&rollout).await;
    
    // ACT: Execute the code under test
    reconcile(Arc::new(rollout), ctx).await.unwrap();
    
    // ASSERT: Verify the results
    let canary = get_replicaset("test-rollout-canary").await.unwrap();
    assert_eq!(canary.spec.unwrap().replicas, Some(0));
}

DOCUMENTATION REQUIREMENTS
Agents must add documentation:
1. Module-level docs:
rust//! Rollout controller reconciliation logic
//!
//! This module implements the core reconciliation loop for Rollout resources.
//! It creates and manages ReplicaSets, updates Gateway API HTTPRoutes,
//! and emits CDEvents for observability.

pub mod rollout;
2. Function-level docs:
rust/// Reconcile a Rollout resource
///
/// This function implements the main reconciliation logic:
/// 1. Creates stable ReplicaSet if missing
/// 2. Creates canary ReplicaSet (0 replicas initially)
/// 3. Updates HTTPRoute weights based on rollout steps
/// 4. Emits CDEvents at each stage
///
/// # Arguments
/// * `rollout` - The Rollout resource to reconcile
/// * `ctx` - Controller context (k8s client, CDEvents sink)
///
/// # Returns
/// * `Ok(Action)` - Next reconciliation action (requeue after duration)
/// * `Err(ReconcileError)` - Reconciliation error
pub async fn reconcile(
    rollout: Arc<Rollout>,
    ctx: Arc<Context>,
) -> Result<Action, ReconcileError> {
    // Implementation
}
3. Complex logic needs comments:
rust// Compute pod-template-hash like Kubernetes does
// This ensures ReplicaSets are uniquely identified by their PodSpec
let hash = compute_pod_template_hash(&rollout.spec.template);

// Check if ReplicaSet exists (404 = not found, other errors = fail)
match rs_api.get(&rs_name).await {
    Ok(existing) => { /* already exists */ }
    Err(kube::Error::Api(err)) if err.code == 404 => { /* create it */ }
    Err(e) => return Err(e.into()),
}

AGENT CHECKLIST FOR EVERY IMPLEMENTATION
Before submitting code, agents must verify:

 Read existing code to understand patterns
 Wrote failing test first (RED phase)
 Implemented minimal code to pass test (GREEN phase)
 Added error handling (no unwrap in production code)
 Added tracing (info/warn/error, not println)
 Added documentation (module, function, complex logic)
 Followed Rust idioms (strong types, explicit errors)
 Suggested commit message
 Tests pass (cargo test)
 Code compiles (cargo build)
 No TODOs or stubs left in code


EXAMPLE: GOOD AGENT INTERACTION
Human: "Implement stable ReplicaSet creation in reconcile"

Agent:
"I'll implement stable ReplicaSet creation following TDD. Let me first check 
the current code..."

[Reads src/controller/rollout.rs]
[Reads src/crd/rollout.rs]
[Reads src/controller/rollout_test.rs]

Agent:
"I see test_reconcile_creates_stable_replicaset already exists and is failing.
This is the RED phase. I'll now implement the minimal code to make it pass."

[Provides complete implementation]

Agent:
"Implementation complete. Changes made:
1. Added namespace validation
2. Created ReplicaSet API client
3. Check if stable ReplicaSet exists (handle 404)
4. Create ReplicaSet if missing
5. Added tracing (info when creating)

Run: cargo test test_reconcile_creates_stable_replicaset

This should pass (GREEN phase). Ready to commit?

Suggested commit:
feat: implement stable ReplicaSet creation in reconcile

- Add namespace validation with proper error
- Create stable ReplicaSet if missing (404 check)
- Add tracing for ReplicaSet operations
- Tests: test_reconcile_creates_stable_replicaset passing"

FINAL REMINDER FOR AGENTS
KULTA is a learning project, but code quality matters:

Write production-quality code (proper errors, tracing, docs)
Follow TDD strictly (RED → GREEN → REFACTOR)
No stubs or TODOs (complete implementations only)
Test thoroughly (unit tests + integration tests)
Commit incrementally (small, focused commits)

When in doubt, ask the human for clarification.



## VERIFICATION CHECKLIST

Before EVERY commit:

```bash
# 1. Format - MANDATORY
cargo fmt

# 2. Clippy - MANDATORY  
cargo clippy -- -D warnings

# 3. Tests - MANDATORY
cargo test

# 4. No .unwrap() in src/ (except tests)
find src -name "*.rs" -not -path "*/test*" \
  -exec grep -l "\.unwrap()" {} \;

# 5. No println! in src/ (except tests)
find src -name "*.rs" -not -path "*/test*" \
  -exec grep -l "println!" {} \;

# 6. No TODOs/stubs
grep -r "TODO\|FIXME\|todo!\|unimplemented!" src/

# 7. All errors are ReconcileError
# Check that we're not using anyhow::Error in reconcile
```

---

## AUTOMATED PRE-COMMIT HOOK

Create `.git/hooks/pre-commit` (executable):

```bash
#!/bin/bash
# KULTA pre-commit quality checks

echo "🔍 Running KULTA pre-commit checks..."

# Format
echo "→ cargo fmt..."
if ! cargo fmt --check; then
    echo "❌ Run: cargo fmt"
    exit 1
fi

# Clippy
echo "→ cargo clippy..."
if ! cargo clippy -- -D warnings; then
    echo "❌ Fix clippy warnings"
    exit 1
fi

# No .unwrap() in production
echo "→ Checking for .unwrap() abuse..."
if find src -name "*.rs" -not -path "*/test*" -exec grep -l "\.unwrap()" {} \; | grep -v "// ALLOWED:"; then
    echo "❌ Found .unwrap() in production code"
    exit 1
fi

# No println! in production
echo "→ Checking for println!..."
if find src -name "*.rs" -not -path "*/test*" -exec grep -l "println!" {} \; | grep -v "// ALLOWED:"; then
    echo "❌ Found println! in production code. Use tracing"
    exit 1
fi

echo "✅ All checks passed!"
```

Install with:
```bash
chmod +x .git/hooks/pre-commit
```

---

## DESIGN SESSION CHECKLIST

Before writing ANY code, answer these questions:

- [ ] What problem are we solving?
- [ ] Which K8s resources do we create/update?
- [ ] What Rollout status fields change?
- [ ] What CDEvents do we emit?
- [ ] What Gateway API changes happen?
- [ ] What's the failure mode?
- [ ] Can we split into <50 line functions?
- [ ] What tests validate this?
- [ ] Draw the state machine

**Example:**
```
Problem: Create canary ReplicaSet
Resources: ReplicaSet (0 replicas)
Status: phase=Progressing, canaryReplicas=0
CDEvents: deployment.progressed
Failures: API error → emit deployment.failed, requeue
Tests: test_reconcile_creates_canary()
State: Initializing → Progressing
```

---

## KUBERNETES CONTROLLER PATTERNS

### Reconciliation Loop
- **Idempotent operations**: Same input → same output
- **Called on every resource change**: Add/Update/Delete
- **Level-triggered, not edge-triggered**: Check desired vs actual state

### Owner References
- **Parent owns children**: Rollout owns ReplicaSets
- **Automatic garbage collection**: Delete Rollout → delete ReplicaSets
- **Cascade deletion**: Kubernetes handles cleanup

### Status Subresource
- **Spec = desired state**: What user wants
- **Status = observed state**: What controller sees
- **Controller reconciles gap**: Drive status → spec

### Label Selectors
- **Group related resources**: All pods for a Rollout
- **Enable querying**: `kubectl get pods -l app=myapp`
- **pod-template-hash pattern**: Track template revisions

---

## TDD EXAMPLE: Complete Walkthrough

### RED Phase: Write Failing Test

```rust
#[tokio::test]
async fn test_reconcile_creates_canary_replicaset() {
    // ARRANGE: Create test Rollout
    let rollout = create_test_rollout("my-app", 3, "nginx:1.0");
    let ctx = Arc::new(Context::new_mock());

    // ACT: Reconcile
    reconcile(Arc::new(rollout.clone()), ctx.clone()).await.unwrap();

    // ASSERT: Canary ReplicaSet exists with 0 replicas
    // (This will FAIL - canary creation not implemented yet)
    let canary = get_replicaset(&ctx.client, "my-app-canary").await.unwrap();
    assert_eq!(canary.spec.unwrap().replicas, Some(0));
}
```

**Run:** `cargo test`  
**Expected:** FAIL ✅ (RED phase confirmed)

---

### GREEN Phase: Minimal Implementation

```rust
pub async fn reconcile(
    rollout: Arc<Rollout>,
    ctx: Arc<Context>,
) -> Result<Action, ReconcileError> {
    let namespace = rollout.namespace().ok_or(ReconcileError::MissingNamespace)?;

    // Existing: Create stable ReplicaSet
    // ... (stable ReplicaSet code)

    // NEW: Create canary ReplicaSet (0 replicas)
    let canary_rs = build_replicaset(&rollout, "canary", 0);
    let canary_rs_name = canary_rs.metadata.name.as_ref().unwrap();

    let rs_api: Api<ReplicaSet> = Api::namespaced(ctx.client.clone(), &namespace);

    match rs_api.get(canary_rs_name).await {
        Ok(_) => info!("Canary ReplicaSet exists"),
        Err(kube::Error::Api(err)) if err.code == 404 => {
            info!("Creating canary ReplicaSet");
            rs_api.create(&PostParams::default(), &canary_rs).await?;
        }
        Err(e) => return Err(ReconcileError::KubeError(e)),
    }

    Ok(Action::requeue(Duration::from_secs(300)))
}
```

**Run:** `cargo test`  
**Expected:** PASS ✅ (GREEN phase confirmed)

---

### REFACTOR Phase: Improve Code

**Extract helper function:**
```rust
async fn ensure_replicaset_exists(
    ctx: &Context,
    namespace: &str,
    rs: &ReplicaSet,
) -> Result<(), ReconcileError> {
    let rs_api: Api<ReplicaSet> = Api::namespaced(ctx.client.clone(), namespace);
    let rs_name = rs.metadata.name.as_ref().unwrap();

    match rs_api.get(rs_name).await {
        Ok(_) => {
            info!(replicaset = ?rs_name, "ReplicaSet exists");
            Ok(())
        }
        Err(kube::Error::Api(err)) if err.code == 404 => {
            info!(replicaset = ?rs_name, "Creating ReplicaSet");
            rs_api.create(&PostParams::default(), rs).await?;
            Ok(())
        }
        Err(e) => Err(ReconcileError::KubeError(e)),
    }
}
```

**Run:** `cargo test`  
**Expected:** Still PASS ✅ (refactor successful)

**Commit:**
```bash
git commit -m "feat: create canary ReplicaSet (0 replicas)

- Add canary ReplicaSet creation after stable
- Extract ensure_replicaset_exists helper
- Idempotent (404 = create, existing = skip)
- Tests: test_reconcile_creates_canary_replicaset passing"
```

