//! v1beta1 CRD types
//!
//! Evolution of v1alpha1 with additional fields:
//! - maxSurge: Controls how many extra pods can be created during rollout
//! - maxUnavailable: Controls how many pods can be unavailable during rollout
//! - progressDeadlineSeconds: Timeout for rollout progress

use k8s_openapi::api::core::v1::PodTemplateSpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Re-export unchanged types from v1alpha1
pub use super::rollout::{
    AnalysisConfig, BlueGreenStrategy, CanaryStep, CanaryStrategy, Decision, DecisionAction,
    DecisionReason, FailurePolicy, GatewayAPIRouting, MetricConfig, MetricSnapshot, PauseDuration,
    Phase, PrometheusConfig, RolloutStatus, RolloutStrategy, SimpleStrategy, TrafficRouting,
};

/// Rollout v1beta1 - Progressive delivery with enhanced rollout controls
///
/// New in v1beta1:
/// - maxSurge: Maximum number of pods above desired during rollout
/// - maxUnavailable: Maximum number of unavailable pods during rollout
/// - progressDeadlineSeconds: Timeout for detecting stuck rollouts
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "kulta.io",
    version = "v1beta1",
    kind = "Rollout",
    namespaced,
    status = "RolloutStatus",
    printcolumn = r#"{"name":"Desired", "type":"integer", "jsonPath":".spec.replicas"}"#,
    printcolumn = r#"{"name":"Current", "type":"integer", "jsonPath":".status.replicas"}"#,
    printcolumn = r#"{"name":"Ready", "type":"integer", "jsonPath":".status.readyReplicas"}"#,
    printcolumn = r#"{"name":"Phase", "type":"string", "jsonPath":".status.phase"}"#,
    printcolumn = r#"{"name":"Weight", "type":"integer", "jsonPath":".status.currentWeight"}"#,
    printcolumn = r#"{"name":"Age", "type":"date", "jsonPath":".metadata.creationTimestamp"}"#
)]
pub struct RolloutSpec {
    /// Number of desired pods
    #[serde(default = "default_replicas")]
    pub replicas: i32,

    /// Label selector for pods
    pub selector: LabelSelector,

    /// Template describes the pods that will be created
    pub template: PodTemplateSpec,

    /// Deployment strategy
    pub strategy: RolloutStrategy,

    // === NEW IN v1beta1 ===
    /// Maximum number of pods that can be scheduled above the desired number during update.
    /// Value can be an absolute number (e.g., "5") or percentage (e.g., "25%").
    /// Defaults to "25%".
    #[serde(rename = "maxSurge", skip_serializing_if = "Option::is_none")]
    pub max_surge: Option<String>,

    /// Maximum number of pods that can be unavailable during the update.
    /// Value can be an absolute number (e.g., "1") or percentage (e.g., "25%").
    /// Defaults to "0".
    #[serde(rename = "maxUnavailable", skip_serializing_if = "Option::is_none")]
    pub max_unavailable: Option<String>,

    /// Maximum time in seconds for a rollout to make progress before it is considered failed.
    /// Defaults to 600 (10 minutes).
    #[serde(
        rename = "progressDeadlineSeconds",
        skip_serializing_if = "Option::is_none"
    )]
    pub progress_deadline_seconds: Option<i32>,
}

fn default_replicas() -> i32 {
    1
}
