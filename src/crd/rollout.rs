use k8s_openapi::api::core::v1::PodTemplateSpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Rollout is a Custom Resource for managing progressive delivery
///
/// Compatible with Argo Rollouts API for easy migration
#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "kulta.io",
    version = "v1alpha1",
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

    /// Deployment strategy (currently only canary)
    pub strategy: RolloutStrategy,

    // === v1beta1 fields (optional for v1alpha1 compatibility) ===
    /// Maximum number of pods that can be scheduled above the desired number during update.
    /// Value can be an absolute number (e.g., "5") or percentage (e.g., "25%").
    /// Defaults to "25%" when not specified.
    #[serde(rename = "maxSurge", skip_serializing_if = "Option::is_none")]
    pub max_surge: Option<String>,

    /// Maximum number of pods that can be unavailable during the update.
    /// Value can be an absolute number (e.g., "1") or percentage (e.g., "25%").
    /// Defaults to "0" when not specified.
    #[serde(rename = "maxUnavailable", skip_serializing_if = "Option::is_none")]
    pub max_unavailable: Option<String>,

    /// Maximum time in seconds for a rollout to make progress before it is considered failed.
    /// Defaults to 600 (10 minutes) when not specified.
    #[serde(
        rename = "progressDeadlineSeconds",
        skip_serializing_if = "Option::is_none"
    )]
    pub progress_deadline_seconds: Option<i32>,
}

fn default_replicas() -> i32 {
    1
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, JsonSchema)]
pub struct RolloutStrategy {
    /// Simple deployment strategy (rolling update with observability)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simple: Option<SimpleStrategy>,

    /// Canary deployment strategy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canary: Option<CanaryStrategy>,

    /// Blue-Green deployment strategy
    #[serde(rename = "blueGreen", skip_serializing_if = "Option::is_none")]
    pub blue_green: Option<BlueGreenStrategy>,

    /// A/B Testing deployment strategy
    #[serde(rename = "abTesting", skip_serializing_if = "Option::is_none")]
    pub ab_testing: Option<ABStrategy>,
}

/// Simple deployment strategy
///
/// Standard Kubernetes rolling update with CDEvents observability.
/// No traffic splitting - just deploy, monitor metrics, and emit events.
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct SimpleStrategy {
    /// Analysis configuration for automated metrics-based rollback
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis: Option<AnalysisConfig>,
}

/// Blue-Green deployment strategy
///
/// Maintains two full environments (active and preview).
/// Traffic is 100% to active until promotion, then instant switch to preview.
/// No gradual traffic shifting - instant cutover.
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct BlueGreenStrategy {
    /// Name of the service that selects active pods (receives production traffic)
    #[serde(rename = "activeService")]
    pub active_service: String,

    /// Name of the service that selects preview pods (for testing before promotion)
    #[serde(rename = "previewService")]
    pub preview_service: String,

    /// Service port for traffic routing (default: 80)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,

    /// Whether to automatically promote after autoPromotionSeconds
    #[serde(
        rename = "autoPromotionEnabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub auto_promotion_enabled: Option<bool>,

    /// Seconds to wait before auto-promoting (if autoPromotionEnabled)
    #[serde(
        rename = "autoPromotionSeconds",
        skip_serializing_if = "Option::is_none"
    )]
    pub auto_promotion_seconds: Option<i32>,

    /// Traffic routing configuration
    #[serde(rename = "trafficRouting", skip_serializing_if = "Option::is_none")]
    pub traffic_routing: Option<TrafficRouting>,

    /// Analysis configuration for automated metrics-based rollback
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis: Option<AnalysisConfig>,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct CanaryStrategy {
    /// Name of the service that selects canary pods
    #[serde(rename = "canaryService")]
    pub canary_service: String,

    /// Name of the service that selects stable pods
    #[serde(rename = "stableService")]
    pub stable_service: String,

    /// Service port for traffic routing (default: 80)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,

    /// Steps define the canary rollout progression
    #[serde(default)]
    pub steps: Vec<CanaryStep>,

    /// Traffic routing configuration
    #[serde(rename = "trafficRouting", skip_serializing_if = "Option::is_none")]
    pub traffic_routing: Option<TrafficRouting>,

    /// Analysis configuration for automated metrics-based rollback
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis: Option<AnalysisConfig>,
}

/// A/B Testing deployment strategy
///
/// Routes users based on headers or cookies to different variants.
/// Unlike canary (weight-based), A/B testing uses deterministic routing.
/// Both variants run at full capacity for fair comparison.
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct ABStrategy {
    /// Name of the service that receives variant-a traffic (control group)
    #[serde(rename = "variantAService")]
    pub variant_a_service: String,

    /// Name of the service that receives variant-b traffic (experiment group)
    #[serde(rename = "variantBService")]
    pub variant_b_service: String,

    /// Service port for traffic routing (default: 80)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,

    /// Match conditions for routing to variant B
    /// Requests matching these conditions go to variant B; others go to variant A
    #[serde(rename = "variantBMatch")]
    pub variant_b_match: ABMatch,

    /// Traffic routing configuration (Gateway API HTTPRoute)
    #[serde(rename = "trafficRouting", skip_serializing_if = "Option::is_none")]
    pub traffic_routing: Option<TrafficRouting>,

    /// Maximum experiment duration before auto-conclusion (safety limit)
    /// Format: "24h", "7d", etc.
    #[serde(rename = "maxDuration", skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<String>,

    /// Analysis configuration for statistical comparison
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis: Option<ABAnalysisConfig>,
}

/// Match conditions for A/B routing to variant B
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct ABMatch {
    /// Header-based matching (e.g., X-Variant: B)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<ABHeaderMatch>,

    /// Cookie-based matching (e.g., ab_variant=B)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookie: Option<ABCookieMatch>,
}

/// Header-based match for A/B routing
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct ABHeaderMatch {
    /// Header name (e.g., "X-Variant")
    pub name: String,

    /// Header value to match (e.g., "B")
    pub value: String,

    /// Match type: Exact (default) or RegularExpression
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub match_type: Option<ABMatchType>,
}

/// Cookie-based match for A/B routing
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct ABCookieMatch {
    /// Cookie name (e.g., "ab_variant")
    pub name: String,

    /// Cookie value to match (e.g., "B")
    pub value: String,
}

/// Match type for header/cookie matching
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, JsonSchema)]
pub enum ABMatchType {
    #[default]
    Exact,
    RegularExpression,
}

/// Analysis configuration for A/B statistical comparison
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct ABAnalysisConfig {
    /// Prometheus configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prometheus: Option<PrometheusConfig>,

    /// Metrics to compare between variants
    #[serde(default)]
    pub metrics: Vec<ABMetricConfig>,

    /// Minimum experiment duration before statistical evaluation starts
    /// Ensures sufficient data collection (e.g., "1h", "30m")
    #[serde(rename = "minDuration", skip_serializing_if = "Option::is_none")]
    pub min_duration: Option<String>,

    /// Minimum sample size per variant before evaluation
    /// Prevents premature conclusions
    #[serde(rename = "minSampleSize", skip_serializing_if = "Option::is_none")]
    pub min_sample_size: Option<i32>,

    /// Statistical confidence level (default: 0.95)
    #[serde(rename = "confidenceLevel", skip_serializing_if = "Option::is_none")]
    pub confidence_level: Option<f64>,
}

/// Metric configuration for A/B comparison
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct ABMetricConfig {
    /// Metric name/template (error-rate, latency-p95, conversion-rate)
    pub name: String,

    /// Direction: "lower" (B should be lower) or "higher" (B should be higher)
    /// Determines which variant "wins" if statistically significant
    pub direction: ABMetricDirection,

    /// Minimum effect size to consider meaningful (optional)
    /// E.g., 0.05 means B must be at least 5% better
    #[serde(rename = "minEffectSize", skip_serializing_if = "Option::is_none")]
    pub min_effect_size: Option<f64>,
}

/// Direction for metric comparison in A/B testing
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub enum ABMetricDirection {
    /// Variant B should have lower value (e.g., error rate, latency)
    #[serde(rename = "lower")]
    Lower,
    /// Variant B should have higher value (e.g., conversion rate)
    #[serde(rename = "higher")]
    Higher,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct CanaryStep {
    /// Set the percentage of traffic to route to canary
    #[serde(rename = "setWeight", skip_serializing_if = "Option::is_none")]
    pub set_weight: Option<i32>,

    /// Pause the rollout
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause: Option<PauseDuration>,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct PauseDuration {
    /// Duration in seconds (e.g., "30s", "5m")
    /// If not specified, pauses indefinitely until manually resumed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct TrafficRouting {
    /// Gateway API configuration (KULTA-specific)
    #[serde(rename = "gatewayAPI", skip_serializing_if = "Option::is_none")]
    pub gateway_api: Option<GatewayAPIRouting>,
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct GatewayAPIRouting {
    /// Name of the HTTPRoute to manipulate
    #[serde(rename = "httpRoute")]
    pub http_route: String,
}

/// What to do when Prometheus is unreachable during analysis
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, JsonSchema)]
pub enum FailurePolicy {
    /// Pause rollout until Prometheus recovers (default, safest)
    #[default]
    Pause,
    /// Proceed without metrics (risky)
    Continue,
    /// Treat as failure, rollback immediately
    Rollback,
}

/// Analysis configuration for automated rollback based on metrics
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct AnalysisConfig {
    /// Prometheus configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prometheus: Option<PrometheusConfig>,

    /// What to do when Prometheus is unreachable
    #[serde(rename = "failurePolicy", skip_serializing_if = "Option::is_none")]
    pub failure_policy: Option<FailurePolicy>,

    /// Warmup duration before starting metrics analysis (e.g., "1m", "30s")
    #[serde(rename = "warmupDuration", skip_serializing_if = "Option::is_none")]
    pub warmup_duration: Option<String>,

    /// List of metrics to monitor
    #[serde(default)]
    pub metrics: Vec<MetricConfig>,
}

/// Prometheus configuration
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct PrometheusConfig {
    /// Prometheus server address (e.g., "http://prometheus:9090")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// Metric configuration for analysis
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct MetricConfig {
    /// Metric name/template (error-rate, latency-p95, latency-p99)
    pub name: String,

    /// Threshold value (metric must be below this)
    pub threshold: f64,

    /// Check interval (e.g., "30s", "1m")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,

    /// Number of consecutive failures before rollback
    #[serde(rename = "failureThreshold", skip_serializing_if = "Option::is_none")]
    pub failure_threshold: Option<i32>,

    /// Minimum sample size required for metric evaluation
    #[serde(rename = "minSampleSize", skip_serializing_if = "Option::is_none")]
    pub min_sample_size: Option<i32>,
}

/// Phase of a Rollout
///
/// Represents the current lifecycle stage of the rollout
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, JsonSchema)]
pub enum Phase {
    /// Initial phase when rollout is being set up
    #[default]
    Initializing,
    /// Rollout is actively progressing through canary steps
    Progressing,
    /// Rollout is paused waiting for manual promotion or duration
    Paused,
    /// Blue-green: Preview environment ready, awaiting promotion
    Preview,
    /// A/B testing: Experiment is active, collecting data
    Experimenting,
    /// A/B testing: Experiment concluded (significance reached or max duration)
    Concluded,
    /// Rollout successfully completed (100% canary or promoted blue-green)
    Completed,
    /// Rollout failed and requires manual intervention
    Failed,
}

/// Action taken by the controller
///
/// Represents what the controller decided to do at a given point
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub enum DecisionAction {
    /// Initial setup of the rollout
    Initialize,
    /// Advance to the next canary step
    StepAdvance,
    /// Manual promotion triggered
    Promotion,
    /// Rollback to stable version
    Rollback,
    /// Pause the rollout
    Pause,
    /// Resume from paused state
    Resume,
    /// Rollout completed successfully
    Complete,
}

/// Reason for the decision
///
/// Explains why a particular action was taken
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub enum DecisionReason {
    /// Metrics analysis passed thresholds
    AnalysisPassed,
    /// Metrics analysis failed thresholds
    AnalysisFailed,
    /// Configured pause duration has elapsed
    PauseDurationExpired,
    /// User triggered manual promotion
    ManualPromotion,
    /// User triggered manual rollback
    ManualRollback,
    /// Operation timed out
    Timeout,
    /// Initial rollout setup
    Initialization,
}

/// Metric snapshot at decision time
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct MetricSnapshot {
    pub value: f64,
    pub threshold: f64,
    pub passed: bool,
}

/// Decision record for observability
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct Decision {
    pub timestamp: String,
    pub action: DecisionAction,
    #[serde(rename = "fromStep", skip_serializing_if = "Option::is_none")]
    pub from_step: Option<i32>,
    #[serde(rename = "toStep", skip_serializing_if = "Option::is_none")]
    pub to_step: Option<i32>,
    pub reason: DecisionReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<std::collections::HashMap<String, MetricSnapshot>>,
}

/// Status of the Rollout
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct RolloutStatus {
    /// Total number of non-terminated pods
    #[serde(default)]
    pub replicas: i32,

    /// Number of ready replicas
    #[serde(rename = "readyReplicas", default)]
    pub ready_replicas: i32,

    /// Number of updated replicas (canary)
    #[serde(rename = "updatedReplicas", default)]
    pub updated_replicas: i32,

    /// Current canary step index (0-indexed)
    #[serde(rename = "currentStepIndex", skip_serializing_if = "Option::is_none")]
    pub current_step_index: Option<i32>,

    /// Current canary weight percentage
    #[serde(rename = "currentWeight", skip_serializing_if = "Option::is_none")]
    pub current_weight: Option<i32>,

    /// Phase of the rollout (Initializing, Progressing, Paused, Completed, Failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,

    /// Human-readable message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Timestamp when current pause started (RFC3339 format)
    #[serde(rename = "pauseStartTime", skip_serializing_if = "Option::is_none")]
    pub pause_start_time: Option<String>,

    /// Timestamp when current step started (RFC3339 format)
    /// Used for warmup duration tracking before metrics analysis begins
    #[serde(rename = "stepStartTime", skip_serializing_if = "Option::is_none")]
    pub step_start_time: Option<String>,

    /// Timestamp when rollout started progressing (RFC3339 format)
    /// Used for progressDeadlineSeconds timeout detection
    #[serde(rename = "progressStartedAt", skip_serializing_if = "Option::is_none")]
    pub progress_started_at: Option<String>,

    /// Decision history for observability
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,

    /// A/B experiment status (only for abTesting strategy)
    #[serde(rename = "abExperiment", skip_serializing_if = "Option::is_none")]
    pub ab_experiment: Option<ABExperimentStatus>,
}

/// A/B experiment status tracking
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct ABExperimentStatus {
    /// When the experiment started (RFC3339)
    #[serde(rename = "startedAt")]
    pub started_at: String,

    /// When the experiment concluded (RFC3339), if concluded
    #[serde(rename = "concludedAt", skip_serializing_if = "Option::is_none")]
    pub concluded_at: Option<String>,

    /// Current sample count for variant A
    #[serde(rename = "sampleSizeA", skip_serializing_if = "Option::is_none")]
    pub sample_size_a: Option<i64>,

    /// Current sample count for variant B
    #[serde(rename = "sampleSizeB", skip_serializing_if = "Option::is_none")]
    pub sample_size_b: Option<i64>,

    /// Statistical results per metric
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<ABMetricResult>,

    /// Overall winner (if concluded with significance)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner: Option<ABVariant>,

    /// Reason the experiment concluded
    #[serde(rename = "conclusionReason", skip_serializing_if = "Option::is_none")]
    pub conclusion_reason: Option<ABConclusionReason>,
}

/// Result for a single A/B metric comparison
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct ABMetricResult {
    /// Metric name
    pub name: String,

    /// Value for variant A
    #[serde(rename = "valueA")]
    pub value_a: f64,

    /// Value for variant B
    #[serde(rename = "valueB")]
    pub value_b: f64,

    /// Statistical confidence level achieved (0.0 to 1.0)
    pub confidence: f64,

    /// Whether the difference is statistically significant
    #[serde(rename = "isSignificant")]
    pub is_significant: bool,

    /// Which variant won for this metric, or None if inconclusive
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner: Option<ABVariant>,
}

/// A/B experiment variant identifier
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub enum ABVariant {
    /// Variant A (control group)
    A,
    /// Variant B (experiment group)
    B,
}

/// Reason for A/B experiment conclusion
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub enum ABConclusionReason {
    /// Statistical significance reached for all metrics
    SignificanceReached,
    /// Maximum experiment duration exceeded
    MaxDurationExceeded,
    /// Manual conclusion via annotation
    ManualConclusion,
    /// Consensus reached (all metrics show same winner)
    ConsensusReached,
}

#[cfg(test)]
#[path = "rollout_test.rs"]
mod tests;
