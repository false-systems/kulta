//! v1alpha1 CRD types
//!
//! Re-exports from rollout.rs for versioned access.
//! This is the original API version.

pub use super::rollout::{
    AnalysisConfig, BlueGreenStrategy, CanaryStep, CanaryStrategy, Decision, DecisionAction,
    DecisionReason, FailurePolicy, GatewayAPIRouting, MetricConfig, MetricSnapshot, PauseDuration,
    Phase, PrometheusConfig, Rollout, RolloutSpec, RolloutStatus, RolloutStrategy, SimpleStrategy,
    TrafficRouting,
};
