//! FALSE Protocol occurrence emission for KULTA
//!
//! FALSE Protocol (Functional AI-native Semantic Events) is the observability
//! format used across False Systems tools (AHTI, SYKLI, NOPEA, etc.). Unlike
//! CDEvents which target CI/CD interoperability, FALSE Protocol occurrences
//! embed AI-consumable context (error blocks, reasoning, history) that enable
//! cross-tool correlation by AHTI.
//!
//! KULTA emits both CDEvents (standard) and FALSE Protocol occurrences (AHTI integration).
//!
//! Types are provided by the `false-protocol` crate — KULTA only contains
//! the mapping logic from rollout state to occurrences.

use crate::controller::clock::Clock;
use crate::crd::rollout::{Phase, Recommendation, Rollout};
use chrono::{DateTime, Utc};
use false_protocol::{Entity, Error as OccurrenceError, Occurrence, Outcome, Severity};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

/// Map phase transition to occurrence type suffix
///
/// Returns just the action suffix (e.g., "failed", "completed").
/// The caller prepends the strategy prefix to form the full type
/// (e.g., "canary.rollout.failed", "bluegreen.rollout.paused").
fn phase_to_occurrence_suffix(_old_phase: Option<&Phase>, new_phase: &Phase) -> &'static str {
    match new_phase {
        Phase::Failed => "failed",
        Phase::Completed | Phase::Concluded => "completed",
        Phase::Paused => "paused",
        _ => "progressing",
    }
}

/// Build the full occurrence type from strategy name and phase transition
///
/// Maps strategy names to FALSE Protocol type prefixes:
/// - "canary" → "canary.rollout.*"
/// - "blue_green" → "bluegreen.rollout.*"
/// - "ab_testing" → "abtesting.rollout.*"
/// - "simple" → "rolling.rollout.*"
fn build_occurrence_type(strategy: &str, old_phase: Option<&Phase>, new_phase: &Phase) -> String {
    let prefix = match strategy {
        "blue_green" => "bluegreen",
        "ab_testing" => "abtesting",
        "simple" => "rolling",
        other => other, // "canary" passes through
    };
    let suffix = phase_to_occurrence_suffix(old_phase, new_phase);
    format!("{}.rollout.{}", prefix, suffix)
}

/// Map phase transition to severity
fn phase_to_severity(new_phase: &Phase) -> Severity {
    match new_phase {
        Phase::Failed => Severity::Error,
        Phase::Paused => Severity::Warning,
        Phase::Completed | Phase::Concluded => Severity::Info,
        _ => Severity::Info,
    }
}

/// Map phase transition to outcome
fn phase_to_outcome(new_phase: &Phase) -> Outcome {
    match new_phase {
        Phase::Failed => Outcome::Failure,
        Phase::Completed | Phase::Concluded => Outcome::Success,
        _ => Outcome::InProgress,
    }
}

/// Emit a FALSE Protocol occurrence for a rollout phase transition
///
/// Writes the occurrence as JSON (one line per occurrence) to the directory
/// specified by `KULTA_OCCURRENCE_DIR` env var (default: `/tmp/kulta`).
/// Non-fatal: logs a warning on failure but never fails reconciliation.
pub fn emit_occurrence(
    rollout: &Rollout,
    old_phase: Option<&Phase>,
    new_phase: &Phase,
    strategy: &str,
    clock: &Arc<dyn Clock>,
) {
    let name = match rollout.metadata.name.as_deref() {
        Some(n) => n,
        None => {
            warn!("Occurrence emission skipped: rollout missing name");
            return;
        }
    };
    let namespace = match rollout.metadata.namespace.as_deref() {
        Some(ns) => ns,
        None => {
            warn!("Occurrence emission skipped: rollout missing namespace");
            return;
        }
    };
    let now = clock.now();
    let occurrence = match build_occurrence(rollout, old_phase, new_phase, strategy, now) {
        Some(occ) => occ,
        None => return,
    };

    let json = match serde_json::to_string(&occurrence) {
        Ok(j) => j,
        Err(e) => {
            warn!(error = %e, rollout = %name, namespace = %namespace,
                "Failed to serialize FALSE Protocol occurrence");
            return;
        }
    };

    if let Err(e) = write_occurrence(&json) {
        warn!(error = %e, rollout = %name, namespace = %namespace,
            "Failed to write FALSE Protocol occurrence (non-fatal)");
    }
}

/// Build an occurrence from rollout state.
///
/// Returns `None` if the crate's validation rejects the occurrence type
/// (should not happen with well-formed strategy names, but we never
/// fail reconciliation on occurrence emission).
fn build_occurrence(
    rollout: &Rollout,
    old_phase: Option<&Phase>,
    new_phase: &Phase,
    strategy: &str,
    now: DateTime<Utc>,
) -> Option<Occurrence> {
    let name = rollout.metadata.name.as_deref().unwrap_or("unknown");
    let namespace = rollout.metadata.namespace.as_deref().unwrap_or("unknown");
    let uid = rollout.metadata.uid.as_deref().unwrap_or("");
    let resource_version = rollout.metadata.resource_version.as_deref().unwrap_or("0");

    let occurrence_type = build_occurrence_type(strategy, old_phase, new_phase);
    let severity = phase_to_severity(new_phase);
    let outcome = phase_to_outcome(new_phase);

    let mut data = HashMap::new();
    data.insert(
        "rollout".to_string(),
        serde_json::json!({
            "name": name,
            "namespace": namespace,
            "strategy": strategy,
            "replicas": rollout.spec.replicas,
            "current_weight": rollout.status.as_ref().and_then(|s| s.current_weight),
            "phase": format!("{:?}", new_phase),
        }),
    );

    let error = if matches!(new_phase, Phase::Failed) {
        let message = rollout
            .status
            .as_ref()
            .and_then(|s| s.message.clone())
            .unwrap_or_else(|| "Rollout failed".to_string());

        let current_weight = rollout.status.as_ref().and_then(|s| s.current_weight);
        let current_step = rollout.status.as_ref().and_then(|s| s.current_step_index);

        // Build rich context: what_failed includes traffic context
        let what_failed = match (current_weight, current_step) {
            (Some(weight), Some(step)) => format!(
                "{} for {} {} at step {}/{} ({}% traffic)",
                message,
                name,
                strategy,
                step + 1,
                rollout
                    .spec
                    .strategy
                    .canary
                    .as_ref()
                    .map(|c| c.steps.len())
                    .unwrap_or(0),
                weight,
            ),
            _ => format!("Rollout {} failed during {} deployment", name, strategy),
        };

        // Richer possible_causes based on failure message
        let mut possible_causes = vec![message.clone()];
        if message.contains("metrics exceeded") || message.contains("error rate") {
            possible_causes.push(format!("New code path in {} handlers", name));
            possible_causes.push("Downstream service degradation".to_string());
        }
        if message.contains("deadline exceeded") {
            possible_causes.push("Pods failing readiness probes".to_string());
            possible_causes.push("Image pull failures or resource constraints".to_string());
        }

        let suggested_fix = if message.contains("metrics exceeded") {
            format!(
                "Rollback {} to stable, check dependent service health, review recent changes",
                name
            )
        } else {
            format!(
                "Check metrics and pod logs for {}, consider manual rollback",
                name
            )
        };

        Some(OccurrenceError {
            code: "ROLLOUT_FAILED".to_string(),
            what_failed,
            why_it_matters: Some(format!(
                "Service {} in namespace {} may be serving degraded traffic to {}% of requests",
                name,
                namespace,
                current_weight.unwrap_or(0),
            )),
            possible_causes,
            suggested_fix: Some(suggested_fix),
            ..Default::default()
        })
    } else {
        None
    };

    let mut entity = Entity::from_k8s("rollout", uid, name, namespace, resource_version);
    entity.observed_at = now;

    let mut occ = match Occurrence::new("kulta", &occurrence_type) {
        Ok(o) => o,
        Err(errs) => {
            warn!(
                errors = ?errs,
                occurrence_type = %occurrence_type,
                "Failed to construct FALSE Protocol occurrence (non-fatal)"
            );
            return None;
        }
    };

    occ.timestamp = now;
    occ = occ
        .severity(severity)
        .outcome(outcome)
        .in_namespace(namespace)
        .correlate("deployment", name)
        .correlate("namespace", namespace)
        .with_entity(entity)
        .with_data(data);

    if let Ok(cluster) = std::env::var("KULTA_CLUSTER_NAME") {
        occ = occ.in_cluster(&cluster);
    }

    if let Some(err) = error {
        occ = occ.with_error(err);
    }

    Some(occ)
}

/// Get the occurrence output directory.
///
/// Uses `KULTA_OCCURRENCE_DIR` env var if set, otherwise defaults to `/tmp/kulta`.
fn occurrence_dir() -> std::path::PathBuf {
    std::env::var("KULTA_OCCURRENCE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/kulta"))
}

/// Maximum occurrence file size (10 MB). Truncated when exceeded.
const MAX_OCCURRENCE_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// Write occurrence JSON to file (one JSON line per occurrence)
///
/// Truncates the file when it exceeds 10 MB to prevent unbounded growth.
fn write_occurrence(json: &str) -> std::io::Result<()> {
    use std::io::Write;

    let dir = occurrence_dir();
    std::fs::create_dir_all(&dir)?;

    let file_path = dir.join("occurrence.json");

    // Truncate if file exceeds size limit to prevent unbounded growth
    if let Ok(metadata) = std::fs::metadata(&file_path) {
        if metadata.len() > MAX_OCCURRENCE_FILE_BYTES {
            warn!("Occurrence file exceeds 10MB, truncating");
            std::fs::write(&file_path, "")?;
        }
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)?;

    writeln!(file, "{}", json)?;
    Ok(())
}

/// Emit a FALSE Protocol occurrence for an advisor consultation (Level 2+)
///
/// Emits `{strategy}.advisor.recommendation` events that record what the
/// advisor recommended alongside the threshold decision.
pub fn emit_advisor_occurrence(
    rollout: &Rollout,
    strategy: &str,
    recommendation: &Recommendation,
    threshold_healthy: bool,
    clock: &Arc<dyn Clock>,
) {
    let name = match rollout.metadata.name.as_deref() {
        Some(n) => n,
        None => return,
    };
    let namespace = match rollout.metadata.namespace.as_deref() {
        Some(ns) => ns,
        None => return,
    };
    let uid = rollout.metadata.uid.as_deref().unwrap_or("");
    let resource_version = rollout.metadata.resource_version.as_deref().unwrap_or("0");
    let now = clock.now();

    let prefix = match strategy {
        "blue_green" => "bluegreen",
        "ab_testing" => "abtesting",
        "simple" => "rolling",
        other => other,
    };
    let occurrence_type = format!("{}.advisor.recommendation", prefix);

    let mut occ = match Occurrence::new("kulta", &occurrence_type) {
        Ok(o) => o,
        Err(errs) => {
            warn!(errors = ?errs, "Failed to construct advisor occurrence (non-fatal)");
            return;
        }
    };

    let mut data = HashMap::new();
    data.insert(
        "advisor".to_string(),
        serde_json::json!({
            "action": recommendation.action,
            "confidence": recommendation.confidence,
            "reasoning": recommendation.reasoning,
            "threshold_healthy": threshold_healthy,
            "threshold_prevails": true,
        }),
    );

    let mut entity = Entity::from_k8s("rollout", uid, name, namespace, resource_version);
    entity.observed_at = now;

    occ.timestamp = now;
    occ = occ
        .severity(Severity::Info)
        .outcome(Outcome::InProgress)
        .in_namespace(namespace)
        .correlate("deployment", name)
        .correlate("namespace", namespace)
        .with_entity(entity)
        .with_data(data);

    if let Ok(cluster) = std::env::var("KULTA_CLUSTER_NAME") {
        occ = occ.in_cluster(&cluster);
    }

    let json = match serde_json::to_string(&occ) {
        Ok(j) => j,
        Err(e) => {
            warn!(error = %e, "Failed to serialize advisor occurrence (non-fatal)");
            return;
        }
    };

    if let Err(e) = write_occurrence(&json) {
        warn!(error = %e, "Failed to write advisor occurrence (non-fatal)");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::controller::clock::MockClock;
    use crate::crd::rollout::{Rollout, RolloutSpec, RolloutStrategy};
    use k8s_openapi::api::core::v1::{Container, PodSpec, PodTemplateSpec};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
    use kube::api::ObjectMeta;

    fn test_rollout() -> Rollout {
        Rollout {
            metadata: ObjectMeta {
                name: Some("my-app".to_string()),
                namespace: Some("production".to_string()),
                uid: Some("uid-123".to_string()),
                resource_version: Some("rv-456".to_string()),
                ..Default::default()
            },
            spec: RolloutSpec {
                replicas: 3,
                selector: LabelSelector::default(),
                template: PodTemplateSpec {
                    spec: Some(PodSpec {
                        containers: vec![Container {
                            name: "app".to_string(),
                            image: Some("nginx:1.21".to_string()),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                strategy: RolloutStrategy {
                    canary: None,
                    blue_green: None,
                    simple: None,
                    ab_testing: None,
                },
                max_surge: None,
                max_unavailable: None,
                progress_deadline_seconds: None,
                advisor: Default::default(),
            },
            status: None,
        }
    }

    #[test]
    fn test_phase_to_occurrence_suffix() {
        assert_eq!(
            phase_to_occurrence_suffix(None, &Phase::Progressing),
            "progressing"
        );
        assert_eq!(
            phase_to_occurrence_suffix(Some(&Phase::Progressing), &Phase::Completed),
            "completed"
        );
        assert_eq!(
            phase_to_occurrence_suffix(Some(&Phase::Progressing), &Phase::Failed),
            "failed"
        );
        assert_eq!(
            phase_to_occurrence_suffix(Some(&Phase::Progressing), &Phase::Paused),
            "paused"
        );
        assert_eq!(
            phase_to_occurrence_suffix(Some(&Phase::Experimenting), &Phase::Concluded),
            "completed"
        );
        assert_eq!(
            phase_to_occurrence_suffix(Some(&Phase::Preview), &Phase::Paused),
            "paused"
        );
    }

    #[test]
    fn test_build_occurrence_type_strategy_prefixes() {
        assert_eq!(
            build_occurrence_type("canary", None, &Phase::Progressing),
            "canary.rollout.progressing"
        );
        assert_eq!(
            build_occurrence_type("blue_green", None, &Phase::Completed),
            "bluegreen.rollout.completed"
        );
        assert_eq!(
            build_occurrence_type("ab_testing", Some(&Phase::Experimenting), &Phase::Failed),
            "abtesting.rollout.failed"
        );
        assert_eq!(
            build_occurrence_type("simple", None, &Phase::Completed),
            "rolling.rollout.completed"
        );
    }

    #[test]
    fn test_build_occurrence_progressing() {
        let rollout = test_rollout();
        let now = Utc::now();

        let occ = build_occurrence(&rollout, None, &Phase::Progressing, "canary", now).unwrap();

        assert_eq!(occ.source, "kulta");
        assert_eq!(occ.occurrence_type, "canary.rollout.progressing");
        assert_eq!(occ.severity, Severity::Info);
        assert_eq!(occ.outcome, Outcome::InProgress);
        assert!(occ.error.is_none());
        assert_eq!(occ.context.entities.len(), 1);
        assert_eq!(occ.context.entities[0].name, "my-app");
        assert_eq!(occ.context.entities[0].version, "rv-456");
        assert_eq!(occ.context.namespace.as_deref(), Some("production"));
    }

    #[test]
    fn test_build_occurrence_failed_has_error() {
        let rollout = test_rollout();
        let now = Utc::now();

        let occ = build_occurrence(
            &rollout,
            Some(&Phase::Progressing),
            &Phase::Failed,
            "canary",
            now,
        )
        .unwrap();

        assert_eq!(occ.occurrence_type, "canary.rollout.failed");
        assert_eq!(occ.severity, Severity::Error);
        assert_eq!(occ.outcome, Outcome::Failure);
        assert!(occ.error.is_some());
        let err = occ.error.as_ref().unwrap();
        assert_eq!(err.code, "ROLLOUT_FAILED");
        assert!(err.what_failed.contains("my-app"));
        assert!(err.why_it_matters.is_some());
    }

    #[test]
    fn test_occurrence_json_serialization() {
        let rollout = test_rollout();
        let now = Utc::now();

        let occ = build_occurrence(&rollout, None, &Phase::Completed, "simple", now).unwrap();
        let json = serde_json::to_string(&occ).expect("Should serialize");

        assert!(json.contains("\"source\":\"kulta\""));
        assert!(json.contains("\"type\":\"rolling.rollout.completed\""));
        assert!(json.contains("\"severity\":\"info\""));
        assert!(json.contains("\"outcome\":\"success\""));
        assert!(json.contains("\"protocol_version\":\"1.0\""));
        // No error block for success
        assert!(!json.contains("\"error\""));
        // No reasoning (AHTI's job)
        assert!(!json.contains("\"reasoning\""));
    }

    #[test]
    fn test_occurrence_id_is_ulid() {
        let rollout = test_rollout();
        let now = Utc::now();

        let occ = build_occurrence(&rollout, None, &Phase::Progressing, "canary", now).unwrap();

        // ULID is 26 characters, uppercase alphanumeric
        assert_eq!(occ.id.len(), 26);
    }

    #[test]
    fn test_emit_occurrence_with_mock_clock() {
        let rollout = test_rollout();
        let fixed_time = Utc::now();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(fixed_time));

        // Just verify it doesn't panic - file write may fail in test env
        emit_occurrence(&rollout, None, &Phase::Progressing, "canary", &clock);
    }

    #[test]
    fn test_build_occurrence_with_missing_metadata() {
        let mut rollout = test_rollout();
        rollout.metadata = ObjectMeta::default();
        let now = Utc::now();

        let occ = build_occurrence(&rollout, None, &Phase::Progressing, "canary", now).unwrap();

        assert_eq!(occ.context.entities[0].name, "unknown");
        assert_eq!(occ.context.namespace.as_deref(), Some("unknown"));
        assert_eq!(occ.context.entities[0].id, "");
        assert_eq!(occ.context.entities[0].version, "0");
    }

    #[test]
    fn test_emit_occurrence_skips_missing_name() {
        let mut rollout = test_rollout();
        rollout.metadata.name = None;
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(Utc::now()));

        // Should not panic — just logs a warning and returns
        emit_occurrence(&rollout, None, &Phase::Progressing, "canary", &clock);
    }

    #[test]
    fn test_emit_occurrence_skips_missing_namespace() {
        let mut rollout = test_rollout();
        rollout.metadata.namespace = None;
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(Utc::now()));

        // Should not panic — logs warning and returns
        emit_occurrence(&rollout, None, &Phase::Progressing, "canary", &clock);
    }

    #[test]
    fn test_phase_to_occurrence_suffix_initializing() {
        assert_eq!(
            phase_to_occurrence_suffix(None, &Phase::Initializing),
            "progressing"
        );
    }

    #[test]
    fn test_phase_to_occurrence_suffix_experimenting() {
        assert_eq!(
            phase_to_occurrence_suffix(None, &Phase::Experimenting),
            "progressing"
        );
    }

    #[test]
    fn test_build_occurrence_failed_with_custom_message() {
        let mut rollout = test_rollout();
        rollout.status = Some(crate::crd::rollout::RolloutStatus {
            message: Some("High error rate detected: 15% > 5% threshold".to_string()),
            ..Default::default()
        });
        let now = Utc::now();

        let occ = build_occurrence(
            &rollout,
            Some(&Phase::Progressing),
            &Phase::Failed,
            "canary",
            now,
        )
        .unwrap();

        let err = occ.error.as_ref().unwrap();
        assert!(err.possible_causes[0].contains("High error rate"));
    }

    #[test]
    fn test_build_occurrence_failed_with_metrics_exceeded_has_rich_context() {
        use crate::crd::rollout::{
            CanaryStep, CanaryStrategy, RolloutStatus, RolloutStrategy as RolloutStrategySpec,
        };

        let mut rollout = test_rollout();
        rollout.spec.strategy = RolloutStrategySpec {
            canary: Some(CanaryStrategy {
                canary_service: "my-app-canary".into(),
                stable_service: "my-app-stable".into(),
                port: None,
                steps: vec![
                    CanaryStep {
                        set_weight: Some(20),
                        pause: None,
                    },
                    CanaryStep {
                        set_weight: Some(50),
                        pause: None,
                    },
                    CanaryStep {
                        set_weight: Some(100),
                        pause: None,
                    },
                ],
                traffic_routing: None,
                analysis: None,
            }),
            blue_green: None,
            simple: None,
            ab_testing: None,
        };
        rollout.status = Some(RolloutStatus {
            phase: Some(Phase::Progressing),
            current_weight: Some(20),
            current_step_index: Some(0),
            message: Some("Rollback triggered: metrics exceeded thresholds".into()),
            ..Default::default()
        });

        let now = Utc::now();
        let occ = build_occurrence(
            &rollout,
            Some(&Phase::Progressing),
            &Phase::Failed,
            "canary",
            now,
        )
        .unwrap();

        let err = occ.error.as_ref().unwrap();
        // Rich what_failed includes step and weight context
        assert!(
            err.what_failed.contains("step 1/3"),
            "what_failed: {}",
            err.what_failed
        );
        assert!(
            err.what_failed.contains("20% traffic"),
            "what_failed: {}",
            err.what_failed
        );
        // Multiple possible causes for metrics failures
        assert!(err.possible_causes.len() > 1);
        // why_it_matters includes traffic percentage
        assert!(err.why_it_matters.as_ref().unwrap().contains("20%"));
        // suggested_fix mentions rollback
        assert!(err.suggested_fix.as_ref().unwrap().contains("Rollback"));
    }

    #[test]
    fn test_build_occurrence_failed_deadline_exceeded_has_rich_context() {
        let mut rollout = test_rollout();
        rollout.status = Some(crate::crd::rollout::RolloutStatus {
            message: Some("Progress deadline exceeded: no progress made in 600 seconds".into()),
            ..Default::default()
        });

        let now = Utc::now();
        let occ = build_occurrence(
            &rollout,
            Some(&Phase::Progressing),
            &Phase::Failed,
            "canary",
            now,
        )
        .unwrap();

        let err = occ.error.as_ref().unwrap();
        assert!(err
            .possible_causes
            .iter()
            .any(|c| c.contains("readiness probes")));
    }

    #[test]
    fn test_emit_advisor_occurrence_does_not_panic() {
        use crate::crd::rollout::{Recommendation, RecommendedAction};

        let rollout = test_rollout();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::new(Utc::now()));

        let recommendation = Recommendation {
            action: RecommendedAction::Continue,
            confidence: 0.85,
            reasoning: "metrics look healthy, no anomalies detected".into(),
        };

        // Should not panic even if file write fails in test env
        emit_advisor_occurrence(&rollout, "canary", &recommendation, true, &clock);
    }
}
