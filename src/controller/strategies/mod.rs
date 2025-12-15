//! Strategy pattern for rollout reconciliation
//!
//! This module defines the RolloutStrategy trait and implementations for each deployment strategy:
//! - SimpleStrategy: Standard rolling update with observability
//! - CanaryStrategy: Progressive traffic shifting with gradual rollout
//! - BlueGreenStrategy: Instant cutover between two full environments

pub mod blue_green;
pub mod canary;
pub mod simple;

use crate::controller::rollout::Context;
use crate::crd::rollout::{Rollout, RolloutStatus};
use async_trait::async_trait;
use thiserror::Error;

/// Errors specific to strategy reconciliation
#[derive(Debug, Error)]
pub enum StrategyError {
    #[error("Failed to reconcile ReplicaSets: {0}")]
    ReplicaSetReconciliationFailed(String),

    #[error("Failed to reconcile traffic routing: {0}")]
    TrafficReconciliationFailed(String),

    #[error("Kubernetes API error: {0}")]
    KubeError(#[from] kube::Error),

    #[error("Missing required field: {0}")]
    MissingField(String),
}

/// Strategy trait for different rollout types
///
/// Each deployment strategy (Simple, Canary, Blue-Green) implements this trait
/// to provide strategy-specific reconciliation logic.
///
/// # Design Principles
/// - Each method should be focused and testable
/// - Methods should not have side effects beyond their stated purpose
/// - Implementations should be idempotent (safe to call multiple times)
///
/// # Example
/// ```ignore
/// let strategy = select_strategy(&rollout);
/// strategy.reconcile_replicasets(&rollout, &ctx).await?;
/// strategy.reconcile_traffic(&rollout, &ctx).await?;
/// let status = strategy.compute_next_status(&rollout);
/// ```
#[async_trait]
pub trait RolloutStrategy: Send + Sync {
    /// Strategy name for logging
    ///
    /// # Returns
    /// Static string identifying the strategy (e.g., "simple", "canary", "blue-green")
    fn name(&self) -> &'static str;

    /// Reconcile ReplicaSets for this strategy
    ///
    /// Creates or updates ReplicaSets according to strategy requirements:
    /// - Simple: 1 ReplicaSet with all replicas
    /// - Canary: 2 ReplicaSets (stable + canary) with traffic-based split
    /// - Blue-Green: 2 ReplicaSets (active + preview) both at full size
    ///
    /// # Arguments
    /// * `rollout` - The Rollout resource
    /// * `ctx` - Controller context with k8s client
    ///
    /// # Returns
    /// * `Ok(())` - ReplicaSets reconciled successfully
    /// * `Err(StrategyError)` - Reconciliation failed
    ///
    /// # Idempotency
    /// This method is idempotent - calling it multiple times with the same inputs
    /// produces the same result. Existing ReplicaSets are updated if needed.
    async fn reconcile_replicasets(
        &self,
        rollout: &Rollout,
        ctx: &Context,
    ) -> Result<(), StrategyError>;

    /// Update traffic routing (HTTPRoute) for this strategy
    ///
    /// Updates Gateway API HTTPRoute with weighted backend refs:
    /// - Simple: No-op (no traffic routing)
    /// - Canary: Gradual weight shift (stable + canary)
    /// - Blue-Green: Instant cutover (active + preview)
    ///
    /// # Arguments
    /// * `rollout` - The Rollout resource
    /// * `ctx` - Controller context with k8s client
    ///
    /// # Returns
    /// * `Ok(())` - Traffic routing updated or not applicable
    /// * `Err(StrategyError)` - Update failed
    ///
    /// # Non-fatal Errors
    /// If HTTPRoute is not found (404), this should NOT fail the reconciliation.
    /// Traffic routing is optional configuration.
    async fn reconcile_traffic(
        &self,
        rollout: &Rollout,
        ctx: &Context,
    ) -> Result<(), StrategyError>;

    /// Compute the next status for this rollout
    ///
    /// Determines what status should be written to the Rollout resource:
    /// - Simple: Always Completed
    /// - Canary: Progressing through steps, or Completed at 100%
    /// - Blue-Green: Preview → Completed on promotion
    ///
    /// # Arguments
    /// * `rollout` - The Rollout resource
    ///
    /// # Returns
    /// The desired RolloutStatus
    ///
    /// # Purity
    /// This function is pure - it has no side effects and always returns
    /// the same output for the same input.
    fn compute_next_status(&self, rollout: &Rollout) -> RolloutStatus;

    /// Does this strategy support metrics-based analysis?
    ///
    /// # Returns
    /// * `true` - Strategy can use Prometheus metrics for rollback
    /// * `false` - Strategy doesn't evaluate metrics
    fn supports_metrics_analysis(&self) -> bool;

    /// Does this strategy support manual promotion?
    ///
    /// # Returns
    /// * `true` - Strategy respects kulta.io/promote annotation
    /// * `false` - Strategy doesn't support manual promotion
    fn supports_manual_promotion(&self) -> bool {
        false // Default: no manual promotion
    }
}

/// Select the appropriate strategy handler based on Rollout spec
///
/// # Arguments
/// * `rollout` - The Rollout resource
///
/// # Returns
/// Box<dyn RolloutStrategy> for the appropriate strategy
///
/// # Strategy Selection Rules
/// 1. If spec.strategy.simple is Some → SimpleStrategyHandler
/// 2. If spec.strategy.blueGreen is Some → BlueGreenStrategyHandler
/// 3. Otherwise → CanaryStrategyHandler (default)
///
/// # Example
/// ```ignore
/// let strategy = select_strategy(&rollout);
/// info!(strategy = strategy.name(), "Selected strategy");
/// ```
pub fn select_strategy(rollout: &Rollout) -> Box<dyn RolloutStrategy> {
    use crate::controller::strategies::{
        blue_green::BlueGreenStrategyHandler, canary::CanaryStrategyHandler,
        simple::SimpleStrategyHandler,
    };

    if rollout.spec.strategy.simple.is_some() {
        Box::new(SimpleStrategyHandler)
    } else if rollout.spec.strategy.blue_green.is_some() {
        Box::new(BlueGreenStrategyHandler)
    } else {
        // Default to canary (most common)
        Box::new(CanaryStrategyHandler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::rollout::{
        BlueGreenStrategy, CanaryStrategy, RolloutSpec, RolloutStrategy as RolloutStrategySpec,
        SimpleStrategy,
    };
    use k8s_openapi::api::core::v1::PodTemplateSpec;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;

    fn create_test_rollout(strategy_spec: RolloutStrategySpec) -> Rollout {
        Rollout {
            metadata: kube::api::ObjectMeta {
                name: Some("test-rollout".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: RolloutSpec {
                replicas: 3,
                selector: LabelSelector::default(),
                template: PodTemplateSpec::default(),
                strategy: strategy_spec,
            },
            status: None,
        }
    }

    #[test]
    fn test_select_strategy_simple() {
        let rollout = create_test_rollout(RolloutStrategySpec {
            simple: Some(SimpleStrategy { analysis: None }),
            canary: None,
            blue_green: None,
        });

        let strategy = select_strategy(&rollout);
        assert_eq!(strategy.name(), "simple");
    }

    #[test]
    fn test_select_strategy_blue_green() {
        let rollout = create_test_rollout(RolloutStrategySpec {
            simple: None,
            canary: None,
            blue_green: Some(BlueGreenStrategy {
                active_service: "app-active".to_string(),
                preview_service: "app-preview".to_string(),
                auto_promotion_enabled: None,
                auto_promotion_seconds: None,
                traffic_routing: None,
                analysis: None,
            }),
        });

        let strategy = select_strategy(&rollout);
        assert_eq!(strategy.name(), "blue-green");
    }

    #[test]
    fn test_select_strategy_canary_default() {
        let rollout = create_test_rollout(RolloutStrategySpec {
            simple: None,
            canary: Some(CanaryStrategy {
                canary_service: "app-canary".to_string(),
                stable_service: "app-stable".to_string(),
                steps: vec![],
                traffic_routing: None,
                analysis: None,
            }),
            blue_green: None,
        });

        let strategy = select_strategy(&rollout);
        assert_eq!(strategy.name(), "canary");
    }

    #[test]
    fn test_select_strategy_empty_defaults_to_canary() {
        let rollout = create_test_rollout(RolloutStrategySpec {
            simple: None,
            canary: None,
            blue_green: None,
        });

        let strategy = select_strategy(&rollout);
        assert_eq!(strategy.name(), "canary");
    }
}
