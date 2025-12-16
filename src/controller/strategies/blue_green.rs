//! Blue-Green deployment strategy
//!
//! Maintains two full environments (active and preview).
//! Traffic is 100% to active until promotion, then instant switch to preview.

use super::{RolloutStrategy, StrategyError};
use crate::controller::rollout::{
    build_gateway_api_backend_refs, build_replicasets_for_blue_green, ensure_replicaset_exists,
    has_promote_annotation, initialize_rollout_status, Context,
};
use crate::crd::rollout::{Phase, Rollout, RolloutStatus};
use async_trait::async_trait;
use k8s_openapi::api::apps::v1::ReplicaSet;
use kube::api::Api;
use kube::ResourceExt;
use tracing::info;

/// Blue-Green strategy handler
///
/// Implements blue-green deployment:
/// - Two full-size ReplicaSets (active + preview)
/// - Instant traffic cutover (no gradual shift)
/// - Preview environment for testing before promotion
/// - Optional auto-promotion after duration
pub struct BlueGreenStrategyHandler;

impl BlueGreenStrategyHandler {
    /// Check if auto-promotion should trigger based on elapsed time
    ///
    /// Returns true if:
    /// - auto_promotion_enabled is true
    /// - auto_promotion_seconds is configured
    /// - pause_start_time is set (when preview started)
    /// - elapsed time >= auto_promotion_seconds
    fn should_auto_promote(&self, rollout: &Rollout, status: &RolloutStatus) -> bool {
        // Get blue-green strategy config
        let blue_green = match &rollout.spec.strategy.blue_green {
            Some(bg) => bg,
            None => return false,
        };

        // Check if auto-promotion is enabled
        let auto_enabled = blue_green.auto_promotion_enabled.unwrap_or(false);
        if !auto_enabled {
            return false;
        }

        // Get auto-promotion duration
        let auto_seconds = match blue_green.auto_promotion_seconds {
            Some(secs) => secs,
            None => return false,
        };

        // Get preview start time from status
        let preview_start_str = match &status.pause_start_time {
            Some(ts) => ts,
            None => return false,
        };

        // Parse preview start time
        let preview_start = match chrono::DateTime::parse_from_rfc3339(preview_start_str) {
            Ok(ts) => ts,
            Err(_) => return false,
        };

        // Calculate elapsed time
        let now = chrono::Utc::now();
        let elapsed = now.signed_duration_since(preview_start);

        // Check if auto-promotion time has passed
        elapsed.num_seconds() >= i64::from(auto_seconds)
    }
}

#[async_trait]
impl RolloutStrategy for BlueGreenStrategyHandler {
    fn name(&self) -> &'static str {
        "blue-green"
    }

    async fn reconcile_replicasets(
        &self,
        rollout: &Rollout,
        ctx: &Context,
    ) -> Result<(), StrategyError> {
        let namespace = rollout
            .namespace()
            .ok_or_else(|| StrategyError::MissingField("namespace".to_string()))?;
        let name = rollout.name_any();

        info!(
            rollout = ?name,
            strategy = "blue-green",
            replicas = rollout.spec.replicas,
            "Reconciling blue-green strategy ReplicaSets"
        );

        // Build both ReplicaSets (active + preview) at full size
        let (active_rs, preview_rs) =
            build_replicasets_for_blue_green(rollout, rollout.spec.replicas)
                .map_err(|e| StrategyError::ReplicaSetReconciliationFailed(e.to_string()))?;

        // Create ReplicaSet API client
        let rs_api: Api<ReplicaSet> = Api::namespaced(ctx.client.clone(), &namespace);

        // Ensure active ReplicaSet exists
        ensure_replicaset_exists(&rs_api, &active_rs, "active", rollout.spec.replicas)
            .await
            .map_err(|e| StrategyError::ReplicaSetReconciliationFailed(e.to_string()))?;

        // Ensure preview ReplicaSet exists
        ensure_replicaset_exists(&rs_api, &preview_rs, "preview", rollout.spec.replicas)
            .await
            .map_err(|e| StrategyError::ReplicaSetReconciliationFailed(e.to_string()))?;

        info!(
            rollout = ?name,
            active_replicas = rollout.spec.replicas,
            preview_replicas = rollout.spec.replicas,
            "Blue-green strategy ReplicaSets reconciled successfully"
        );

        Ok(())
    }

    async fn reconcile_traffic(
        &self,
        rollout: &Rollout,
        ctx: &Context,
    ) -> Result<(), StrategyError> {
        // Check if blue-green strategy has traffic routing configured
        let blue_green = match &rollout.spec.strategy.blue_green {
            Some(strategy) => strategy,
            None => return Ok(()),
        };

        let traffic_routing = match &blue_green.traffic_routing {
            Some(routing) => routing,
            None => return Ok(()),
        };

        let gateway_api_routing = match &traffic_routing.gateway_api {
            Some(routing) => routing,
            None => return Ok(()),
        };

        let namespace = rollout
            .namespace()
            .ok_or_else(|| StrategyError::MissingField("namespace".to_string()))?;

        let backend_refs = build_gateway_api_backend_refs(rollout);

        super::patch_httproute_weights(
            ctx,
            &namespace,
            &gateway_api_routing.http_route,
            backend_refs,
            &rollout.name_any(),
            "blue-green",
        )
        .await
    }

    fn compute_next_status(&self, rollout: &Rollout) -> RolloutStatus {
        // If no status exists, initialize to Preview phase
        let current_status = match &rollout.status {
            Some(status) => status,
            None => return initialize_rollout_status(rollout),
        };

        // Get current phase
        let current_phase = current_status
            .phase
            .as_ref()
            .cloned()
            .unwrap_or(Phase::Preview);

        match current_phase {
            Phase::Preview => {
                // Check for manual promotion annotation
                if has_promote_annotation(rollout) {
                    info!(
                        rollout = ?rollout.name_any(),
                        "Blue-green promotion triggered via annotation"
                    );
                    return RolloutStatus {
                        phase: Some(Phase::Completed),
                        message: Some(
                            "Blue-green rollout completed: promoted to production".to_string(),
                        ),
                        ..current_status.clone()
                    };
                }

                // Check for auto-promotion timer
                if self.should_auto_promote(rollout, current_status) {
                    info!(
                        rollout = ?rollout.name_any(),
                        "Blue-green auto-promotion triggered: time elapsed"
                    );
                    return RolloutStatus {
                        phase: Some(Phase::Completed),
                        message: Some(
                            "Blue-green rollout completed: auto-promoted after duration"
                                .to_string(),
                        ),
                        ..current_status.clone()
                    };
                }

                // Stay in Preview phase
                current_status.clone()
            }
            Phase::Completed => {
                // Already completed, return as-is
                current_status.clone()
            }
            Phase::Failed => {
                // Failed, return as-is
                current_status.clone()
            }
            _ => {
                // Unexpected phase for blue-green, reinitialize
                initialize_rollout_status(rollout)
            }
        }
    }

    fn supports_metrics_analysis(&self) -> bool {
        // Blue-green can support metrics analysis if configured
        true
    }

    fn supports_manual_promotion(&self) -> bool {
        // Blue-green supports manual promotion
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::rollout::{
        BlueGreenStrategy, GatewayAPIRouting, Phase, RolloutSpec,
        RolloutStrategy as RolloutStrategySpec, TrafficRouting,
    };
    use k8s_openapi::api::core::v1::PodTemplateSpec;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;

    fn create_blue_green_rollout(replicas: i32) -> Rollout {
        Rollout {
            metadata: kube::api::ObjectMeta {
                name: Some("test-bg-rollout".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: RolloutSpec {
                replicas,
                selector: LabelSelector::default(),
                template: PodTemplateSpec::default(),
                strategy: RolloutStrategySpec {
                    simple: None,
                    canary: None,
                    blue_green: Some(BlueGreenStrategy {
                        active_service: "app-active".to_string(),
                        preview_service: "app-preview".to_string(),
                        auto_promotion_enabled: None,
                        auto_promotion_seconds: None,
                        traffic_routing: Some(TrafficRouting {
                            gateway_api: Some(GatewayAPIRouting {
                                http_route: "app-route".to_string(),
                            }),
                        }),
                        analysis: None,
                    }),
                },
            },
            status: None,
        }
    }

    #[test]
    fn test_blue_green_strategy_name() {
        let strategy = BlueGreenStrategyHandler;
        assert_eq!(strategy.name(), "blue-green");
    }

    #[test]
    fn test_blue_green_strategy_supports_metrics_analysis() {
        let strategy = BlueGreenStrategyHandler;
        assert!(strategy.supports_metrics_analysis());
    }

    #[test]
    fn test_blue_green_strategy_supports_manual_promotion() {
        let strategy = BlueGreenStrategyHandler;
        assert!(strategy.supports_manual_promotion());
    }

    #[test]
    fn test_blue_green_strategy_compute_next_status() {
        let rollout = create_blue_green_rollout(5);
        let strategy = BlueGreenStrategyHandler;

        let status = strategy.compute_next_status(&rollout);

        // Blue-green should start in Preview phase
        assert_eq!(status.phase, Some(Phase::Preview));
        assert_eq!(status.current_step_index, None); // No steps in blue-green
        assert_eq!(status.current_weight, None); // No weight in blue-green
        match status.message {
            Some(msg) => assert!(msg.contains("preview environment ready")),
            None => panic!("status should have a message"),
        }
    }

    // Note: reconcile_replicasets() and reconcile_traffic() require K8s API
    // These are tested in integration tests

    // TDD RED: Test that promotion annotation transitions to Completed
    #[test]
    fn test_blue_green_promotion_annotation_transitions_to_completed() {
        use std::collections::BTreeMap;

        let mut rollout = create_blue_green_rollout(5);

        // Set existing status to Preview (simulating previous reconcile)
        rollout.status = Some(crate::crd::rollout::RolloutStatus {
            phase: Some(Phase::Preview),
            message: Some("Blue-green rollout: preview environment ready".to_string()),
            ..Default::default()
        });

        // Add promote annotation
        let mut annotations = BTreeMap::new();
        annotations.insert("kulta.io/promote".to_string(), "true".to_string());
        rollout.metadata.annotations = Some(annotations);

        let strategy = BlueGreenStrategyHandler;
        let status = strategy.compute_next_status(&rollout);

        // After promotion, should transition to Completed
        assert_eq!(
            status.phase,
            Some(Phase::Completed),
            "Blue-green with promote annotation should transition to Completed"
        );
    }

    // TDD RED: Test that rollout stays in Preview without promotion
    #[test]
    fn test_blue_green_stays_in_preview_without_promotion() {
        let mut rollout = create_blue_green_rollout(5);

        // Set existing status to Preview (simulating previous reconcile)
        rollout.status = Some(crate::crd::rollout::RolloutStatus {
            phase: Some(Phase::Preview),
            message: Some("Blue-green rollout: preview environment ready".to_string()),
            ..Default::default()
        });

        // No promote annotation

        let strategy = BlueGreenStrategyHandler;
        let status = strategy.compute_next_status(&rollout);

        // Without promotion, should stay in Preview
        assert_eq!(
            status.phase,
            Some(Phase::Preview),
            "Blue-green without promote annotation should stay in Preview"
        );
    }

    fn create_blue_green_rollout_with_auto_promotion(
        replicas: i32,
        auto_promotion_enabled: bool,
        auto_promotion_seconds: i32,
    ) -> Rollout {
        Rollout {
            metadata: kube::api::ObjectMeta {
                name: Some("test-bg-rollout".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: RolloutSpec {
                replicas,
                selector: LabelSelector::default(),
                template: PodTemplateSpec::default(),
                strategy: RolloutStrategySpec {
                    simple: None,
                    canary: None,
                    blue_green: Some(BlueGreenStrategy {
                        active_service: "app-active".to_string(),
                        preview_service: "app-preview".to_string(),
                        auto_promotion_enabled: Some(auto_promotion_enabled),
                        auto_promotion_seconds: Some(auto_promotion_seconds),
                        traffic_routing: None,
                        analysis: None,
                    }),
                },
            },
            status: None,
        }
    }

    #[test]
    fn test_blue_green_auto_promotion_triggers_when_time_elapsed() {
        use chrono::{Duration, Utc};

        let mut rollout = create_blue_green_rollout_with_auto_promotion(5, true, 60);

        // Set preview start time to 2 minutes ago (well past the 60s threshold)
        let preview_start = Utc::now() - Duration::seconds(120);
        rollout.status = Some(crate::crd::rollout::RolloutStatus {
            phase: Some(Phase::Preview),
            pause_start_time: Some(preview_start.to_rfc3339()),
            message: Some("Blue-green rollout: preview environment ready".to_string()),
            ..Default::default()
        });

        let strategy = BlueGreenStrategyHandler;
        let status = strategy.compute_next_status(&rollout);

        assert_eq!(
            status.phase,
            Some(Phase::Completed),
            "Blue-green should auto-promote to Completed when time elapsed"
        );
    }

    #[test]
    fn test_blue_green_auto_promotion_does_not_trigger_before_time() {
        use chrono::{Duration, Utc};

        let mut rollout = create_blue_green_rollout_with_auto_promotion(5, true, 300);

        // Set preview start time to 30 seconds ago (before 300s threshold)
        let preview_start = Utc::now() - Duration::seconds(30);
        rollout.status = Some(crate::crd::rollout::RolloutStatus {
            phase: Some(Phase::Preview),
            pause_start_time: Some(preview_start.to_rfc3339()),
            message: Some("Blue-green rollout: preview environment ready".to_string()),
            ..Default::default()
        });

        let strategy = BlueGreenStrategyHandler;
        let status = strategy.compute_next_status(&rollout);

        assert_eq!(
            status.phase,
            Some(Phase::Preview),
            "Blue-green should stay in Preview before auto-promotion time"
        );
    }

    #[test]
    fn test_blue_green_auto_promotion_disabled() {
        use chrono::{Duration, Utc};

        let mut rollout = create_blue_green_rollout_with_auto_promotion(5, false, 60);

        // Set preview start time to 2 minutes ago (past the threshold)
        let preview_start = Utc::now() - Duration::seconds(120);
        rollout.status = Some(crate::crd::rollout::RolloutStatus {
            phase: Some(Phase::Preview),
            pause_start_time: Some(preview_start.to_rfc3339()),
            message: Some("Blue-green rollout: preview environment ready".to_string()),
            ..Default::default()
        });

        let strategy = BlueGreenStrategyHandler;
        let status = strategy.compute_next_status(&rollout);

        assert_eq!(
            status.phase,
            Some(Phase::Preview),
            "Blue-green should stay in Preview when auto-promotion is disabled"
        );
    }

    #[test]
    fn test_blue_green_initialization_sets_pause_start_time() {
        let rollout = create_blue_green_rollout_with_auto_promotion(5, true, 60);
        let strategy = BlueGreenStrategyHandler;

        let status = strategy.compute_next_status(&rollout);

        assert_eq!(status.phase, Some(Phase::Preview));
        assert!(
            status.pause_start_time.is_some(),
            "Blue-green initialization should set pause_start_time for auto-promotion tracking"
        );
    }
}
