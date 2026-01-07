use super::*;
use crate::crd::rollout::{
    CanaryStep, CanaryStrategy, Phase, Rollout, RolloutSpec, RolloutStatus, RolloutStrategy,
};
use kube::api::ObjectMeta;

// TDD Cycle 1: RED - Test that service.deployed event is emitted when rollout initializes
#[tokio::test]
async fn test_emit_service_deployed_on_initialization() {
    // ARRANGE: Create test rollout with no status (new rollout)
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-app".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: Default::default(),
            template: create_test_pod_template("nginx:1.0"),
            strategy: RolloutStrategy {
                simple: None,
                blue_green: None,
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![CanaryStep {
                        set_weight: Some(10),
                        pause: None,
                    }],
                    analysis: None,
                    traffic_routing: None,
                }),
            },

            max_surge: None,
            max_unavailable: None,
            progress_deadline_seconds: None,
        },
        status: None, // No status yet - this is a new rollout
    };

    // Create mock CDEvents sink
    let sink = CDEventsSink::new_mock();

    // Old status (None - new rollout)
    let old_status = None;

    // New status (Initializing → Progressing)
    let new_status = RolloutStatus {
        phase: Some(Phase::Progressing),
        current_step_index: Some(0),
        current_weight: Some(10),
        ..Default::default()
    };

    // ACT: Emit CDEvent for status change
    emit_status_change_event(&rollout, &old_status, &new_status, &sink)
        .await
        .unwrap();

    // ASSERT: Verify service.deployed event was emitted
    let events = sink.get_emitted_events();
    assert_eq!(events.len(), 1, "Expected exactly 1 event");

    let event = &events[0];

    // Use AttributesReader trait to access event.ty()
    use cloudevents::AttributesReader;
    assert_eq!(
        event.ty(),
        "dev.cdevents.service.deployed.0.2.0",
        "Expected service.deployed event"
    );

    // Verify event data contains expected CDEvent fields
    let data = event.data().expect("Event should have data");
    let json: serde_json::Value = match data {
        cloudevents::Data::Json(v) => v.clone(),
        _ => panic!("Expected JSON data"),
    };

    // Verify subject contains artifact_id (the container image)
    let artifact_id = &json["subject"]["content"]["artifactId"];
    assert_eq!(
        artifact_id.as_str(),
        Some("nginx:1.0"),
        "artifact_id should be the container image"
    );

    // Verify subject contains environment
    let environment_id = &json["subject"]["content"]["environment"]["id"];
    assert_eq!(
        environment_id.as_str(),
        Some("default/test-app"),
        "environment.id should be namespace/name"
    );
}

// TDD Cycle 2: RED - Test that service.upgraded event is emitted when canary progresses
#[tokio::test]
async fn test_emit_service_upgraded_on_step_progression() {
    // ARRANGE: Create test rollout
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-app".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: Default::default(),
            template: create_test_pod_template("nginx:2.0"),
            strategy: RolloutStrategy {
                simple: None,
                blue_green: None,
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![
                        CanaryStep {
                            set_weight: Some(10),
                            pause: None,
                        },
                        CanaryStep {
                            set_weight: Some(50),
                            pause: None,
                        },
                    ],
                    analysis: None,
                    traffic_routing: None,
                }),
            },

            max_surge: None,
            max_unavailable: None,
            progress_deadline_seconds: None,
        },
        status: None,
    };

    // Create mock CDEvents sink
    let sink = CDEventsSink::new_mock();

    // Old status (Progressing at step 0, weight 10%)
    let old_status = Some(RolloutStatus {
        phase: Some(Phase::Progressing),
        current_step_index: Some(0),
        current_weight: Some(10),
        ..Default::default()
    });

    // New status (Progressing at step 1, weight 50%)
    let new_status = RolloutStatus {
        phase: Some(Phase::Progressing),
        current_step_index: Some(1),
        current_weight: Some(50),
        ..Default::default()
    };

    // ACT: Emit CDEvent for status change
    emit_status_change_event(&rollout, &old_status, &new_status, &sink)
        .await
        .unwrap();

    // ASSERT: Verify service.upgraded event was emitted
    let events = sink.get_emitted_events();
    assert_eq!(events.len(), 1, "Expected exactly 1 event");

    let event = &events[0];

    // Use AttributesReader trait to access event.ty()
    use cloudevents::AttributesReader;
    assert_eq!(
        event.ty(),
        "dev.cdevents.service.upgraded.0.2.0",
        "Expected service.upgraded event"
    );

    // Verify event data contains expected CDEvent fields
    let data = event.data().expect("Event should have data");
    let json: serde_json::Value = match data {
        cloudevents::Data::Json(v) => v.clone(),
        _ => panic!("Expected JSON data"),
    };

    // Verify subject contains artifact_id (the container image)
    let artifact_id = &json["subject"]["content"]["artifactId"];
    assert_eq!(
        artifact_id.as_str(),
        Some("nginx:2.0"),
        "artifact_id should be the container image"
    );

    // Verify subject contains environment
    let environment_id = &json["subject"]["content"]["environment"]["id"];
    assert_eq!(
        environment_id.as_str(),
        Some("default/test-app"),
        "environment.id should be namespace/name"
    );

    // Verify step metadata in customData
    let kulta = &json["customData"]["kulta"];
    assert_eq!(kulta["step"]["index"], 1, "step index should be 1");
    assert_eq!(
        kulta["step"]["traffic_weight"], 50,
        "traffic weight should be 50"
    );
}

// TDD Cycle 3: RED - Test that service.rolledback event is emitted on failure
#[tokio::test]
async fn test_emit_service_rolledback_on_failure() {
    // ARRANGE: Create test rollout
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-app".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: Default::default(),
            template: create_test_pod_template("nginx:2.0"),
            strategy: RolloutStrategy {
                simple: None,
                blue_green: None,
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![CanaryStep {
                        set_weight: Some(50),
                        pause: None,
                    }],
                    analysis: None,
                    traffic_routing: None,
                }),
            },

            max_surge: None,
            max_unavailable: None,
            progress_deadline_seconds: None,
        },
        status: None,
    };

    // Create mock CDEvents sink
    let sink = CDEventsSink::new_mock();

    // Old status (Progressing at step 0, weight 50%)
    let old_status = Some(RolloutStatus {
        phase: Some(Phase::Progressing),
        current_step_index: Some(0),
        current_weight: Some(50),
        ..Default::default()
    });

    // New status (Failed - rollback triggered)
    let new_status = RolloutStatus {
        phase: Some(Phase::Failed),
        current_step_index: Some(0),
        current_weight: Some(0),
        ..Default::default()
    };

    // ACT: Emit CDEvent for status change
    emit_status_change_event(&rollout, &old_status, &new_status, &sink)
        .await
        .unwrap();

    // ASSERT: Verify service.rolledback event was emitted
    let events = sink.get_emitted_events();
    assert_eq!(events.len(), 1, "Expected exactly 1 event");

    let event = &events[0];

    // Use AttributesReader trait to access event.ty()
    use cloudevents::AttributesReader;
    assert_eq!(
        event.ty(),
        "dev.cdevents.service.rolledback.0.2.0",
        "Expected service.rolledback event"
    );

    // Verify event data contains expected CDEvent fields
    let data = event.data().expect("Event should have data");
    let json: serde_json::Value = match data {
        cloudevents::Data::Json(v) => v.clone(),
        _ => panic!("Expected JSON data"),
    };

    // Verify subject contains artifact_id (the container image)
    let artifact_id = &json["subject"]["content"]["artifactId"];
    assert_eq!(
        artifact_id.as_str(),
        Some("nginx:2.0"),
        "artifact_id should be the container image"
    );

    // Verify subject contains environment
    let environment_id = &json["subject"]["content"]["environment"]["id"];
    assert_eq!(
        environment_id.as_str(),
        Some("default/test-app"),
        "environment.id should be namespace/name"
    );

    // Verify failure reason in customData
    let kulta = &json["customData"]["kulta"];
    assert_eq!(
        kulta["decision"]["reason"].as_str(),
        Some("analysis_failed"),
        "decision reason should indicate analysis failure"
    );
}

// TDD Cycle 4: RED - Test that service.published event is emitted on completion
#[tokio::test]
async fn test_emit_service_published_on_completion() {
    // ARRANGE: Create test rollout
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-app".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: Default::default(),
            template: create_test_pod_template("nginx:2.0"),
            strategy: RolloutStrategy {
                simple: None,
                blue_green: None,
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![
                        CanaryStep {
                            set_weight: Some(50),
                            pause: None,
                        },
                        CanaryStep {
                            set_weight: Some(100),
                            pause: None,
                        },
                    ],
                    analysis: None,
                    traffic_routing: None,
                }),
            },

            max_surge: None,
            max_unavailable: None,
            progress_deadline_seconds: None,
        },
        status: None,
    };

    // Create mock CDEvents sink
    let sink = CDEventsSink::new_mock();

    // Old status (Progressing at final step, weight 100%)
    let old_status = Some(RolloutStatus {
        phase: Some(Phase::Progressing),
        current_step_index: Some(1),
        current_weight: Some(100),
        ..Default::default()
    });

    // New status (Completed - 100% traffic reached)
    let new_status = RolloutStatus {
        phase: Some(Phase::Completed),
        current_step_index: Some(1),
        current_weight: Some(100),
        ..Default::default()
    };

    // ACT: Emit CDEvent for status change
    emit_status_change_event(&rollout, &old_status, &new_status, &sink)
        .await
        .unwrap();

    // ASSERT: Verify service.published event was emitted
    let events = sink.get_emitted_events();
    assert_eq!(events.len(), 1, "Expected exactly 1 event");

    let event = &events[0];

    // Use AttributesReader trait to access event.ty()
    use cloudevents::AttributesReader;
    assert_eq!(
        event.ty(),
        "dev.cdevents.service.published.0.2.0",
        "Expected service.published event"
    );

    // Verify event data contains expected CDEvent fields
    let data = event.data().expect("Event should have data");
    let json: serde_json::Value = match data {
        cloudevents::Data::Json(v) => v.clone(),
        _ => panic!("Expected JSON data"),
    };

    // Verify subject contains environment (service.published doesn't have artifact_id)
    let environment_id = &json["subject"]["content"]["environment"]["id"];
    assert_eq!(
        environment_id.as_str(),
        Some("default/test-app"),
        "environment.id should be namespace/name"
    );

    // Verify completion reason in customData
    let kulta = &json["customData"]["kulta"];
    assert_eq!(
        kulta["decision"]["reason"].as_str(),
        Some("completed"),
        "decision reason should indicate completion"
    );
}

// TDD: Test that customData contains KULTA decision context
#[tokio::test]
async fn test_cdevent_contains_kulta_custom_data() {
    // ARRANGE: Create test rollout
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("test-app".to_string()),
            namespace: Some("default".to_string()),
            uid: Some("abc-123-def".to_string()),
            generation: Some(5),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: Default::default(),
            template: create_test_pod_template("nginx:2.0"),
            strategy: RolloutStrategy {
                simple: None,
                blue_green: None,
                canary: Some(CanaryStrategy {
                    canary_service: "test-app-canary".to_string(),
                    stable_service: "test-app-stable".to_string(),
                    steps: vec![
                        CanaryStep {
                            set_weight: Some(10),
                            pause: None,
                        },
                        CanaryStep {
                            set_weight: Some(50),
                            pause: None,
                        },
                    ],
                    analysis: None,
                    traffic_routing: None,
                }),
            },

            max_surge: None,
            max_unavailable: None,
            progress_deadline_seconds: None,
        },
        status: None,
    };

    let sink = CDEventsSink::new_mock();

    // Old status (step 0)
    let old_status = Some(RolloutStatus {
        phase: Some(Phase::Progressing),
        current_step_index: Some(0),
        current_weight: Some(10),
        ..Default::default()
    });

    // New status (step 1)
    let new_status = RolloutStatus {
        phase: Some(Phase::Progressing),
        current_step_index: Some(1),
        current_weight: Some(50),
        ..Default::default()
    };

    // ACT
    emit_status_change_event(&rollout, &old_status, &new_status, &sink)
        .await
        .unwrap();

    // ASSERT: Check customData exists and has kulta structure
    let events = sink.get_emitted_events();
    assert_eq!(events.len(), 1);

    let event = &events[0];

    // Get the data payload

    let data = event.data().expect("Event should have data");

    // Parse as JSON
    let json: serde_json::Value = match data {
        cloudevents::Data::Json(v) => v.clone(),
        _ => panic!("Expected JSON data"),
    };

    // Verify kulta customData structure
    let kulta = &json["customData"]["kulta"];
    assert_eq!(kulta["version"], "v1");
    assert_eq!(kulta["strategy"], "canary");
    assert_eq!(kulta["step"]["index"], 1);
    assert_eq!(kulta["step"]["traffic_weight"], 50);
    assert!(kulta["rollout"]["name"].as_str().is_some());
}

// TDD: Test that simple strategy emits both deployed and published events
#[tokio::test]
async fn test_simple_strategy_emits_deployed_and_published() {
    use crate::crd::rollout::SimpleStrategy;
    use cloudevents::AttributesReader;

    // ARRANGE: Create rollout with simple strategy
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("simple-app".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: Default::default(),
            template: create_test_pod_template("nginx:2.0"),
            strategy: RolloutStrategy {
                simple: Some(SimpleStrategy { analysis: None }),
                canary: None,
                blue_green: None,
            },

            max_surge: None,
            max_unavailable: None,
            progress_deadline_seconds: None,
        },
        status: None,
    };

    // Create mock CDEvents sink
    let sink = CDEventsSink::new_mock();

    // New status for simple strategy (directly Completed)
    let new_status = RolloutStatus {
        phase: Some(Phase::Completed),
        current_step_index: None,
        current_weight: None,
        message: Some("Simple rollout completed".to_string()),
        ..Default::default()
    };

    // ACT: Emit status change event (None → Completed)
    emit_status_change_event(&rollout, &None, &new_status, &sink)
        .await
        .expect("Event emission should succeed");

    // ASSERT: Both deployed and published events should be emitted
    let events = sink.get_emitted_events();
    assert_eq!(events.len(), 2, "Simple strategy should emit 2 events");

    // First event: service.deployed
    assert_eq!(
        events[0].ty(),
        "dev.cdevents.service.deployed.0.2.0",
        "First event should be service.deployed"
    );

    // Second event: service.published
    assert_eq!(
        events[1].ty(),
        "dev.cdevents.service.published.0.2.0",
        "Second event should be service.published"
    );
}

// TDD: Test that blue-green strategy emits deployed when entering Preview phase
#[tokio::test]
async fn test_blue_green_emits_deployed_on_preview() {
    use crate::crd::rollout::BlueGreenStrategy;
    use cloudevents::AttributesReader;

    // ARRANGE: Create rollout with blue-green strategy
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("blue-green-app".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: Default::default(),
            template: create_test_pod_template("nginx:2.0"),
            strategy: RolloutStrategy {
                simple: None,
                canary: None,
                blue_green: Some(BlueGreenStrategy {
                    active_service: "my-app-active".to_string(),
                    preview_service: "my-app-preview".to_string(),
                    auto_promotion_enabled: Some(true),
                    auto_promotion_seconds: Some(30),
                    traffic_routing: None,
                    analysis: None,
                }),
            },

            max_surge: None,
            max_unavailable: None,
            progress_deadline_seconds: None,
        },
        status: None,
    };

    let sink = CDEventsSink::new_mock();

    // New status: Preview phase (blue-green initialization)
    let new_status = RolloutStatus {
        phase: Some(Phase::Preview),
        current_step_index: None,
        current_weight: None,
        message: Some("Blue-green: preview environment ready".to_string()),
        ..Default::default()
    };

    // ACT: Emit status change event (None → Preview)
    emit_status_change_event(&rollout, &None, &new_status, &sink)
        .await
        .expect("Event emission should succeed");

    // ASSERT: service.deployed event emitted
    let events = sink.get_emitted_events();
    assert_eq!(
        events.len(),
        1,
        "Blue-green should emit 1 event on initialization"
    );
    assert_eq!(
        events[0].ty(),
        "dev.cdevents.service.deployed.0.2.0",
        "First event should be service.deployed"
    );

    // Verify customData contains blue-green strategy
    let data = events[0].data().expect("Event should have data");
    let json: serde_json::Value = match data {
        cloudevents::Data::Json(v) => v.clone(),
        _ => panic!("Expected JSON data"),
    };
    let kulta = &json["customData"]["kulta"];
    assert_eq!(kulta["strategy"], "blue-green");
}

// TDD: Test that blue-green emits published when promoted (Preview → Completed)
#[tokio::test]
async fn test_blue_green_emits_published_on_promotion() {
    use crate::crd::rollout::BlueGreenStrategy;
    use cloudevents::AttributesReader;

    // ARRANGE: Create rollout with blue-green strategy
    let rollout = Rollout {
        metadata: ObjectMeta {
            name: Some("blue-green-app".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: RolloutSpec {
            replicas: 3,
            selector: Default::default(),
            template: create_test_pod_template("nginx:2.0"),
            strategy: RolloutStrategy {
                simple: None,
                canary: None,
                blue_green: Some(BlueGreenStrategy {
                    active_service: "my-app-active".to_string(),
                    preview_service: "my-app-preview".to_string(),
                    auto_promotion_enabled: Some(true),
                    auto_promotion_seconds: Some(30),
                    traffic_routing: None,
                    analysis: None,
                }),
            },

            max_surge: None,
            max_unavailable: None,
            progress_deadline_seconds: None,
        },
        status: None,
    };

    let sink = CDEventsSink::new_mock();

    // Old status: Preview phase
    let old_status = Some(RolloutStatus {
        phase: Some(Phase::Preview),
        current_step_index: None,
        current_weight: None,
        ..Default::default()
    });

    // New status: Completed (promoted)
    let new_status = RolloutStatus {
        phase: Some(Phase::Completed),
        current_step_index: None,
        current_weight: None,
        message: Some("Blue-green: promoted to active".to_string()),
        ..Default::default()
    };

    // ACT: Emit status change event (Preview → Completed)
    emit_status_change_event(&rollout, &old_status, &new_status, &sink)
        .await
        .expect("Event emission should succeed");

    // ASSERT: service.published event emitted
    let events = sink.get_emitted_events();
    assert_eq!(events.len(), 1, "Blue-green promotion should emit 1 event");
    assert_eq!(
        events[0].ty(),
        "dev.cdevents.service.published.0.2.0",
        "Promotion should emit service.published"
    );

    // Verify customData contains blue-green strategy
    let data = events[0].data().expect("Event should have data");
    let json: serde_json::Value = match data {
        cloudevents::Data::Json(v) => v.clone(),
        _ => panic!("Expected JSON data"),
    };
    let kulta = &json["customData"]["kulta"];
    assert_eq!(kulta["strategy"], "blue-green");
}

// Helper to create test pod template
fn create_test_pod_template(image: &str) -> k8s_openapi::api::core::v1::PodTemplateSpec {
    use k8s_openapi::api::core::v1::{Container, PodSpec, PodTemplateSpec};

    PodTemplateSpec {
        metadata: Some(ObjectMeta {
            labels: Some([("app".to_string(), "test-app".to_string())].into()),
            ..Default::default()
        }),
        spec: Some(PodSpec {
            containers: vec![Container {
                name: "nginx".to_string(),
                image: Some(image.to_string()),
                ..Default::default()
            }],
            ..Default::default()
        }),
    }
}
