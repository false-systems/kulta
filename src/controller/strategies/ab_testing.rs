//! A/B Testing strategy implementation
//!
//! Routes users based on headers or cookies to different variants.
//! Unlike canary (weight-based), A/B testing uses deterministic routing.
//! Both variants run at full capacity for fair comparison.

use super::{RolloutStrategy, StrategyError};
use crate::controller::rollout::{
    build_replicasets_for_ab_testing, default_service_port, ensure_replicaset_exists, Context,
};
use crate::crd::rollout::{ABMatchType, ABStrategy, Phase, Rollout, RolloutStatus};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gateway_api::apis::standard::httproutes::{
    HTTPRouteRules, HTTPRouteRulesBackendRefs, HTTPRouteRulesMatches, HTTPRouteRulesMatchesHeaders,
    HTTPRouteRulesMatchesHeadersType,
};
use k8s_openapi::api::apps::v1::ReplicaSet;
use kube::api::{Api, Patch, PatchParams};
use kube::core::DynamicObject;
use kube::discovery::ApiResource;
use kube::{Client, ResourceExt};
use tracing::{info, warn};

/// A/B Testing strategy handler
///
/// Implements header/cookie-based routing for A/B experiments.
/// Both variants run at full capacity (like blue-green).
/// Experiment concludes when statistical significance is reached.
pub struct ABTestingStrategyHandler;

#[async_trait]
impl RolloutStrategy for ABTestingStrategyHandler {
    fn name(&self) -> &'static str {
        "ab-testing"
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
            strategy = "ab-testing",
            replicas = rollout.spec.replicas,
            "Reconciling A/B testing strategy ReplicaSets"
        );

        // Build both ReplicaSets (variant-a + variant-b) at full size
        let (variant_a_rs, variant_b_rs) =
            build_replicasets_for_ab_testing(rollout, rollout.spec.replicas)
                .map_err(|e| StrategyError::ReplicaSetReconciliationFailed(e.to_string()))?;

        // Create ReplicaSet API client
        let rs_api: Api<ReplicaSet> = Api::namespaced(ctx.client.clone(), &namespace);

        // Ensure variant-a ReplicaSet exists
        ensure_replicaset_exists(&rs_api, &variant_a_rs, "variant-a", rollout.spec.replicas)
            .await
            .map_err(|e| StrategyError::ReplicaSetReconciliationFailed(e.to_string()))?;

        // Ensure variant-b ReplicaSet exists
        ensure_replicaset_exists(&rs_api, &variant_b_rs, "variant-b", rollout.spec.replicas)
            .await
            .map_err(|e| StrategyError::ReplicaSetReconciliationFailed(e.to_string()))?;

        info!(
            rollout = ?name,
            variant_a_replicas = rollout.spec.replicas,
            variant_b_replicas = rollout.spec.replicas,
            "A/B testing strategy ReplicaSets reconciled successfully"
        );

        Ok(())
    }

    async fn reconcile_traffic(
        &self,
        rollout: &Rollout,
        ctx: &Context,
    ) -> Result<(), StrategyError> {
        let ab_strategy =
            rollout.spec.strategy.ab_testing.as_ref().ok_or_else(|| {
                StrategyError::MissingField("spec.strategy.abTesting".to_string())
            })?;

        // Only configure traffic routing if Gateway API config is present
        let traffic_routing = match &ab_strategy.traffic_routing {
            Some(tr) => tr,
            None => {
                info!(
                    rollout = rollout.name_any(),
                    "No traffic routing configured for A/B testing"
                );
                return Ok(());
            }
        };

        let gateway_api_routing = match &traffic_routing.gateway_api {
            Some(ga) => ga,
            None => {
                info!(
                    rollout = rollout.name_any(),
                    "No Gateway API routing configured"
                );
                return Ok(());
            }
        };

        let namespace = rollout.namespace().unwrap_or_else(|| "default".to_string());

        // Build HTTPRoute rules for A/B testing
        let rules = build_ab_testing_httproute_rules(ab_strategy);

        // Patch the HTTPRoute with header-based rules
        patch_httproute_with_rules(
            &ctx.client,
            &namespace,
            &rollout.name_any(),
            &gateway_api_routing.http_route,
            &rules,
        )
        .await?;

        Ok(())
    }

    fn compute_next_status(&self, rollout: &Rollout, _now: DateTime<Utc>) -> RolloutStatus {
        let current_status = rollout.status.as_ref();
        let current_phase = current_status.and_then(|s| s.phase.clone());

        match current_phase {
            // Already completed - stay completed
            Some(Phase::Completed) => RolloutStatus {
                phase: Some(Phase::Completed),
                message: Some("A/B experiment completed".to_string()),
                ..current_status.cloned().unwrap_or_default()
            },

            // Already concluded - wait for promotion to complete
            Some(Phase::Concluded) => {
                // Check for promote annotation
                if has_promote_annotation(rollout) {
                    RolloutStatus {
                        phase: Some(Phase::Completed),
                        message: Some("A/B experiment promoted".to_string()),
                        ..current_status.cloned().unwrap_or_default()
                    }
                } else {
                    // Stay concluded, waiting for promotion
                    RolloutStatus {
                        phase: Some(Phase::Concluded),
                        message: Some("A/B experiment concluded, awaiting promotion".to_string()),
                        ..current_status.cloned().unwrap_or_default()
                    }
                }
            }

            // Experimenting - check if conclusion was reached by evaluate_ab_experiment()
            Some(Phase::Experimenting) => {
                // Statistical evaluation happens in reconcile() via evaluate_ab_experiment().
                // If it concluded, conclusion_reason will be set on the status.
                let has_conclusion = current_status
                    .and_then(|s| s.ab_experiment.as_ref())
                    .and_then(|ab| ab.conclusion_reason.as_ref())
                    .is_some();

                if has_conclusion {
                    RolloutStatus {
                        phase: Some(Phase::Concluded),
                        message: Some("A/B experiment concluded, awaiting promotion".to_string()),
                        ..current_status.cloned().unwrap_or_default()
                    }
                } else {
                    RolloutStatus {
                        phase: Some(Phase::Experimenting),
                        message: Some("A/B experiment in progress".to_string()),
                        ..current_status.cloned().unwrap_or_default()
                    }
                }
            }

            // Initial state or unknown - start experimenting
            _ => {
                let now = chrono::Utc::now().to_rfc3339();
                RolloutStatus {
                    phase: Some(Phase::Experimenting),
                    message: Some("A/B experiment started".to_string()),
                    ab_experiment: Some(crate::crd::rollout::ABExperimentStatus {
                        started_at: now,
                        concluded_at: None,
                        sample_size_a: None,
                        sample_size_b: None,
                        results: vec![],
                        winner: None,
                        conclusion_reason: None,
                    }),
                    ..Default::default()
                }
            }
        }
    }

    fn supports_metrics_analysis(&self) -> bool {
        // A/B testing uses metrics for statistical comparison
        true
    }

    fn supports_manual_promotion(&self) -> bool {
        // Can manually conclude/promote experiment
        true
    }
}

/// Build HTTPRoute rules for A/B testing
///
/// Creates multiple rules:
/// 1. Rule with header/cookie match -> variant B service
/// 2. Default rule (no match) -> variant A service (control)
///
/// The match rule comes first so it has higher priority.
pub fn build_ab_testing_httproute_rules(ab_strategy: &ABStrategy) -> Vec<HTTPRouteRules> {
    let port = default_service_port(ab_strategy.port);
    let mut rules = vec![];

    // Rule 1: Match condition -> Variant B (experiment)
    // This rule MUST come first (more specific matches first)
    if let Some(header_match) = &ab_strategy.variant_b_match.header {
        let match_type = match header_match.match_type {
            Some(ABMatchType::RegularExpression) => {
                Some(HTTPRouteRulesMatchesHeadersType::RegularExpression)
            }
            _ => Some(HTTPRouteRulesMatchesHeadersType::Exact),
        };

        rules.push(HTTPRouteRules {
            name: Some("variant-b".to_string()),
            matches: Some(vec![HTTPRouteRulesMatches {
                headers: Some(vec![HTTPRouteRulesMatchesHeaders {
                    name: header_match.name.clone(),
                    value: header_match.value.clone(),
                    r#type: match_type,
                }]),
                method: None,
                path: None,
                query_params: None,
            }]),
            backend_refs: Some(vec![HTTPRouteRulesBackendRefs {
                name: ab_strategy.variant_b_service.clone(),
                port: Some(port),
                weight: Some(100),
                kind: Some("Service".to_string()),
                group: Some(String::new()),
                namespace: None,
                filters: None,
            }]),
            filters: None,
            timeouts: None,
        });
    }

    // Cookie matching: Cookies are sent in the "Cookie" header
    // Match pattern: cookie_name=cookie_value
    if let Some(cookie_match) = &ab_strategy.variant_b_match.cookie {
        let cookie_pattern = format!("{}={}", cookie_match.name, cookie_match.value);

        rules.push(HTTPRouteRules {
            name: Some("variant-b-cookie".to_string()),
            matches: Some(vec![HTTPRouteRulesMatches {
                headers: Some(vec![HTTPRouteRulesMatchesHeaders {
                    name: "Cookie".to_string(),
                    value: cookie_pattern,
                    // Use RegularExpression to match cookie anywhere in header
                    r#type: Some(HTTPRouteRulesMatchesHeadersType::RegularExpression),
                }]),
                method: None,
                path: None,
                query_params: None,
            }]),
            backend_refs: Some(vec![HTTPRouteRulesBackendRefs {
                name: ab_strategy.variant_b_service.clone(),
                port: Some(port),
                weight: Some(100),
                kind: Some("Service".to_string()),
                group: Some(String::new()),
                namespace: None,
                filters: None,
            }]),
            filters: None,
            timeouts: None,
        });
    }

    // Rule 2: Default (no match) -> Variant A (control)
    // This catches all requests not matching variant B conditions
    rules.push(HTTPRouteRules {
        name: Some("variant-a".to_string()),
        matches: None, // No matches = default route
        backend_refs: Some(vec![HTTPRouteRulesBackendRefs {
            name: ab_strategy.variant_a_service.clone(),
            port: Some(port),
            weight: Some(100),
            kind: Some("Service".to_string()),
            group: Some(String::new()),
            namespace: None,
            filters: None,
        }]),
        filters: None,
        timeouts: None,
    });

    rules
}

/// Patch HTTPRoute with multiple rules (for A/B testing)
///
/// Unlike weight-based patching, this replaces all rules with header-match rules.
pub async fn patch_httproute_with_rules(
    client: &Client,
    namespace: &str,
    rollout_name: &str,
    httproute_name: &str,
    rules: &[HTTPRouteRules],
) -> Result<(), StrategyError> {
    // Use DynamicObject to avoid version issues with gateway-api types
    let api_resource = ApiResource::from_gvk(&kube::api::GroupVersionKind {
        group: "gateway.networking.k8s.io".to_string(),
        version: "v1".to_string(),
        kind: "HTTPRoute".to_string(),
    });

    let httproute_api: Api<DynamicObject> =
        Api::namespaced_with(client.clone(), namespace, &api_resource);

    // Build the patch with all rules
    let patch_json = serde_json::json!({
        "spec": {
            "rules": rules
        }
    });

    info!(
        rollout = rollout_name,
        httproute = httproute_name,
        rules_count = rules.len(),
        "Patching HTTPRoute with A/B testing rules"
    );

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
                rollout = rollout_name,
                httproute = httproute_name,
                "HTTPRoute patched successfully for A/B testing"
            );
            Ok(())
        }
        Err(kube::Error::Api(err)) if err.code == 404 => {
            warn!(
                rollout = rollout_name,
                httproute = httproute_name,
                "HTTPRoute not found (non-fatal)"
            );
            Ok(())
        }
        Err(e) => {
            warn!(
                rollout = rollout_name,
                httproute = httproute_name,
                error = ?e,
                "Failed to patch HTTPRoute"
            );
            Err(StrategyError::TrafficReconciliationFailed(e.to_string()))
        }
    }
}

/// Check if rollout has the promote annotation
fn has_promote_annotation(rollout: &Rollout) -> bool {
    rollout
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("kulta.io/promote"))
        .is_some()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::crd::rollout::{
        ABAnalysisConfig, ABCookieMatch, ABHeaderMatch, ABMatch, ABStrategy, ABVariant,
        RolloutSpec, RolloutStrategy as RolloutStrategySpec, TrafficRouting,
    };
    use k8s_openapi::api::core::v1::PodTemplateSpec;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
    use std::collections::BTreeMap;

    fn create_ab_testing_rollout(replicas: i32, phase: Option<Phase>) -> Rollout {
        Rollout {
            metadata: kube::api::ObjectMeta {
                name: Some("ab-test-rollout".to_string()),
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
                    blue_green: None,
                    ab_testing: Some(ABStrategy {
                        variant_a_service: "app-variant-a".to_string(),
                        variant_b_service: "app-variant-b".to_string(),
                        port: None,
                        variant_b_match: ABMatch {
                            header: Some(ABHeaderMatch {
                                name: "X-Variant".to_string(),
                                value: "B".to_string(),
                                match_type: None,
                            }),
                            cookie: None,
                        },
                        traffic_routing: Some(TrafficRouting { gateway_api: None }),
                        max_duration: Some("7d".to_string()),
                        analysis: Some(ABAnalysisConfig {
                            prometheus: None,
                            metrics: vec![],
                            min_duration: Some("1h".to_string()),
                            min_sample_size: Some(1000),
                            confidence_level: Some(0.95),
                        }),
                    }),
                },
                max_surge: None,
                max_unavailable: None,
                progress_deadline_seconds: None,
            },
            status: phase.map(|p| RolloutStatus {
                phase: Some(p),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn test_ab_testing_strategy_name() {
        let strategy = ABTestingStrategyHandler;
        assert_eq!(strategy.name(), "ab-testing");
    }

    #[test]
    fn test_ab_testing_strategy_supports_metrics_analysis() {
        let strategy = ABTestingStrategyHandler;
        assert!(strategy.supports_metrics_analysis());
    }

    #[test]
    fn test_ab_testing_strategy_supports_manual_promotion() {
        let strategy = ABTestingStrategyHandler;
        assert!(strategy.supports_manual_promotion());
    }

    #[test]
    fn test_ab_testing_compute_next_status_no_status_starts_experimenting() {
        let rollout = create_ab_testing_rollout(3, None);
        let strategy = ABTestingStrategyHandler;

        let status = strategy.compute_next_status(&rollout, Utc::now());

        assert_eq!(status.phase, Some(Phase::Experimenting));
        assert!(status.ab_experiment.is_some());
        let ab_exp = status.ab_experiment.unwrap();
        assert!(!ab_exp.started_at.is_empty());
        assert!(ab_exp.winner.is_none());
    }

    #[test]
    fn test_ab_testing_compute_next_status_experimenting_continues() {
        let rollout = create_ab_testing_rollout(3, Some(Phase::Experimenting));
        let strategy = ABTestingStrategyHandler;

        let status = strategy.compute_next_status(&rollout, Utc::now());

        assert_eq!(status.phase, Some(Phase::Experimenting));
    }

    #[test]
    fn test_ab_testing_compute_next_status_concluded_waits_for_promotion() {
        let rollout = create_ab_testing_rollout(3, Some(Phase::Concluded));
        let strategy = ABTestingStrategyHandler;

        let status = strategy.compute_next_status(&rollout, Utc::now());

        assert_eq!(status.phase, Some(Phase::Concluded));
        assert!(status.message.unwrap().contains("awaiting promotion"));
    }

    #[test]
    fn test_ab_testing_compute_next_status_concluded_with_promote_annotation() {
        let mut rollout = create_ab_testing_rollout(3, Some(Phase::Concluded));
        let mut annotations = BTreeMap::new();
        annotations.insert("kulta.io/promote".to_string(), "true".to_string());
        rollout.metadata.annotations = Some(annotations);

        let strategy = ABTestingStrategyHandler;
        let status = strategy.compute_next_status(&rollout, Utc::now());

        assert_eq!(status.phase, Some(Phase::Completed));
    }

    #[test]
    fn test_ab_testing_compute_next_status_completed_stays_completed() {
        let rollout = create_ab_testing_rollout(3, Some(Phase::Completed));
        let strategy = ABTestingStrategyHandler;

        let status = strategy.compute_next_status(&rollout, Utc::now());

        assert_eq!(status.phase, Some(Phase::Completed));
    }

    // === HTTPRoute rules building tests ===

    #[test]
    fn test_build_ab_testing_rules_with_header_match() {
        let ab_strategy = ABStrategy {
            variant_a_service: "app-control".to_string(),
            variant_b_service: "app-experiment".to_string(),
            port: None,
            variant_b_match: ABMatch {
                header: Some(ABHeaderMatch {
                    name: "X-Variant".to_string(),
                    value: "B".to_string(),
                    match_type: None,
                }),
                cookie: None,
            },
            traffic_routing: None,
            max_duration: None,
            analysis: None,
        };

        let rules = build_ab_testing_httproute_rules(&ab_strategy);

        // Should have 2 rules: variant-b (with match) and variant-a (default)
        assert_eq!(rules.len(), 2);

        // First rule: variant-b with header match
        let variant_b_rule = &rules[0];
        assert_eq!(variant_b_rule.name, Some("variant-b".to_string()));
        assert!(variant_b_rule.matches.is_some());
        let matches = variant_b_rule.matches.as_ref().unwrap();
        assert_eq!(matches.len(), 1);
        let headers = matches[0].headers.as_ref().unwrap();
        assert_eq!(headers[0].name, "X-Variant");
        assert_eq!(headers[0].value, "B");
        assert_eq!(
            headers[0].r#type,
            Some(HTTPRouteRulesMatchesHeadersType::Exact)
        );

        // Backend should be variant-b service
        let backend_refs = variant_b_rule.backend_refs.as_ref().unwrap();
        assert_eq!(backend_refs[0].name, "app-experiment");

        // Second rule: variant-a (default, no matches)
        let variant_a_rule = &rules[1];
        assert_eq!(variant_a_rule.name, Some("variant-a".to_string()));
        assert!(variant_a_rule.matches.is_none());
        let backend_refs = variant_a_rule.backend_refs.as_ref().unwrap();
        assert_eq!(backend_refs[0].name, "app-control");
    }

    #[test]
    fn test_build_ab_testing_rules_with_cookie_match() {
        let ab_strategy = ABStrategy {
            variant_a_service: "app-control".to_string(),
            variant_b_service: "app-experiment".to_string(),
            port: None,
            variant_b_match: ABMatch {
                header: None,
                cookie: Some(ABCookieMatch {
                    name: "ab_variant".to_string(),
                    value: "experiment".to_string(),
                }),
            },
            traffic_routing: None,
            max_duration: None,
            analysis: None,
        };

        let rules = build_ab_testing_httproute_rules(&ab_strategy);

        // Should have 2 rules: variant-b-cookie and variant-a
        assert_eq!(rules.len(), 2);

        // First rule: cookie match (regex on Cookie header)
        let cookie_rule = &rules[0];
        assert_eq!(cookie_rule.name, Some("variant-b-cookie".to_string()));
        let matches = cookie_rule.matches.as_ref().unwrap();
        let headers = matches[0].headers.as_ref().unwrap();
        assert_eq!(headers[0].name, "Cookie");
        assert_eq!(headers[0].value, "ab_variant=experiment");
        assert_eq!(
            headers[0].r#type,
            Some(HTTPRouteRulesMatchesHeadersType::RegularExpression)
        );
    }

    #[test]
    fn test_build_ab_testing_rules_with_both_header_and_cookie() {
        let ab_strategy = ABStrategy {
            variant_a_service: "app-control".to_string(),
            variant_b_service: "app-experiment".to_string(),
            port: None,
            variant_b_match: ABMatch {
                header: Some(ABHeaderMatch {
                    name: "X-Variant".to_string(),
                    value: "B".to_string(),
                    match_type: None,
                }),
                cookie: Some(ABCookieMatch {
                    name: "user_variant".to_string(),
                    value: "B".to_string(),
                }),
            },
            traffic_routing: None,
            max_duration: None,
            analysis: None,
        };

        let rules = build_ab_testing_httproute_rules(&ab_strategy);

        // Should have 3 rules: header match, cookie match, and default
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].name, Some("variant-b".to_string()));
        assert_eq!(rules[1].name, Some("variant-b-cookie".to_string()));
        assert_eq!(rules[2].name, Some("variant-a".to_string()));
    }

    // === A/B ReplicaSet builder tests ===

    #[test]
    fn test_ab_replicasets_naming() {
        use crate::controller::rollout::build_replicasets_for_ab_testing;

        let rollout = create_ab_testing_rollout(3, None);
        let (variant_a, variant_b) = build_replicasets_for_ab_testing(&rollout, 3).unwrap();

        assert_eq!(
            variant_a.metadata.name,
            Some("ab-test-rollout-variant-a".to_string())
        );
        assert_eq!(
            variant_b.metadata.name,
            Some("ab-test-rollout-variant-b".to_string())
        );
    }

    #[test]
    fn test_ab_replicasets_labels() {
        use crate::controller::rollout::build_replicasets_for_ab_testing;

        let rollout = create_ab_testing_rollout(3, None);
        let (variant_a, variant_b) = build_replicasets_for_ab_testing(&rollout, 3).unwrap();

        let labels_a = variant_a.metadata.labels.as_ref().unwrap();
        assert_eq!(
            labels_a.get("rollouts.kulta.io/type"),
            Some(&"variant-a".to_string())
        );
        assert_eq!(
            labels_a.get("rollouts.kulta.io/managed"),
            Some(&"true".to_string())
        );

        let labels_b = variant_b.metadata.labels.as_ref().unwrap();
        assert_eq!(
            labels_b.get("rollouts.kulta.io/type"),
            Some(&"variant-b".to_string())
        );
    }

    #[test]
    fn test_ab_replicasets_replica_counts() {
        use crate::controller::rollout::build_replicasets_for_ab_testing;

        let rollout = create_ab_testing_rollout(5, None);
        let (variant_a, variant_b) = build_replicasets_for_ab_testing(&rollout, 5).unwrap();

        assert_eq!(variant_a.spec.as_ref().unwrap().replicas, Some(5));
        assert_eq!(variant_b.spec.as_ref().unwrap().replicas, Some(5));
    }

    #[test]
    fn test_ab_replicasets_have_template_hash() {
        use crate::controller::rollout::build_replicasets_for_ab_testing;

        let rollout = create_ab_testing_rollout(3, None);
        let (variant_a, variant_b) = build_replicasets_for_ab_testing(&rollout, 3).unwrap();

        let labels_a = variant_a.metadata.labels.as_ref().unwrap();
        let labels_b = variant_b.metadata.labels.as_ref().unwrap();

        // Both should have the same pod-template-hash (same template)
        let hash_a = labels_a.get("pod-template-hash").unwrap();
        let hash_b = labels_b.get("pod-template-hash").unwrap();
        assert_eq!(hash_a, hash_b);
        assert_eq!(hash_a.len(), 10);
    }

    // === compute_next_status Experimenting → Concluded tests ===

    #[test]
    fn test_ab_experimenting_with_conclusion_reason_transitions_to_concluded() {
        use crate::crd::rollout::{ABConclusionReason, ABExperimentStatus};

        let mut rollout = create_ab_testing_rollout(3, Some(Phase::Experimenting));
        // Set conclusion_reason (as would be set by evaluate_ab_experiment)
        rollout.status = Some(RolloutStatus {
            phase: Some(Phase::Experimenting),
            ab_experiment: Some(ABExperimentStatus {
                started_at: "2026-01-01T00:00:00Z".to_string(),
                concluded_at: None,
                sample_size_a: Some(5000),
                sample_size_b: Some(5000),
                results: vec![],
                winner: Some(ABVariant::B),
                conclusion_reason: Some(ABConclusionReason::ConsensusReached),
            }),
            ..Default::default()
        });

        let strategy = ABTestingStrategyHandler;
        let status = strategy.compute_next_status(&rollout, Utc::now());

        assert_eq!(status.phase, Some(Phase::Concluded));
    }

    #[test]
    fn test_ab_experimenting_without_conclusion_stays_experimenting() {
        use crate::crd::rollout::ABExperimentStatus;

        let mut rollout = create_ab_testing_rollout(3, Some(Phase::Experimenting));
        rollout.status = Some(RolloutStatus {
            phase: Some(Phase::Experimenting),
            ab_experiment: Some(ABExperimentStatus {
                started_at: "2026-01-01T00:00:00Z".to_string(),
                concluded_at: None,
                sample_size_a: Some(100),
                sample_size_b: Some(100),
                results: vec![],
                winner: None,
                conclusion_reason: None, // No conclusion yet
            }),
            ..Default::default()
        });

        let strategy = ABTestingStrategyHandler;
        let status = strategy.compute_next_status(&rollout, Utc::now());

        assert_eq!(status.phase, Some(Phase::Experimenting));
    }

    #[test]
    fn test_build_ab_testing_rules_with_regex_match_type() {
        let ab_strategy = ABStrategy {
            variant_a_service: "app-control".to_string(),
            variant_b_service: "app-experiment".to_string(),
            port: None,
            variant_b_match: ABMatch {
                header: Some(ABHeaderMatch {
                    name: "X-User-Segment".to_string(),
                    value: "beta.*".to_string(),
                    match_type: Some(ABMatchType::RegularExpression),
                }),
                cookie: None,
            },
            traffic_routing: None,
            max_duration: None,
            analysis: None,
        };

        let rules = build_ab_testing_httproute_rules(&ab_strategy);

        let variant_b_rule = &rules[0];
        let matches = variant_b_rule.matches.as_ref().unwrap();
        let headers = matches[0].headers.as_ref().unwrap();
        assert_eq!(
            headers[0].r#type,
            Some(HTTPRouteRulesMatchesHeadersType::RegularExpression)
        );
    }
}
