//! CRD version conversion between v1alpha1 and v1beta1
//!
//! Provides bidirectional conversion for the Rollout CRD.
//!
//! ## Conversion rules:
//! - v1alpha1 -> v1beta1: Add defaults for new fields (maxSurge, maxUnavailable, progressDeadlineSeconds)
//! - v1beta1 -> v1alpha1: Drop new fields (they don't exist in v1alpha1)

use super::v1alpha1;
use super::v1beta1;

/// Default maxSurge value when converting from v1alpha1
pub const DEFAULT_MAX_SURGE: &str = "25%";

/// Default maxUnavailable value when converting from v1alpha1
pub const DEFAULT_MAX_UNAVAILABLE: i32 = 0;

/// Default progressDeadlineSeconds when converting from v1alpha1
pub const DEFAULT_PROGRESS_DEADLINE_SECONDS: i32 = 600;

/// Convert v1alpha1 RolloutSpec to v1beta1
///
/// Adds default values for fields new in v1beta1:
/// - maxSurge: "25%"
/// - maxUnavailable: 0
/// - progressDeadlineSeconds: 600
pub fn convert_to_v1beta1(spec: &v1alpha1::RolloutSpec) -> v1beta1::RolloutSpec {
    v1beta1::RolloutSpec {
        replicas: spec.replicas,
        selector: spec.selector.clone(),
        template: spec.template.clone(),
        strategy: spec.strategy.clone(),
        // New fields get defaults
        max_surge: Some(DEFAULT_MAX_SURGE.to_string()),
        max_unavailable: Some(DEFAULT_MAX_UNAVAILABLE),
        progress_deadline_seconds: Some(DEFAULT_PROGRESS_DEADLINE_SECONDS),
    }
}

/// Convert v1beta1 RolloutSpec to v1alpha1
///
/// Drops fields that don't exist in v1alpha1:
/// - maxSurge
/// - maxUnavailable
/// - progressDeadlineSeconds
pub fn convert_to_v1alpha1(spec: &v1beta1::RolloutSpec) -> v1alpha1::RolloutSpec {
    v1alpha1::RolloutSpec {
        replicas: spec.replicas,
        selector: spec.selector.clone(),
        template: spec.template.clone(),
        strategy: spec.strategy.clone(),
        // New v1beta1 fields are simply dropped
    }
}

#[cfg(test)]
#[path = "conversion_test.rs"]
mod tests;
