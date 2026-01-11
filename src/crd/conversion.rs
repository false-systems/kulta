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
pub const DEFAULT_MAX_UNAVAILABLE: &str = "0";

/// Default progressDeadlineSeconds when converting from v1alpha1
pub const DEFAULT_PROGRESS_DEADLINE_SECONDS: i32 = 600;

/// Convert v1alpha1 RolloutSpec to v1beta1
///
/// Uses existing v1beta1 field values if present (for round-trip preservation),
/// otherwise adds default values:
/// - maxSurge: "25%"
/// - maxUnavailable: "0"
/// - progressDeadlineSeconds: 600
pub fn convert_to_v1beta1(spec: &v1alpha1::RolloutSpec) -> v1beta1::RolloutSpec {
    v1beta1::RolloutSpec {
        replicas: spec.replicas,
        selector: spec.selector.clone(),
        template: spec.template.clone(),
        strategy: spec.strategy.clone(),
        // Use existing values if present, otherwise use defaults
        max_surge: spec
            .max_surge
            .clone()
            .or_else(|| Some(DEFAULT_MAX_SURGE.to_string())),
        max_unavailable: spec
            .max_unavailable
            .clone()
            .or_else(|| Some(DEFAULT_MAX_UNAVAILABLE.to_string())),
        progress_deadline_seconds: spec
            .progress_deadline_seconds
            .or(Some(DEFAULT_PROGRESS_DEADLINE_SECONDS)),
    }
}

/// Convert v1beta1 RolloutSpec to v1alpha1
///
/// v1alpha1 spec now includes v1beta1 fields as optional for internal use.
/// We preserve these fields to avoid data loss during round-trip conversion.
pub fn convert_to_v1alpha1(spec: &v1beta1::RolloutSpec) -> v1alpha1::RolloutSpec {
    v1alpha1::RolloutSpec {
        replicas: spec.replicas,
        selector: spec.selector.clone(),
        template: spec.template.clone(),
        strategy: spec.strategy.clone(),
        // Preserve v1beta1 fields to avoid data loss in round-trip conversion
        max_surge: spec.max_surge.clone(),
        max_unavailable: spec.max_unavailable.clone(),
        progress_deadline_seconds: spec.progress_deadline_seconds,
    }
}

#[cfg(test)]
#[path = "conversion_test.rs"]
mod tests;
