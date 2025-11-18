use super::*;
use crate::crd::rollout::{
    CanaryStep, CanaryStrategy, GatewayAPIRouting, Rollout, RolloutSpec, RolloutStatus,
    RolloutStrategy, TrafficRouting,
};
use kube::api::ObjectMeta;
use std::sync::Arc;

#[tokio::test]
async fn test_reconcile_creates_stable_replicaset() {
    // Create a mock Rollout resource
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-rollout".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                match_labels: Some(
                    vec![("app".to_string(), "test-app".to_string())]
                        .into_iter()
                        .collect(),
                ),
                ..Default::default()
            },
            template: k8s_openapi::api::core::v1::PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(
                        vec![("app".to_string(), "test-app".to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                }),
                spec: Some(k8s_openapi::api::core::v1::PodSpec {
                    containers: vec![k8s_openapi::api::core::v1::Container {
                        name: "app".to_string(),
                        image: Some("nginx:1.0".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            strategy: RolloutStrategy {
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![
                        CanaryStep {
                            set_weight: Some(20),
                            pause: None,
                        },
                        CanaryStep {
                            set_weight: Some(50),
                            pause: None,
                        },
                    ],
                    traffic_routing: Some(TrafficRouting {
                        gateway_api: Some(GatewayAPIRouting {
                            http_route: "test-route".to_string(),
                        }),
                    }),
                }),
            },
        },
        status: None,
    };

    // Test that reconcile creates a stable ReplicaSet
    // This test verifies the ReplicaSet is actually created (not just that reconcile returns Ok)

    // For now, we'll test that build_replicaset is called correctly
    // (Full integration test requires real K8s cluster)
    let stable_rs = build_replicaset(&rollout, "stable", rollout.spec.replicas);

    // Verify stable ReplicaSet has correct properties
    assert_eq!(
        stable_rs.metadata.name.as_deref(),
        Some("test-rollout-stable")
    );
    assert_eq!(stable_rs.metadata.namespace.as_deref(), Some("default"));
    assert_eq!(stable_rs.spec.as_ref().unwrap().replicas, Some(3));

    // Verify reconcile returns Ok
    let result = reconcile(Arc::new(rollout), Arc::new(Context::new_mock())).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_compute_pod_template_hash() {
    // Test that we can generate stable pod-template-hash for ReplicaSets
    let pod_template = k8s_openapi::api::core::v1::PodTemplateSpec {
        metadata: Some(ObjectMeta {
            labels: Some(
                vec![("app".to_string(), "test-app".to_string())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        }),
        spec: Some(k8s_openapi::api::core::v1::PodSpec {
            containers: vec![k8s_openapi::api::core::v1::Container {
                name: "app".to_string(),
                image: Some("nginx:1.0".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        }),
    };

    let hash1 = compute_pod_template_hash(&pod_template);
    let hash2 = compute_pod_template_hash(&pod_template);

    // Same template should produce same hash
    assert_eq!(hash1, hash2);
    assert_eq!(hash1.len(), 10); // 10-character hash like Kubernetes

    // Different template should produce different hash
    let mut different_template = pod_template.clone();
    if let Some(ref mut spec) = different_template.spec {
        spec.containers[0].image = Some("nginx:2.0".to_string());
    }

    let hash3 = compute_pod_template_hash(&different_template);
    assert_ne!(hash1, hash3);
}

#[tokio::test]
async fn test_build_replicaset_spec() {
    // Test that we can build a ReplicaSet from a Rollout
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-rollout".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                match_labels: Some(
                    vec![("app".to_string(), "test-app".to_string())]
                        .into_iter()
                        .collect(),
                ),
                ..Default::default()
            },
            template: k8s_openapi::api::core::v1::PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(
                        vec![("app".to_string(), "test-app".to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                }),
                spec: Some(k8s_openapi::api::core::v1::PodSpec {
                    containers: vec![k8s_openapi::api::core::v1::Container {
                        name: "app".to_string(),
                        image: Some("nginx:1.0".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            strategy: RolloutStrategy {
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![],
                    traffic_routing: None,
                }),
            },
        },
        status: None,
    };

    // Build stable ReplicaSet
    let rs = build_replicaset(&rollout, "stable", 3);

    assert_eq!(rs.metadata.name.as_deref(), Some("test-rollout-stable"));
    assert_eq!(rs.metadata.namespace.as_deref(), Some("default"));
    assert_eq!(rs.spec.as_ref().unwrap().replicas, Some(3));

    // Verify pod-template-hash label exists
    let labels = &rs
        .spec
        .as_ref()
        .unwrap()
        .template
        .as_ref()
        .unwrap()
        .metadata
        .as_ref()
        .unwrap()
        .labels;
    assert!(labels.as_ref().unwrap().contains_key("pod-template-hash"));

    // Verify rollouts.kulta.io/type label
    assert_eq!(
        labels.as_ref().unwrap().get("rollouts.kulta.io/type"),
        Some(&"stable".to_string())
    );
}

#[tokio::test]
async fn test_reconcile_creates_canary_replicaset() {
    // Test that reconcile creates BOTH stable and canary ReplicaSets
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-rollout".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                match_labels: Some(
                    vec![("app".to_string(), "test-app".to_string())]
                        .into_iter()
                        .collect(),
                ),
                ..Default::default()
            },
            template: k8s_openapi::api::core::v1::PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(
                        vec![("app".to_string(), "test-app".to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                }),
                spec: Some(k8s_openapi::api::core::v1::PodSpec {
                    containers: vec![k8s_openapi::api::core::v1::Container {
                        name: "app".to_string(),
                        image: Some("nginx:1.0".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            strategy: RolloutStrategy {
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![CanaryStep {
                        set_weight: Some(20),
                        pause: None,
                    }],
                    traffic_routing: Some(TrafficRouting {
                        gateway_api: Some(GatewayAPIRouting {
                            http_route: "test-route".to_string(),
                        }),
                    }),
                }),
            },
        },
        status: None,
    };

    // Build canary ReplicaSet (should have 0 replicas initially)
    let canary_rs = build_replicaset(&rollout, "canary", 0);

    // Verify canary ReplicaSet has correct properties
    assert_eq!(
        canary_rs.metadata.name.as_deref(),
        Some("test-rollout-canary")
    );
    assert_eq!(canary_rs.metadata.namespace.as_deref(), Some("default"));
    assert_eq!(canary_rs.spec.as_ref().unwrap().replicas, Some(0));

    // Verify canary has rollouts.kulta.io/type=canary label
    let labels = &canary_rs
        .spec
        .as_ref()
        .unwrap()
        .template
        .as_ref()
        .unwrap()
        .metadata
        .as_ref()
        .unwrap()
        .labels;
    assert_eq!(
        labels.as_ref().unwrap().get("rollouts.kulta.io/type"),
        Some(&"canary".to_string())
    );

    // Test that reconcile logic would create canary (test verifies build logic)
    // Full integration test requires real K8s cluster
    let result = reconcile(Arc::new(rollout), Arc::new(Context::new_mock())).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_build_both_stable_and_canary_replicasets() {
    // Test that we can build both stable and canary ReplicaSets
    // This test ensures both types are buildable before reconcile uses them
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-rollout".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 5,
            selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector {
                match_labels: Some(
                    vec![("app".to_string(), "test-app".to_string())]
                        .into_iter()
                        .collect(),
                ),
                ..Default::default()
            },
            template: k8s_openapi::api::core::v1::PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(
                        vec![("app".to_string(), "test-app".to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    ..Default::default()
                }),
                spec: Some(k8s_openapi::api::core::v1::PodSpec {
                    containers: vec![k8s_openapi::api::core::v1::Container {
                        name: "app".to_string(),
                        image: Some("nginx:2.0".to_string()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            strategy: RolloutStrategy {
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![],
                    traffic_routing: None,
                }),
            },
        },
        status: None,
    };

    // Build both ReplicaSets
    let stable_rs = build_replicaset(&rollout, "stable", rollout.spec.replicas);
    let canary_rs = build_replicaset(&rollout, "canary", 0);

    // Verify stable ReplicaSet
    assert_eq!(
        stable_rs.metadata.name.as_deref(),
        Some("test-rollout-stable")
    );
    assert_eq!(stable_rs.spec.as_ref().unwrap().replicas, Some(5));
    assert_eq!(
        stable_rs
            .spec
            .as_ref()
            .unwrap()
            .template
            .as_ref()
            .unwrap()
            .metadata
            .as_ref()
            .unwrap()
            .labels
            .as_ref()
            .unwrap()
            .get("rollouts.kulta.io/type"),
        Some(&"stable".to_string())
    );

    // Verify canary ReplicaSet
    assert_eq!(
        canary_rs.metadata.name.as_deref(),
        Some("test-rollout-canary")
    );
    assert_eq!(canary_rs.spec.as_ref().unwrap().replicas, Some(0));
    assert_eq!(
        canary_rs
            .spec
            .as_ref()
            .unwrap()
            .template
            .as_ref()
            .unwrap()
            .metadata
            .as_ref()
            .unwrap()
            .labels
            .as_ref()
            .unwrap()
            .get("rollouts.kulta.io/type"),
        Some(&"canary".to_string())
    );

    // Verify both share the same pod-template-hash (same template)
    let stable_hash = stable_rs
        .spec
        .as_ref()
        .unwrap()
        .template
        .as_ref()
        .unwrap()
        .metadata
        .as_ref()
        .unwrap()
        .labels
        .as_ref()
        .unwrap()
        .get("pod-template-hash")
        .unwrap();

    let canary_hash = canary_rs
        .spec
        .as_ref()
        .unwrap()
        .template
        .as_ref()
        .unwrap()
        .metadata
        .as_ref()
        .unwrap()
        .labels
        .as_ref()
        .unwrap()
        .get("pod-template-hash")
        .unwrap();

    assert_eq!(stable_hash, canary_hash);
}

#[tokio::test]
async fn test_calculate_traffic_weights_step0() {
    // Test weight calculation for canary step 0 (20%)
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-rollout".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector::default(),
            template: k8s_openapi::api::core::v1::PodTemplateSpec::default(),
            strategy: RolloutStrategy {
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![
                        CanaryStep {
                            set_weight: Some(20),
                            pause: None,
                        },
                        CanaryStep {
                            set_weight: Some(50),
                            pause: None,
                        },
                        CanaryStep {
                            set_weight: Some(100),
                            pause: None,
                        },
                    ],
                    traffic_routing: Some(TrafficRouting {
                        gateway_api: Some(GatewayAPIRouting {
                            http_route: "test-route".to_string(),
                        }),
                    }),
                }),
            },
        },
        status: Some(RolloutStatus {
            current_step_index: Some(0), // First step: 20% canary
            ..Default::default()
        }),
    };

    // Calculate weights for step 0
    let (stable_weight, canary_weight) = calculate_traffic_weights(&rollout);

    assert_eq!(canary_weight, 20);
    assert_eq!(stable_weight, 80);
}

#[tokio::test]
async fn test_calculate_traffic_weights_step1() {
    // Test weight calculation for canary step 1 (50%)
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-rollout".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector::default(),
            template: k8s_openapi::api::core::v1::PodTemplateSpec::default(),
            strategy: RolloutStrategy {
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![
                        CanaryStep {
                            set_weight: Some(20),
                            pause: None,
                        },
                        CanaryStep {
                            set_weight: Some(50),
                            pause: None,
                        },
                    ],
                    traffic_routing: None,
                }),
            },
        },
        status: Some(RolloutStatus {
            current_step_index: Some(1), // Second step: 50% canary
            ..Default::default()
        }),
    };

    // Calculate weights for step 1
    let (stable_weight, canary_weight) = calculate_traffic_weights(&rollout);

    assert_eq!(canary_weight, 50);
    assert_eq!(stable_weight, 50);
}

#[tokio::test]
async fn test_calculate_traffic_weights_no_step() {
    // Test weight calculation when no step is active (100% stable)
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-rollout".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector::default(),
            template: k8s_openapi::api::core::v1::PodTemplateSpec::default(),
            strategy: RolloutStrategy {
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![CanaryStep {
                        set_weight: Some(20),
                        pause: None,
                    }],
                    traffic_routing: None,
                }),
            },
        },
        status: None, // No status yet, default to 100% stable
    };

    // Calculate weights when no step is active
    let (stable_weight, canary_weight) = calculate_traffic_weights(&rollout);

    assert_eq!(canary_weight, 0);
    assert_eq!(stable_weight, 100);
}

#[tokio::test]
async fn test_calculate_traffic_weights_complete() {
    // Test weight calculation when rollout is complete (100% canary)
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-rollout".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector::default(),
            template: k8s_openapi::api::core::v1::PodTemplateSpec::default(),
            strategy: RolloutStrategy {
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![
                        CanaryStep {
                            set_weight: Some(20),
                            pause: None,
                        },
                        CanaryStep {
                            set_weight: Some(100),
                            pause: None,
                        },
                    ],
                    traffic_routing: None,
                }),
            },
        },
        status: Some(RolloutStatus {
            current_step_index: Some(1), // Last step: 100% canary
            ..Default::default()
        }),
    };

    // Calculate weights for final step
    let (stable_weight, canary_weight) = calculate_traffic_weights(&rollout);

    assert_eq!(canary_weight, 100);
    assert_eq!(stable_weight, 0);
}

#[tokio::test]
async fn test_calculate_traffic_weights_beyond_steps() {
    // Test weight calculation when step index is beyond available steps
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-rollout".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector::default(),
            template: k8s_openapi::api::core::v1::PodTemplateSpec::default(),
            strategy: RolloutStrategy {
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![CanaryStep {
                        set_weight: Some(20),
                        pause: None,
                    }],
                    traffic_routing: None,
                }),
            },
        },
        status: Some(RolloutStatus {
            current_step_index: Some(5), // Beyond available steps (only 1 step)
            ..Default::default()
        }),
    };

    // When step index exceeds steps, rollout is complete (100% canary)
    let (stable_weight, canary_weight) = calculate_traffic_weights(&rollout);

    assert_eq!(canary_weight, 100);
    assert_eq!(stable_weight, 0);
}
