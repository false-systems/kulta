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
use gateway_api::apis::standard::httproutes::HTTPRouteRulesBackendRefs;
use thiserror::Error;
use tracing::{error, info, warn};

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

/// Patch an HTTPRoute with weighted backend refs
///
/// This shared helper handles the common logic for updating Gateway API HTTPRoute
/// resources with weighted backends. Used by both canary and blue-green strategies.
///
/// # Arguments
/// * `ctx` - Controller context with k8s client
/// * `namespace` - Namespace of the HTTPRoute
/// * `httproute_name` - Name of the HTTPRoute to patch
/// * `backend_refs` - Weighted backend refs to set
/// * `rollout_name` - Rollout name for logging
/// * `strategy_name` - Strategy name for logging ("canary" or "blue-green")
///
/// # Returns
/// * `Ok(())` - HTTPRoute patched successfully or not found (non-fatal)
/// * `Err(StrategyError)` - Patch failed
pub async fn patch_httproute_weights(
    ctx: &Context,
    namespace: &str,
    httproute_name: &str,
    backend_refs: Vec<HTTPRouteRulesBackendRefs>,
    rollout_name: &str,
    strategy_name: &str,
) -> Result<(), StrategyError> {
    use kube::api::{Api, Patch, PatchParams};
    use kube::core::DynamicObject;
    use kube::discovery::ApiResource;

    info!(
        rollout = ?rollout_name,
        httproute = ?httproute_name,
        strategy = strategy_name,
        "Updating HTTPRoute with weighted backends"
    );

    // Create JSON patch to update HTTPRoute's first rule's backendRefs
    let patch_json = serde_json::json!({
        "spec": {
            "rules": [{
                "backendRefs": backend_refs
            }]
        }
    });

    // Create HTTPRoute API client using DynamicObject
    let ar = ApiResource {
        group: "gateway.networking.k8s.io".to_string(),
        version: "v1".to_string(),
        api_version: "gateway.networking.k8s.io/v1".to_string(),
        kind: "HTTPRoute".to_string(),
        plural: "httproutes".to_string(),
    };

    let httproute_api: Api<DynamicObject> =
        Api::namespaced_with(ctx.client.clone(), namespace, &ar);

    // Apply the patch
    match httproute_api
        .patch(
            httproute_name,
            &PatchParams::default(),
            &Patch::Merge(&patch_json),
        )
        .await
    {
        Ok(_) => {
            info!(
                rollout = ?rollout_name,
                httproute = ?httproute_name,
                weight_0 = backend_refs.first().and_then(|b| b.weight),
                weight_1 = backend_refs.get(1).and_then(|b| b.weight),
                strategy = strategy_name,
                "HTTPRoute updated successfully"
            );
            Ok(())
        }
        Err(kube::Error::Api(err)) if err.code == 404 => {
            // HTTPRoute not found - this is non-fatal, traffic routing is optional
            warn!(
                rollout = ?rollout_name,
                httproute = ?httproute_name,
                "HTTPRoute not found - skipping traffic routing update"
            );
            Ok(())
        }
        Err(e) => {
            error!(
                error = ?e,
                rollout = ?rollout_name,
                httproute = ?httproute_name,
                "Failed to patch HTTPRoute"
            );
            Err(StrategyError::TrafficReconciliationFailed(e.to_string()))
        }
    }
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
    fn supports_manual_promotion(&self) -> bool;
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
            simple: Some(SimpleStrategy {}),
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
