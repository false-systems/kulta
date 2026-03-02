use crate::controller::advisor::{
    resolve_advisor, AdvisorCache, AnalysisAdvisor, AnalysisContext, NoOpAdvisor,
};
use crate::controller::cdevents::emit_status_change_event;
use crate::controller::occurrence::emit_occurrence;
use crate::controller::prometheus::MetricsQuerier;
use crate::crd::rollout::{AdvisorLevel, Phase, Rollout, RolloutStatus};
use crate::server::LeaderState;
use chrono::{DateTime, Utc};
use kube::api::{Api, Patch, PatchParams};
use kube::runtime::controller::Action;
use kube::{Resource, ResourceExt};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, error, info, warn};

use super::status::{
    calculate_requeue_interval_from_rollout, has_promote_annotation, is_progress_deadline_exceeded,
};
use super::validation::{parse_duration, validate_rollout};

#[derive(Debug, Error)]
pub enum ReconcileError {
    #[error("Kubernetes API error: {0}")]
    KubeError(#[from] kube::Error),

    #[error("Rollout missing namespace")]
    MissingNamespace,

    #[error("Rollout missing name")]
    MissingName,

    #[error("ReplicaSet missing name in metadata")]
    ReplicaSetMissingName,

    #[error("Failed to serialize PodTemplateSpec: {0}")]
    SerializationError(String),

    #[error("Invalid Rollout spec: {0}")]
    ValidationError(String),

    #[error("Metrics evaluation failed: {0}")]
    MetricsEvaluationFailed(String),

    #[error("Strategy reconciliation failed: {0}")]
    StrategyError(#[from] crate::controller::strategies::StrategyError),
}

pub struct Context {
    pub client: kube::Client,
    pub cdevents_sink: Arc<dyn crate::controller::cdevents::EventSink>,
    pub prometheus_client: Arc<dyn MetricsQuerier>,
    pub advisor: Arc<dyn AnalysisAdvisor>,
    pub advisor_cache: AdvisorCache,
    pub clock: Arc<dyn crate::controller::clock::Clock>,
    /// Optional leader state for multi-replica deployments
    /// When Some, reconciliation is skipped if not the leader
    pub leader_state: Option<LeaderState>,
    /// Optional controller metrics for Prometheus
    /// When Some, records reconciliation counts and durations
    pub metrics: Option<crate::server::SharedMetrics>,
}

impl Context {
    /// Create a new Context without leader election (single instance mode)
    pub fn new(
        client: kube::Client,
        cdevents_sink: impl crate::controller::cdevents::EventSink + 'static,
        prometheus_client: impl MetricsQuerier + 'static,
        clock: Arc<dyn crate::controller::clock::Clock>,
        metrics: Option<crate::server::SharedMetrics>,
    ) -> Self {
        Context {
            client,
            cdevents_sink: Arc::new(cdevents_sink),
            prometheus_client: Arc::new(prometheus_client),
            advisor: Arc::new(NoOpAdvisor),
            advisor_cache: AdvisorCache::new(),
            clock,
            leader_state: None,
            metrics,
        }
    }

    /// Create a new Context with leader election support
    ///
    /// When leader_state is provided, reconciliation will check if this
    /// instance is the leader before performing any work.
    pub fn new_with_leader(
        client: kube::Client,
        cdevents_sink: impl crate::controller::cdevents::EventSink + 'static,
        prometheus_client: impl MetricsQuerier + 'static,
        clock: Arc<dyn crate::controller::clock::Clock>,
        leader_state: LeaderState,
        metrics: Option<crate::server::SharedMetrics>,
    ) -> Self {
        Context {
            client,
            cdevents_sink: Arc::new(cdevents_sink),
            prometheus_client: Arc::new(prometheus_client),
            advisor: Arc::new(NoOpAdvisor),
            advisor_cache: AdvisorCache::new(),
            clock,
            leader_state: Some(leader_state),
            metrics,
        }
    }

    /// Check if this instance should reconcile
    ///
    /// Returns true if:
    /// - No leader election configured (single instance mode)
    /// - Leader election enabled and this instance is the leader
    pub fn should_reconcile(&self) -> bool {
        match &self.leader_state {
            None => true, // No leader election - always reconcile
            Some(state) => state.is_leader(),
        }
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)] // Test helper - panicking is acceptable
    pub fn new_mock() -> Self {
        // Install ring as the default crypto provider for rustls
        // This is needed because reqwest/kube use rustls, and we need to pick a provider
        // The install_default() call is idempotent - it's safe to call multiple times
        let _ = rustls::crypto::ring::default_provider().install_default();

        // For testing, create a mock client that doesn't require kubeconfig
        // Use a minimal Config - the client won't actually be used in unit tests
        let mut config = kube::Config::new("https://localhost:8080".parse().unwrap());
        config.default_namespace = "default".to_string();
        config.accept_invalid_certs = true;

        let client = kube::Client::try_from(config).unwrap();

        Context {
            client,
            cdevents_sink: Arc::new(crate::controller::cdevents::MockEventSink::new()),
            prometheus_client: Arc::new(crate::controller::prometheus::MockPrometheusClient::new()),
            advisor: Arc::new(NoOpAdvisor),
            advisor_cache: AdvisorCache::new(),
            clock: Arc::new(crate::controller::clock::SystemClock),
            leader_state: None,
            metrics: None,
        }
    }

    /// Create a mock Context with leader election enabled
    ///
    /// Use this instead of direct struct initialization to avoid
    /// maintenance burden when Context fields change.
    #[cfg(test)]
    #[allow(clippy::unwrap_used)] // Test helper - panicking is acceptable
    pub fn new_mock_with_leader(leader_state: LeaderState) -> Self {
        let mock = Self::new_mock();
        Context {
            client: mock.client,
            cdevents_sink: mock.cdevents_sink,
            prometheus_client: mock.prometheus_client,
            advisor: mock.advisor,
            advisor_cache: AdvisorCache::new(),
            clock: mock.clock,
            leader_state: Some(leader_state),
            metrics: None,
        }
    }
}

/// Reconcile a Rollout resource
///
/// Main reconciliation loop that orchestrates progressive delivery:
/// 1. Validates the Rollout spec
/// 2. Selects the appropriate strategy (canary, blue-green, simple, A/B testing)
/// 3. Delegates ReplicaSet and traffic management to the strategy
/// 4. Evaluates Prometheus metrics for automated rollback (if supported)
/// 5. Computes desired status and patches it to K8s
/// 6. Emits CDEvents and FALSE Protocol occurrences for observability
///
/// # Arguments
/// * `rollout` - The Rollout resource to reconcile
/// * `ctx` - Controller context (k8s client, clock, metrics, CDEvents sink)
///
/// # Returns
/// * `Ok(Action)` - Requeue action with interval based on rollout state
/// * `Err(ReconcileError)` - Reconciliation error
pub async fn reconcile(rollout: Arc<Rollout>, ctx: Arc<Context>) -> Result<Action, ReconcileError> {
    // Check if we should reconcile (leader election)
    if !ctx.should_reconcile() {
        // Not the leader - skip reconciliation, requeue later to check again
        debug!(rollout = ?rollout.name_any(), "Skipping reconciliation - not leader");

        // Record skipped metric
        if let Some(ref metrics) = ctx.metrics {
            metrics.record_reconciliation_skipped();
        }

        return Ok(Action::requeue(Duration::from_secs(5)));
    }

    // Start timing for metrics
    let start_time = std::time::Instant::now();

    // Validate rollout has required fields
    let namespace = rollout
        .namespace()
        .ok_or(ReconcileError::MissingNamespace)?;
    let name = rollout.name_any();

    info!(
        rollout = ?name,
        namespace = ?namespace,
        "Reconciling Rollout"
    );

    // Validate Rollout spec (runtime constraints beyond what the CRD schema enforces)
    if let Err(validation_error) = validate_rollout(&rollout) {
        error!(
            rollout = ?name,
            error = ?validation_error,
            "Rollout spec validation failed"
        );
        return Err(ReconcileError::ValidationError(validation_error));
    }

    // Select strategy handler based on rollout spec
    let strategy = crate::controller::strategies::select_strategy(&rollout);
    info!(rollout = ?name, strategy = strategy.name(), "Selected deployment strategy");

    // Reconcile ReplicaSets using strategy-specific logic
    strategy.reconcile_replicasets(&rollout, &ctx).await?;

    // Reconcile traffic routing using strategy-specific logic
    strategy.reconcile_traffic(&rollout, &ctx).await?;

    // Evaluate metrics and trigger rollback if unhealthy (only for strategies that support it)
    if strategy.supports_metrics_analysis() {
        if let Some(current_status) = &rollout.status {
            if current_status.phase == Some(Phase::Progressing) {
                let is_healthy = evaluate_rollout_metrics(&rollout, &ctx).await?;

                // Consult advisor at Level 2+ (advisory only — threshold still decides)
                // Skip if endpoint is not configured to avoid misleading no-op events
                if matches!(
                    rollout.spec.advisor.level,
                    AdvisorLevel::Advised | AdvisorLevel::Planned | AdvisorLevel::Driven
                ) && rollout.spec.advisor.endpoint.is_some()
                {
                    let analysis_ctx = AnalysisContext {
                        rollout_name: name.clone(),
                        namespace: namespace.clone(),
                        strategy: strategy.name().to_string(),
                        current_step: current_status.current_step_index,
                        current_weight: current_status.current_weight,
                        metrics_healthy: is_healthy,
                        phase: current_status
                            .phase
                            .as_ref()
                            .map(|p| format!("{:?}", p))
                            .unwrap_or_else(|| "Unknown".into()),
                        history: current_status
                            .decisions
                            .iter()
                            .map(|d| format!("{}: {:?}", d.timestamp, d.action))
                            .collect(),
                    };

                    let advisor =
                        resolve_advisor(&rollout.spec.advisor, &ctx.advisor, &ctx.advisor_cache);
                    match advisor.advise(&analysis_ctx).await {
                        Ok(recommendation) => {
                            info!(
                                rollout = ?name,
                                advisor_action = ?recommendation.action,
                                confidence = recommendation.confidence,
                                reasoning = %recommendation.reasoning,
                                threshold_healthy = is_healthy,
                                "Advisor recommendation received (threshold decision prevails)"
                            );
                            // Emit advisor recommendation occurrence
                            crate::controller::occurrence::emit_advisor_occurrence(
                                &rollout,
                                strategy.name(),
                                &recommendation,
                                is_healthy,
                                &ctx.clock,
                            );
                        }
                        Err(e) => {
                            warn!(
                                rollout = ?name,
                                error = %e,
                                "Advisor consultation failed, falling back to threshold decision"
                            );
                        }
                    }
                }

                if !is_healthy {
                    warn!(rollout = ?name, "Metrics unhealthy, triggering rollback");

                    let failed_status = RolloutStatus {
                        phase: Some(Phase::Failed),
                        message: Some(
                            "Rollback triggered: metrics exceeded thresholds".to_string(),
                        ),
                        ..current_status.clone()
                    };

                    // Emit rollback CDEvent (non-fatal)
                    if let Err(e) = emit_status_change_event(
                        &rollout,
                        &rollout.status,
                        &failed_status,
                        ctx.cdevents_sink.as_ref(),
                    )
                    .await
                    {
                        warn!(error = ?e, rollout = ?name, "Failed to emit rollback CDEvent (non-fatal)");
                    }

                    // Emit FALSE Protocol occurrence (non-fatal)
                    emit_occurrence(
                        &rollout,
                        Some(&Phase::Progressing),
                        &Phase::Failed,
                        strategy.name(),
                        &ctx.clock,
                    );

                    // Patch status to Failed
                    let rollout_api: Api<Rollout> = Api::namespaced(ctx.client.clone(), &namespace);
                    rollout_api
                        .patch_status(
                            &name,
                            &PatchParams::default(),
                            &Patch::Merge(&serde_json::json!({
                                "status": failed_status
                            })),
                        )
                        .await?;

                    info!(rollout = ?name, "Rollout marked as Failed due to unhealthy metrics");
                    return Ok(Action::requeue(Duration::from_secs(30)));
                }
            }
        }
    }

    // Evaluate A/B experiment for conclusion (only for Experimenting phase)
    if rollout.spec.strategy.ab_testing.is_some() {
        if let Some(current_status) = &rollout.status {
            if current_status.phase == Some(Phase::Experimenting) {
                let evaluation = evaluate_ab_experiment(&rollout, &ctx).await?;

                if evaluation.should_conclude {
                    info!(
                        rollout = ?name,
                        winner = ?evaluation.winner,
                        reason = ?evaluation.reason,
                        "A/B experiment concluding"
                    );

                    // Build concluded status
                    let concluded_status = RolloutStatus {
                        phase: Some(Phase::Concluded),
                        message: Some(format!("A/B experiment concluded: {:?}", evaluation.reason)),
                        ab_experiment: Some(crate::crd::rollout::ABExperimentStatus {
                            started_at: current_status
                                .ab_experiment
                                .as_ref()
                                .map(|ab| ab.started_at.clone())
                                .unwrap_or_else(|| ctx.clock.now().to_rfc3339()),
                            concluded_at: Some(ctx.clock.now().to_rfc3339()),
                            sample_size_a: evaluation.sample_size_a,
                            sample_size_b: evaluation.sample_size_b,
                            results: evaluation.results,
                            winner: evaluation.winner,
                            conclusion_reason: evaluation.reason,
                        }),
                        last_decision_source: None,
                        ..current_status.clone()
                    };

                    // Emit CDEvent (non-fatal)
                    if let Err(e) = emit_status_change_event(
                        &rollout,
                        &rollout.status,
                        &concluded_status,
                        ctx.cdevents_sink.as_ref(),
                    )
                    .await
                    {
                        warn!(error = ?e, rollout = ?name, "Failed to emit A/B concluded CDEvent (non-fatal)");
                    }

                    // Emit FALSE Protocol occurrence (non-fatal)
                    emit_occurrence(
                        &rollout,
                        Some(&Phase::Experimenting),
                        &Phase::Concluded,
                        strategy.name(),
                        &ctx.clock,
                    );

                    // Patch status to Concluded
                    let rollout_api: Api<Rollout> = Api::namespaced(ctx.client.clone(), &namespace);
                    rollout_api
                        .patch_status(
                            &name,
                            &PatchParams::default(),
                            &Patch::Merge(&serde_json::json!({
                                "status": concluded_status
                            })),
                        )
                        .await?;

                    info!(rollout = ?name, "A/B experiment marked as Concluded");
                    return Ok(Action::requeue(Duration::from_secs(30)));
                }
            }
        }
    }

    // Check progress deadline (for Progressing or Preview phases with deadline configured)
    if let Some(deadline_seconds) = rollout.spec.progress_deadline_seconds {
        if let Some(current_status) = &rollout.status {
            if (current_status.phase == Some(Phase::Progressing)
                || current_status.phase == Some(Phase::Preview))
                && is_progress_deadline_exceeded(current_status, deadline_seconds, ctx.clock.now())
            {
                warn!(
                    rollout = ?name,
                    deadline_seconds = deadline_seconds,
                    "Progress deadline exceeded, marking rollout as Failed"
                );

                let failed_status = RolloutStatus {
                    phase: Some(Phase::Failed),
                    message: Some(format!(
                        "Progress deadline exceeded: no progress made in {} seconds",
                        deadline_seconds
                    )),
                    ..current_status.clone()
                };

                // Emit rollback CDEvent (non-fatal)
                if let Err(e) = emit_status_change_event(
                    &rollout,
                    &rollout.status,
                    &failed_status,
                    ctx.cdevents_sink.as_ref(),
                )
                .await
                {
                    warn!(error = ?e, rollout = ?name, "Failed to emit deadline exceeded CDEvent (non-fatal)");
                }

                // Emit FALSE Protocol occurrence (non-fatal)
                let old_phase = current_status.phase.as_ref().unwrap_or(&Phase::Progressing);
                emit_occurrence(
                    &rollout,
                    Some(old_phase),
                    &Phase::Failed,
                    strategy.name(),
                    &ctx.clock,
                );

                // Patch status to Failed
                let rollout_api: Api<Rollout> = Api::namespaced(ctx.client.clone(), &namespace);
                rollout_api
                    .patch_status(
                        &name,
                        &PatchParams::default(),
                        &Patch::Merge(&serde_json::json!({
                            "status": failed_status
                        })),
                    )
                    .await?;

                info!(
                    rollout = ?name,
                    "Rollout marked as Failed due to progress deadline exceeded"
                );

                // Record metrics for the failure
                if let Some(ref metrics) = ctx.metrics {
                    let duration_secs = start_time.elapsed().as_secs_f64();
                    metrics.record_reconciliation_error(&name, duration_secs);
                }

                return Ok(Action::requeue(Duration::from_secs(30)));
            }
        }
    }

    // Check for promote annotation before computing status (avoid race condition)
    let had_promote_annotation = has_promote_annotation(&rollout);
    let was_paused_before = rollout
        .status
        .as_ref()
        .map(|s| s.phase == Some(Phase::Paused))
        .unwrap_or(false);

    // Compute desired status using strategy-specific logic
    let desired_status = strategy.compute_next_status(&rollout, ctx.clock.now());

    // Determine if we progressed due to the annotation
    let progressed_due_to_annotation = had_promote_annotation
        && was_paused_before
        && rollout.status.as_ref() != Some(&desired_status);

    // Update Rollout status if it changed
    if rollout.status.as_ref() != Some(&desired_status) {
        info!(
            rollout = ?name,
            current_step = ?desired_status.current_step_index,
            current_weight = ?desired_status.current_weight,
            phase = ?desired_status.phase,
            "Updating Rollout status"
        );

        // Emit CDEvent (non-fatal)
        if let Err(e) = emit_status_change_event(
            &rollout,
            &rollout.status,
            &desired_status,
            ctx.cdevents_sink.as_ref(),
        )
        .await
        {
            warn!(error = ?e, rollout = ?name, "Failed to emit CDEvent (non-fatal)");
        }

        // Emit FALSE Protocol occurrence (non-fatal)
        let old_phase = rollout.status.as_ref().and_then(|s| s.phase.as_ref());
        if let Some(new_phase) = &desired_status.phase {
            emit_occurrence(&rollout, old_phase, new_phase, strategy.name(), &ctx.clock);
        }

        // Patch status subresource
        let rollout_api: Api<Rollout> = Api::namespaced(ctx.client.clone(), &namespace);

        match rollout_api
            .patch_status(
                &name,
                &PatchParams::default(),
                &Patch::Merge(&serde_json::json!({
                    "status": desired_status
                })),
            )
            .await
        {
            Ok(_) => {
                info!(rollout = ?name, "Status updated successfully");

                // Remove promote annotation if it was used for progression
                if progressed_due_to_annotation {
                    info!(
                        rollout = ?name,
                        "Removing promote annotation after successful promotion"
                    );

                    match rollout_api
                        .patch(
                            &name,
                            &PatchParams::default(),
                            &Patch::Merge(&serde_json::json!({
                                "metadata": {
                                    "annotations": {
                                        "kulta.io/promote": serde_json::Value::Null
                                    }
                                }
                            })),
                        )
                        .await
                    {
                        Ok(_) => {
                            info!(rollout = ?name, "Promote annotation removed successfully")
                        }
                        Err(e) => {
                            warn!(error = ?e, rollout = ?name, "Failed to remove promote annotation (non-fatal)")
                        }
                    }
                }
            }
            Err(e) => {
                error!(error = ?e, rollout = ?name, "Failed to update status");
                return Err(ReconcileError::KubeError(e));
            }
        }
    }

    // Calculate requeue interval and return
    let requeue_interval =
        calculate_requeue_interval_from_rollout(&rollout, &desired_status, ctx.clock.now());

    // Record success metrics
    if let Some(ref metrics) = ctx.metrics {
        let duration_secs = start_time.elapsed().as_secs_f64();
        metrics.record_reconciliation_success(strategy.name(), duration_secs);

        // Update traffic weight gauge
        if let Some(weight) = desired_status.current_weight {
            metrics.set_traffic_weight(&namespace, &name, weight as i64);
        }
    }

    Ok(Action::requeue(requeue_interval))
}

/// Evaluate rollout metrics against Prometheus thresholds
///
/// Checks if the canary revision is healthy based on the analysis config.
/// Returns Ok(true) if healthy, Ok(false) if unhealthy.
///
/// # Arguments
/// * `rollout` - The Rollout to evaluate
/// * `ctx` - Controller context with PrometheusClient
///
/// # Returns
/// * `Ok(true)` - All metrics healthy (or no analysis config)
/// * `Ok(false)` - One or more metrics unhealthy
/// * `Err(_)` - Query execution failed
pub(crate) async fn evaluate_rollout_metrics(
    rollout: &Rollout,
    ctx: &Context,
) -> Result<bool, ReconcileError> {
    // Check if rollout has canary strategy with analysis config
    let analysis_config = match &rollout.spec.strategy.canary {
        Some(canary_strategy) => match &canary_strategy.analysis {
            Some(analysis) => analysis,
            None => {
                // No analysis config - consider healthy (no constraints)
                return Ok(true);
            }
        },
        None => {
            // No canary strategy - no metrics to check
            return Ok(true);
        }
    };

    // Check if warmup period has elapsed
    if let Some(warmup_str) = &analysis_config.warmup_duration {
        if let Some(warmup_duration) = parse_duration(warmup_str) {
            // Get step start time from status, or fall back to rollout creation time
            let step_start_time = rollout
                .status
                .as_ref()
                .and_then(|s| s.step_start_time.as_ref())
                .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .or_else(|| rollout.meta().creation_timestamp.as_ref().map(|t| t.0));

            if let Some(start_time) = step_start_time {
                let now = ctx.clock.now();
                let elapsed = now.signed_duration_since(start_time);
                let warmup_duration_secs = warmup_duration.as_secs() as i64;

                if elapsed.num_seconds() < warmup_duration_secs {
                    // Still in warmup period - skip analysis, consider healthy
                    let remaining = warmup_duration_secs - elapsed.num_seconds();
                    debug!(
                        rollout = rollout.name_any(),
                        warmup_remaining_secs = remaining,
                        "Skipping metrics analysis - warmup period not elapsed"
                    );
                    return Ok(true);
                }
            } else {
                // Warmup is configured but step_start_time is missing or invalid.
                // Treat this as if warmup just started: skip analysis for now.
                warn!(
                    rollout = rollout.name_any(),
                    "Warmup duration is configured but step_start_time is missing or invalid; skipping metrics analysis and treating warmup as just started"
                );
                return Ok(true);
            }
        }
    }

    // Get rollout name for Prometheus labels
    let rollout_name = rollout.name_any();

    // Evaluate all metrics
    let is_healthy = ctx
        .prometheus_client
        .evaluate_all_metrics(&analysis_config.metrics, &rollout_name, "canary")
        .await
        .map_err(|e| ReconcileError::MetricsEvaluationFailed(e.to_string()))?;

    Ok(is_healthy)
}

/// Result of A/B experiment evaluation
#[derive(Debug, Clone)]
pub struct ABExperimentEvaluation {
    /// Should the experiment conclude?
    pub should_conclude: bool,
    /// Winner if concluded, or None for timeout/inconclusive
    pub winner: Option<crate::crd::rollout::ABVariant>,
    /// Reason for conclusion
    pub reason: Option<crate::crd::rollout::ABConclusionReason>,
    /// Metric results for status update
    pub results: Vec<crate::crd::rollout::ABMetricResult>,
    /// Sample sizes
    pub sample_size_a: Option<i64>,
    pub sample_size_b: Option<i64>,
}

/// Evaluate A/B experiment for conclusion conditions
///
/// Checks duration constraints and statistical significance.
/// Returns evaluation result with conclusion decision.
///
/// # Arguments
/// * `rollout` - The Rollout with A/B testing strategy
/// * `ctx` - Controller context with Prometheus client
///
/// # Returns
/// * `Ok(ABExperimentEvaluation)` - Evaluation result
/// * `Err(_)` - Evaluation failed
pub async fn evaluate_ab_experiment(
    rollout: &Rollout,
    ctx: &Context,
) -> Result<ABExperimentEvaluation, ReconcileError> {
    use crate::controller::prometheus_ab::{determine_experiment_conclusion, evaluate_ab_metrics};
    use crate::crd::rollout::{ABConclusionReason, ABMetricDirection};

    // Get A/B strategy config
    let ab_strategy = match &rollout.spec.strategy.ab_testing {
        Some(ab) => ab,
        None => {
            return Ok(ABExperimentEvaluation {
                should_conclude: false,
                winner: None,
                reason: None,
                results: vec![],
                sample_size_a: None,
                sample_size_b: None,
            });
        }
    };

    // Check for manual conclude annotation
    if rollout
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("kulta.io/conclude-experiment"))
        .is_some()
    {
        info!(
            rollout = rollout.name_any(),
            "Manual experiment conclusion requested"
        );
        return Ok(ABExperimentEvaluation {
            should_conclude: true,
            winner: None, // User decides winner via promote
            reason: Some(ABConclusionReason::ManualConclusion),
            results: vec![],
            sample_size_a: None,
            sample_size_b: None,
        });
    }

    // Get experiment start time
    let started_at = rollout
        .status
        .as_ref()
        .and_then(|s| s.ab_experiment.as_ref())
        .and_then(|ab| DateTime::parse_from_rfc3339(&ab.started_at).ok())
        .map(|dt| dt.with_timezone(&Utc));

    let elapsed = started_at.map(|start| ctx.clock.now().signed_duration_since(start));

    // Check max_duration (safety timeout)
    if let Some(max_duration_str) = &ab_strategy.max_duration {
        if let Some(max_duration) = parse_duration(max_duration_str) {
            if let Some(elapsed_duration) = elapsed {
                if elapsed_duration.num_seconds() >= max_duration.as_secs() as i64 {
                    warn!(
                        rollout = rollout.name_any(),
                        max_duration = ?max_duration_str,
                        "A/B experiment max duration exceeded"
                    );
                    return Ok(ABExperimentEvaluation {
                        should_conclude: true,
                        winner: None, // No winner - timeout
                        reason: Some(ABConclusionReason::MaxDurationExceeded),
                        results: vec![],
                        sample_size_a: None,
                        sample_size_b: None,
                    });
                }
            }
        }
    }

    // Get analysis config
    let analysis_config = match &ab_strategy.analysis {
        Some(analysis) => analysis,
        None => {
            // No analysis config - can't evaluate statistically
            return Ok(ABExperimentEvaluation {
                should_conclude: false,
                winner: None,
                reason: None,
                results: vec![],
                sample_size_a: None,
                sample_size_b: None,
            });
        }
    };

    // Check min_duration (don't evaluate too early)
    if let Some(min_duration_str) = &analysis_config.min_duration {
        if let Some(min_duration) = parse_duration(min_duration_str) {
            if let Some(elapsed_duration) = elapsed {
                if elapsed_duration.num_seconds() < min_duration.as_secs() as i64 {
                    debug!(
                        rollout = rollout.name_any(),
                        min_duration = ?min_duration_str,
                        elapsed_secs = elapsed_duration.num_seconds(),
                        "A/B experiment min duration not reached - skipping analysis"
                    );
                    return Ok(ABExperimentEvaluation {
                        should_conclude: false,
                        winner: None,
                        reason: None,
                        results: vec![],
                        sample_size_a: None,
                        sample_size_b: None,
                    });
                }
            }
        }
    }

    // Query Prometheus for variant metrics
    let service_a = &ab_strategy.variant_a_service;
    let service_b = &ab_strategy.variant_b_service;

    // Query sample counts — return early if Prometheus is unreachable
    let inconclusive = ABExperimentEvaluation {
        should_conclude: false,
        winner: None,
        reason: None,
        results: vec![],
        sample_size_a: None,
        sample_size_b: None,
    };

    let sample_a = match ctx.prometheus_client.query_ab_sample_count(service_a).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, service = %service_a, rollout = rollout.name_any(),
                "Failed to query A/B sample count for variant A");
            return Ok(inconclusive);
        }
    };
    let sample_b = match ctx.prometheus_client.query_ab_sample_count(service_b).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, service = %service_b, rollout = rollout.name_any(),
                "Failed to query A/B sample count for variant B");
            return Ok(inconclusive);
        }
    };

    // Check minimum sample size
    let min_samples = analysis_config.min_sample_size.unwrap_or(30) as i64;
    if sample_a < min_samples || sample_b < min_samples {
        debug!(
            rollout = rollout.name_any(),
            sample_a = sample_a,
            sample_b = sample_b,
            min_samples = min_samples,
            "Insufficient samples for A/B analysis"
        );
        return Ok(ABExperimentEvaluation {
            should_conclude: false,
            winner: None,
            reason: None,
            results: vec![],
            sample_size_a: Some(sample_a),
            sample_size_b: Some(sample_b),
        });
    }

    // Query error rates for both variants
    let rate_a = match ctx.prometheus_client.query_ab_error_rate(service_a).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, service = %service_a, rollout = rollout.name_any(),
                "Failed to query A/B error rate for variant A");
            return Ok(ABExperimentEvaluation {
                should_conclude: false,
                winner: None,
                reason: None,
                results: vec![],
                sample_size_a: Some(sample_a),
                sample_size_b: Some(sample_b),
            });
        }
    };
    let rate_b = match ctx.prometheus_client.query_ab_error_rate(service_b).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, service = %service_b, rollout = rollout.name_any(),
                "Failed to query A/B error rate for variant B");
            return Ok(ABExperimentEvaluation {
                should_conclude: false,
                winner: None,
                reason: None,
                results: vec![],
                sample_size_a: Some(sample_a),
                sample_size_b: Some(sample_b),
            });
        }
    };

    // Get confidence level (default 0.95)
    let confidence_level = analysis_config.confidence_level.unwrap_or(0.95);

    // Build metrics for evaluation
    // For now, use error-rate as the primary metric
    let metrics_data: Vec<(String, f64, f64, i64, i64, ABMetricDirection)> = vec![(
        "error-rate".to_string(),
        rate_a,
        rate_b,
        sample_a,
        sample_b,
        ABMetricDirection::Lower, // Lower error rate is better
    )];

    // Run statistical analysis
    let results = evaluate_ab_metrics(&metrics_data, confidence_level);

    // Determine conclusion
    let conclusion = determine_experiment_conclusion(&results);

    match conclusion {
        Some((winner, reason)) => {
            info!(
                rollout = rollout.name_any(),
                winner = ?winner,
                reason = ?reason,
                "A/B experiment concluded with statistical significance"
            );
            Ok(ABExperimentEvaluation {
                should_conclude: true,
                winner: Some(winner),
                reason: Some(reason),
                results,
                sample_size_a: Some(sample_a),
                sample_size_b: Some(sample_b),
            })
        }
        None => Ok(ABExperimentEvaluation {
            should_conclude: false,
            winner: None,
            reason: None,
            results,
            sample_size_a: Some(sample_a),
            sample_size_b: Some(sample_b),
        }),
    }
}
