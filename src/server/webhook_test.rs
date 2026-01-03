//! Tests for CRD conversion webhook
//!
//! TDD: Tests written before implementation

use super::{convert_rollout, ConversionRequest};
use serde_json::json;

/// Test: Webhook converts v1alpha1 to v1beta1
#[test]
fn test_convert_v1alpha1_to_v1beta1() {
    let request = ConversionRequest {
        uid: "test-uid-123".to_string(),
        desired_api_version: "kulta.io/v1beta1".to_string(),
        objects: vec![json!({
            "apiVersion": "kulta.io/v1alpha1",
            "kind": "Rollout",
            "metadata": {
                "name": "test-rollout",
                "namespace": "default"
            },
            "spec": {
                "replicas": 3,
                "selector": {},
                "template": {},
                "strategy": {}
            }
        })],
    };

    let response = convert_rollout(request);

    assert_eq!(response.result.status, "Success");
    assert_eq!(response.uid, "test-uid-123");
    assert_eq!(response.converted_objects.len(), 1);

    let converted = &response.converted_objects[0];
    assert_eq!(converted["apiVersion"], "kulta.io/v1beta1");
    assert_eq!(converted["spec"]["maxSurge"], "25%");
    assert_eq!(converted["spec"]["maxUnavailable"], "0");
    assert_eq!(converted["spec"]["progressDeadlineSeconds"], 600);
}

/// Test: Webhook converts v1beta1 to v1alpha1
#[test]
fn test_convert_v1beta1_to_v1alpha1() {
    let request = ConversionRequest {
        uid: "test-uid-456".to_string(),
        desired_api_version: "kulta.io/v1alpha1".to_string(),
        objects: vec![json!({
            "apiVersion": "kulta.io/v1beta1",
            "kind": "Rollout",
            "metadata": {
                "name": "test-rollout",
                "namespace": "default"
            },
            "spec": {
                "replicas": 5,
                "selector": {},
                "template": {},
                "strategy": {},
                "maxSurge": "50%",
                "maxUnavailable": 2,
                "progressDeadlineSeconds": 900
            }
        })],
    };

    let response = convert_rollout(request);

    assert_eq!(response.result.status, "Success");
    assert_eq!(response.converted_objects.len(), 1);

    let converted = &response.converted_objects[0];
    assert_eq!(converted["apiVersion"], "kulta.io/v1alpha1");
    // v1alpha1 should NOT have the new fields
    assert!(converted["spec"].get("maxSurge").is_none());
    assert!(converted["spec"].get("maxUnavailable").is_none());
    assert!(converted["spec"].get("progressDeadlineSeconds").is_none());
    // But should preserve existing fields
    assert_eq!(converted["spec"]["replicas"], 5);
}

/// Test: Webhook handles multiple objects in single request
#[test]
fn test_convert_multiple_objects() {
    let request = ConversionRequest {
        uid: "batch-uid".to_string(),
        desired_api_version: "kulta.io/v1beta1".to_string(),
        objects: vec![
            json!({
                "apiVersion": "kulta.io/v1alpha1",
                "kind": "Rollout",
                "metadata": {"name": "rollout-1", "namespace": "default"},
                "spec": {"replicas": 1, "selector": {}, "template": {}, "strategy": {}}
            }),
            json!({
                "apiVersion": "kulta.io/v1alpha1",
                "kind": "Rollout",
                "metadata": {"name": "rollout-2", "namespace": "default"},
                "spec": {"replicas": 2, "selector": {}, "template": {}, "strategy": {}}
            }),
        ],
    };

    let response = convert_rollout(request);

    assert_eq!(response.result.status, "Success");
    assert_eq!(response.converted_objects.len(), 2);
    assert_eq!(response.converted_objects[0]["metadata"]["name"], "rollout-1");
    assert_eq!(response.converted_objects[1]["metadata"]["name"], "rollout-2");
}

/// Test: Webhook preserves metadata during conversion
#[test]
fn test_convert_preserves_metadata() {
    let request = ConversionRequest {
        uid: "meta-uid".to_string(),
        desired_api_version: "kulta.io/v1beta1".to_string(),
        objects: vec![json!({
            "apiVersion": "kulta.io/v1alpha1",
            "kind": "Rollout",
            "metadata": {
                "name": "my-rollout",
                "namespace": "production",
                "labels": {"app": "myapp"},
                "annotations": {"note": "test"}
            },
            "spec": {"replicas": 3, "selector": {}, "template": {}, "strategy": {}}
        })],
    };

    let response = convert_rollout(request);

    let converted = &response.converted_objects[0];
    assert_eq!(converted["metadata"]["name"], "my-rollout");
    assert_eq!(converted["metadata"]["namespace"], "production");
    assert_eq!(converted["metadata"]["labels"]["app"], "myapp");
    assert_eq!(converted["metadata"]["annotations"]["note"], "test");
}

/// Test: Webhook preserves status during conversion
#[test]
fn test_convert_preserves_status() {
    let request = ConversionRequest {
        uid: "status-uid".to_string(),
        desired_api_version: "kulta.io/v1beta1".to_string(),
        objects: vec![json!({
            "apiVersion": "kulta.io/v1alpha1",
            "kind": "Rollout",
            "metadata": {"name": "test", "namespace": "default"},
            "spec": {"replicas": 3, "selector": {}, "template": {}, "strategy": {}},
            "status": {
                "phase": "Progressing",
                "replicas": 3,
                "readyReplicas": 2,
                "currentWeight": 50
            }
        })],
    };

    let response = convert_rollout(request);

    let converted = &response.converted_objects[0];
    assert_eq!(converted["status"]["phase"], "Progressing");
    assert_eq!(converted["status"]["replicas"], 3);
    assert_eq!(converted["status"]["currentWeight"], 50);
}

/// Test: Webhook handles same-version "conversion" (no-op)
#[test]
fn test_convert_same_version_is_noop() {
    let request = ConversionRequest {
        uid: "noop-uid".to_string(),
        desired_api_version: "kulta.io/v1alpha1".to_string(),
        objects: vec![json!({
            "apiVersion": "kulta.io/v1alpha1",
            "kind": "Rollout",
            "metadata": {"name": "test", "namespace": "default"},
            "spec": {"replicas": 3, "selector": {}, "template": {}, "strategy": {}}
        })],
    };

    let response = convert_rollout(request);

    assert_eq!(response.result.status, "Success");
    // Object should be unchanged
    assert_eq!(response.converted_objects[0]["apiVersion"], "kulta.io/v1alpha1");
}

/// Test: Webhook returns error for unknown version
#[test]
fn test_convert_unknown_version_fails() {
    let request = ConversionRequest {
        uid: "error-uid".to_string(),
        desired_api_version: "kulta.io/v2".to_string(),
        objects: vec![json!({
            "apiVersion": "kulta.io/v1alpha1",
            "kind": "Rollout",
            "metadata": {"name": "test", "namespace": "default"},
            "spec": {"replicas": 3, "selector": {}, "template": {}, "strategy": {}}
        })],
    };

    let response = convert_rollout(request);

    assert_eq!(response.result.status, "Failed");
    assert!(response.result.message.is_some());
}
