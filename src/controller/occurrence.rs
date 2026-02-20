//! FALSE Protocol occurrence emission for KULTA
//!
//! FALSE Protocol (Functional AI-native Semantic Events) is the observability
//! format used across False Systems tools (AHTI, SYKLI, NOPEA, etc.). Unlike
//! CDEvents which target CI/CD interoperability, FALSE Protocol occurrences
//! embed AI-consumable context (error blocks, reasoning, history) that enable
//! cross-tool correlation by AHTI.
//!
//! KULTA emits both CDEvents (standard) and FALSE Protocol occurrences (AHTI integration).

use crate::controller::clock::Clock;
use crate::crd::rollout::{Phase, Rollout};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

/// FALSE Protocol occurrence
#[derive(Debug, Serialize)]
pub struct Occurrence {
    pub id: String,
    pub timestamp: String,
    pub source: String,
    #[serde(rename = "type")]
    pub occurrence_type: String,
    pub severity: Severity,
    pub outcome: Outcome,
    pub context: OccurrenceContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<OccurrenceError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<History>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub data: HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub entities: Vec<Entity>,
}

/// Severity levels
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

/// Outcome of the occurrence
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
    Failure,
    Timeout,
    InProgress,
    Unknown,
}

/// Occurrence context
#[derive(Debug, Serialize)]
pub struct OccurrenceContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub correlation_keys: Vec<CorrelationKey>,
}

/// Correlation key for cross-tool linking
#[derive(Debug, Serialize)]
pub struct CorrelationKey {
    #[serde(rename = "type")]
    pub key_type: String,
    pub value: String,
}

/// AI-native error block
#[derive(Debug, Default, Serialize)]
pub struct OccurrenceError {
    pub code: String,
    pub what_failed: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why_it_matters: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub possible_causes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_fix: Option<String>,
}

/// AI-native reasoning block
#[derive(Debug, Serialize)]
pub struct Reasoning {
    pub summary: String,
    pub explanation: String,
    pub confidence: f64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub recommendations: Vec<String>,
}

/// History of steps taken
#[derive(Debug, Serialize)]
pub struct History {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub steps: Vec<HistoryStep>,
}

/// Individual step in history
#[derive(Debug, Serialize)]
pub struct HistoryStep {
    pub description: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Entity reference with version tracking
#[derive(Debug, Serialize)]
pub struct Entity {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub observed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_of_truth: Option<String>,
}

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
    let occurrence = build_occurrence(rollout, old_phase, new_phase, strategy, now);

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

/// Build an occurrence from rollout state
fn build_occurrence(
    rollout: &Rollout,
    old_phase: Option<&Phase>,
    new_phase: &Phase,
    strategy: &str,
    now: DateTime<Utc>,
) -> Occurrence {
    // name and namespace are validated by emit_occurrence before calling this
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
        Some(OccurrenceError {
            code: "ROLLOUT_FAILED".to_string(),
            what_failed: format!("Rollout {} failed during {} deployment", name, strategy),
            why_it_matters: Some(format!(
                "Service {} in namespace {} may be serving degraded traffic",
                name, namespace
            )),
            possible_causes: vec![message],
            suggested_fix: Some(format!(
                "Check metrics for {} and consider manual rollback",
                name
            )),
        })
    } else {
        None
    };

    Occurrence {
        id: ulid::Ulid::new().to_string(),
        timestamp: now.to_rfc3339(),
        source: "kulta".to_string(),
        occurrence_type: occurrence_type.to_string(),
        severity,
        outcome,
        context: OccurrenceContext {
            cluster: std::env::var("KULTA_CLUSTER_NAME").ok(),
            namespace: Some(namespace.to_string()),
            correlation_keys: vec![
                CorrelationKey {
                    key_type: "deployment".to_string(),
                    value: name.to_string(),
                },
                CorrelationKey {
                    key_type: "namespace".to_string(),
                    value: namespace.to_string(),
                },
            ],
        },
        error,
        reasoning: None, // AHTI adds reasoning, not KULTA
        history: None,
        data,
        entities: vec![Entity {
            entity_type: "rollout".to_string(),
            id: uid.to_string(),
            name: name.to_string(),
            version: resource_version.to_string(),
            observed_at: now.to_rfc3339(),
            namespace: Some(namespace.to_string()),
            source_of_truth: Some("k8s-api".to_string()),
        }],
    }
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
        // Concluded maps to completed
        assert_eq!(
            phase_to_occurrence_suffix(Some(&Phase::Experimenting), &Phase::Concluded),
            "completed"
        );
        // Paused from non-Progressing phase still maps to paused
        assert_eq!(
            phase_to_occurrence_suffix(Some(&Phase::Preview), &Phase::Paused),
            "paused"
        );
    }

    #[test]
    fn test_build_occurrence_type_strategy_prefixes() {
        // Canary strategy
        assert_eq!(
            build_occurrence_type("canary", None, &Phase::Progressing),
            "canary.rollout.progressing"
        );
        // Blue-green strategy
        assert_eq!(
            build_occurrence_type("blue_green", None, &Phase::Completed),
            "bluegreen.rollout.completed"
        );
        // A/B testing strategy
        assert_eq!(
            build_occurrence_type("ab_testing", Some(&Phase::Experimenting), &Phase::Failed),
            "abtesting.rollout.failed"
        );
        // Simple strategy
        assert_eq!(
            build_occurrence_type("simple", None, &Phase::Completed),
            "rolling.rollout.completed"
        );
    }

    #[test]
    fn test_build_occurrence_progressing() {
        let rollout = test_rollout();
        let now = Utc::now();

        let occ = build_occurrence(&rollout, None, &Phase::Progressing, "canary", now);

        assert_eq!(occ.source, "kulta");
        assert_eq!(occ.occurrence_type, "canary.rollout.progressing");
        assert!(matches!(occ.severity, Severity::Info));
        assert!(matches!(occ.outcome, Outcome::InProgress));
        assert!(occ.error.is_none());
        assert_eq!(occ.entities.len(), 1);
        assert_eq!(occ.entities[0].name, "my-app");
        assert_eq!(occ.entities[0].version, "rv-456");
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
        );

        assert_eq!(occ.occurrence_type, "canary.rollout.failed");
        assert!(matches!(occ.severity, Severity::Error));
        assert!(matches!(occ.outcome, Outcome::Failure));
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

        let occ = build_occurrence(&rollout, None, &Phase::Completed, "simple", now);
        let json = serde_json::to_string(&occ).expect("Should serialize");

        // Verify key fields present in JSON
        assert!(json.contains("\"source\":\"kulta\""));
        assert!(json.contains("\"type\":\"rolling.rollout.completed\""));
        assert!(json.contains("\"severity\":\"info\""));
        assert!(json.contains("\"outcome\":\"success\""));
        // No error block for success
        assert!(!json.contains("\"error\""));
        // No reasoning (AHTI's job)
        assert!(!json.contains("\"reasoning\""));
    }

    #[test]
    fn test_occurrence_id_is_ulid() {
        let rollout = test_rollout();
        let now = Utc::now();

        let occ = build_occurrence(&rollout, None, &Phase::Progressing, "canary", now);

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
        rollout.metadata = ObjectMeta::default(); // no name, namespace, uid, resource_version
        let now = Utc::now();

        let occ = build_occurrence(&rollout, None, &Phase::Progressing, "canary", now);

        assert_eq!(occ.entities[0].name, "unknown");
        assert_eq!(occ.context.namespace.as_deref(), Some("unknown"));
        assert_eq!(occ.entities[0].id, ""); // uid defaults to ""
        assert_eq!(occ.entities[0].version, "0"); // resource_version defaults to "0"
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
        // Initializing maps to "progressing" (catch-all)
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
        );

        let err = occ.error.as_ref().unwrap();
        assert!(err.possible_causes[0].contains("High error rate"));
    }
}
