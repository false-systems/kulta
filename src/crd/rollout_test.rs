#![allow(clippy::unwrap_used)] // Tests can use unwrap for brevity
#![allow(clippy::expect_used)] // Tests can use expect for better error messages

use super::*;
use kube::CustomResourceExt;

// TDD Cycle 1 (Blue-Green Strategy): RED - Test that strategy.blueGreen can be deserialized
#[test]
fn test_blue_green_strategy_deserialize_from_yaml() {
    let yaml = r#"
apiVersion: kulta.io/v1alpha1
kind: Rollout
metadata:
  name: blue-green-rollout
spec:
  replicas: 3
  selector:
    matchLabels:
      app: test-app
  template:
    metadata:
      labels:
        app: test-app
    spec:
      containers:
      - name: app
        image: nginx:2.0
  strategy:
    blueGreen:
      activeService: my-app-active
      previewService: my-app-preview
      autoPromotionEnabled: true
      autoPromotionSeconds: 30
      trafficRouting:
        gatewayAPI:
          httpRoute: my-app-route
"#;

    let rollout: Rollout = serde_yaml::from_str(yaml).expect("Failed to deserialize Rollout");

    assert_eq!(rollout.metadata.name.as_deref(), Some("blue-green-rollout"));
    assert_eq!(rollout.spec.replicas, 3);

    // Verify blue-green strategy is set
    assert!(rollout.spec.strategy.blue_green.is_some());
    assert!(rollout.spec.strategy.canary.is_none());
    assert!(rollout.spec.strategy.simple.is_none());

    let bg = rollout.spec.strategy.blue_green.unwrap();
    assert_eq!(bg.active_service, "my-app-active");
    assert_eq!(bg.preview_service, "my-app-preview");
    assert_eq!(bg.auto_promotion_enabled, Some(true));
    assert_eq!(bg.auto_promotion_seconds, Some(30));

    // Verify traffic routing
    let traffic = bg.traffic_routing.unwrap();
    assert_eq!(traffic.gateway_api.unwrap().http_route, "my-app-route");
}

// TDD Cycle 1 (Simple Strategy): Test that strategy.simple can be deserialized
#[test]
fn test_simple_strategy_deserialize_from_yaml() {
    let yaml = r#"
apiVersion: kulta.io/v1alpha1
kind: Rollout
metadata:
  name: simple-rollout
spec:
  replicas: 3
  selector:
    matchLabels:
      app: test-app
  template:
    metadata:
      labels:
        app: test-app
    spec:
      containers:
      - name: app
        image: nginx:1.20
  strategy:
    simple:
      analysis:
        prometheus:
          address: http://prometheus:9090
        metrics:
          - name: error-rate
            threshold: 0.01
"#;

    let rollout: Rollout = serde_yaml::from_str(yaml).expect("Failed to deserialize Rollout");

    assert_eq!(rollout.metadata.name.as_deref(), Some("simple-rollout"));
    assert_eq!(rollout.spec.replicas, 3);

    // Verify simple strategy is set
    assert!(rollout.spec.strategy.simple.is_some());
    assert!(rollout.spec.strategy.canary.is_none());

    let simple = rollout.spec.strategy.simple.unwrap();
    assert!(simple.analysis.is_some());
    let analysis = simple.analysis.unwrap();
    assert_eq!(
        analysis.prometheus.unwrap().address.as_deref(),
        Some("http://prometheus:9090")
    );
}

#[test]
fn test_rollout_deserialize_from_yaml() {
    let yaml = r#"
apiVersion: kulta.io/v1alpha1
kind: Rollout
metadata:
  name: test-rollout
spec:
  replicas: 3
  selector:
    matchLabels:
      app: test-app
  template:
    metadata:
      labels:
        app: test-app
    spec:
      containers:
      - name: app
        image: nginx:latest
  strategy:
    canary:
      canaryService: test-app-canary
      stableService: test-app-stable
      steps:
      - setWeight: 20
      - pause:
          duration: 30s
      - setWeight: 50
      trafficRouting:
        gatewayAPI:
          httpRoute: test-route
"#;

    let rollout: Rollout = serde_yaml::from_str(yaml).expect("Failed to deserialize Rollout");

    assert_eq!(rollout.metadata.name.as_deref(), Some("test-rollout"));
    assert_eq!(rollout.spec.replicas, 3);
    assert!(rollout.spec.strategy.canary.is_some());

    let canary = rollout.spec.strategy.canary.unwrap();
    assert_eq!(canary.canary_service, "test-app-canary");
    assert_eq!(canary.stable_service, "test-app-stable");
    assert_eq!(canary.steps.len(), 3);
    assert_eq!(canary.steps[0].set_weight, Some(20));
    assert!(canary.steps[1].pause.is_some());
    assert_eq!(canary.steps[2].set_weight, Some(50));

    assert!(canary.traffic_routing.is_some());
    let traffic = canary.traffic_routing.unwrap();
    assert!(traffic.gateway_api.is_some());
    assert_eq!(traffic.gateway_api.unwrap().http_route, "test-route");
}

#[test]
fn test_rollout_crd_schema_generation() {
    // Generate CRD YAML that gets installed in Kubernetes
    let crd = Rollout::crd();

    assert_eq!(crd.spec.group, "kulta.io");
    assert_eq!(crd.spec.names.kind, "Rollout");
    assert_eq!(crd.spec.names.plural, "rollouts");

    // Verify schema exists (validates CRD structure)
    assert!(!crd.spec.versions.is_empty());
    let version = &crd.spec.versions[0];
    assert_eq!(version.name, "v1alpha1");
    assert!(version.served);
    assert!(version.storage);
    assert!(version.schema.is_some());
}

#[test]
fn test_analysis_failure_policy() {
    let yaml = r#"
prometheus:
  address: http://prometheus:9090
failurePolicy: Pause
warmupDuration: 60s
metrics:
  - name: error-rate
    threshold: 0.01
"#;

    let config: AnalysisConfig = serde_yaml::from_str(yaml).expect("deserialize");
    assert_eq!(config.failure_policy, Some(FailurePolicy::Pause));

    // Test all variants serialize correctly
    assert_eq!(
        serde_json::to_string(&FailurePolicy::Pause).unwrap(),
        "\"Pause\""
    );
    assert_eq!(
        serde_json::to_string(&FailurePolicy::Continue).unwrap(),
        "\"Continue\""
    );
    assert_eq!(
        serde_json::to_string(&FailurePolicy::Rollback).unwrap(),
        "\"Rollback\""
    );
}

#[test]
fn test_status_decisions_serialization() {
    let status = RolloutStatus {
        phase: Some(Phase::Progressing),
        current_step_index: Some(1),
        current_weight: Some(50),
        decisions: vec![Decision {
            timestamp: "2024-12-01T10:00:00Z".to_string(),
            action: DecisionAction::StepAdvance,
            from_step: Some(0),
            to_step: Some(1),
            reason: DecisionReason::AnalysisPassed,
            message: None,
            metrics: None,
        }],
        ..Default::default()
    };

    let json = serde_json::to_string(&status).expect("serialize");
    assert!(json.contains("decisions"));
    assert!(json.contains("StepAdvance"));
    assert!(json.contains("AnalysisPassed"));

    // Roundtrip
    let parsed: RolloutStatus = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.decisions.len(), 1);
    assert_eq!(parsed.decisions[0].action, DecisionAction::StepAdvance);
}
