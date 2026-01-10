//! Tests for CRD version conversion (v1alpha1 <-> v1beta1)
//!
//! TDD: These tests are written FIRST, before the implementation.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::{convert_to_v1alpha1, convert_to_v1beta1};
use crate::crd::v1alpha1;
use crate::crd::v1beta1;

/// Test: v1alpha1 -> v1beta1 adds default maxSurge
#[test]
fn test_v1alpha1_to_v1beta1_adds_default_max_surge() {
    let v1alpha1_spec = v1alpha1::RolloutSpec {
        replicas: 3,
        selector: Default::default(),
        template: Default::default(),
        strategy: v1alpha1::RolloutStrategy::default(),
        max_surge: None,
        max_unavailable: None,
        progress_deadline_seconds: None,
    };

    let v1beta1_spec = convert_to_v1beta1(&v1alpha1_spec);

    // Should have default maxSurge of "25%"
    assert_eq!(v1beta1_spec.max_surge, Some("25%".to_string()));
}

/// Test: v1alpha1 -> v1beta1 adds default maxUnavailable
#[test]
fn test_v1alpha1_to_v1beta1_adds_default_max_unavailable() {
    let v1alpha1_spec = v1alpha1::RolloutSpec {
        replicas: 3,
        selector: Default::default(),
        template: Default::default(),
        strategy: v1alpha1::RolloutStrategy::default(),
        max_surge: None,
        max_unavailable: None,
        progress_deadline_seconds: None,
    };

    let v1beta1_spec = convert_to_v1beta1(&v1alpha1_spec);

    // Should have default maxUnavailable of "0"
    assert_eq!(v1beta1_spec.max_unavailable, Some("0".to_string()));
}

/// Test: v1alpha1 -> v1beta1 adds default progressDeadlineSeconds
#[test]
fn test_v1alpha1_to_v1beta1_adds_default_progress_deadline() {
    let v1alpha1_spec = v1alpha1::RolloutSpec {
        replicas: 3,
        selector: Default::default(),
        template: Default::default(),
        strategy: v1alpha1::RolloutStrategy::default(),
        max_surge: None,
        max_unavailable: None,
        progress_deadline_seconds: None,
    };

    let v1beta1_spec = convert_to_v1beta1(&v1alpha1_spec);

    // Should have default progressDeadlineSeconds of 600
    assert_eq!(v1beta1_spec.progress_deadline_seconds, Some(600));
}

/// Test: v1alpha1 -> v1beta1 preserves all existing fields
#[test]
fn test_v1alpha1_to_v1beta1_preserves_existing_fields() {
    let v1alpha1_spec = v1alpha1::RolloutSpec {
        replicas: 5,
        selector: Default::default(),
        template: Default::default(),
        strategy: v1alpha1::RolloutStrategy {
            simple: None,
            canary: Some(v1alpha1::CanaryStrategy {
                canary_service: "my-canary".to_string(),
                stable_service: "my-stable".to_string(),
                steps: vec![v1alpha1::CanaryStep {
                    set_weight: Some(20),
                    pause: None,
                }],
                traffic_routing: None,
                analysis: None,
            }),
            blue_green: None,
        },
        max_surge: None,
        max_unavailable: None,
        progress_deadline_seconds: None,
    };

    let v1beta1_spec = convert_to_v1beta1(&v1alpha1_spec);

    // Replicas preserved
    assert_eq!(v1beta1_spec.replicas, 5);

    // Canary strategy preserved
    let canary = v1beta1_spec
        .strategy
        .canary
        .as_ref()
        .expect("canary should exist");
    assert_eq!(canary.canary_service, "my-canary");
    assert_eq!(canary.stable_service, "my-stable");
    assert_eq!(canary.steps.len(), 1);
    assert_eq!(canary.steps[0].set_weight, Some(20));
}

/// Test: v1beta1 -> v1alpha1 drops new fields
#[test]
fn test_v1beta1_to_v1alpha1_drops_new_fields() {
    let v1beta1_spec = v1beta1::RolloutSpec {
        replicas: 3,
        selector: Default::default(),
        template: Default::default(),
        strategy: v1beta1::RolloutStrategy::default(),
        max_surge: Some("50%".to_string()),
        max_unavailable: Some("1".to_string()),
        progress_deadline_seconds: Some(300),
    };

    let v1alpha1_spec = convert_to_v1alpha1(&v1beta1_spec);

    // v1alpha1 doesn't have these fields, so they're dropped
    // The struct simply won't have them - this test verifies it compiles
    // and preserves the fields that DO exist
    assert_eq!(v1alpha1_spec.replicas, 3);
}

/// Test: v1beta1 -> v1alpha1 preserves existing fields
#[test]
fn test_v1beta1_to_v1alpha1_preserves_existing_fields() {
    let v1beta1_spec = v1beta1::RolloutSpec {
        replicas: 10,
        selector: Default::default(),
        template: Default::default(),
        strategy: v1beta1::RolloutStrategy {
            simple: None,
            canary: Some(v1beta1::CanaryStrategy {
                canary_service: "svc-canary".to_string(),
                stable_service: "svc-stable".to_string(),
                steps: vec![],
                traffic_routing: None,
                analysis: None,
            }),
            blue_green: None,
        },
        max_surge: Some("25%".to_string()),
        max_unavailable: Some("0".to_string()),
        progress_deadline_seconds: Some(600),
    };

    let v1alpha1_spec = convert_to_v1alpha1(&v1beta1_spec);

    assert_eq!(v1alpha1_spec.replicas, 10);
    let canary = v1alpha1_spec
        .strategy
        .canary
        .as_ref()
        .expect("canary should exist");
    assert_eq!(canary.canary_service, "svc-canary");
    assert_eq!(canary.stable_service, "svc-stable");
}

/// Test: Round-trip conversion preserves data (v1alpha1 -> v1beta1 -> v1alpha1)
#[test]
fn test_roundtrip_v1alpha1_to_v1beta1_to_v1alpha1() {
    let original = v1alpha1::RolloutSpec {
        replicas: 7,
        selector: Default::default(),
        template: Default::default(),
        strategy: v1alpha1::RolloutStrategy {
            simple: Some(v1alpha1::SimpleStrategy { analysis: None }),
            canary: None,
            blue_green: None,
        },
        max_surge: None,
        max_unavailable: None,
        progress_deadline_seconds: None,
    };

    let converted = convert_to_v1beta1(&original);
    let back = convert_to_v1alpha1(&converted);

    assert_eq!(back.replicas, original.replicas);
    assert!(back.strategy.simple.is_some());
}

/// Test: Round-trip conversion preserves data (v1beta1 -> v1alpha1 -> v1beta1)
/// Note: New fields are lost in this direction
#[test]
fn test_roundtrip_v1beta1_to_v1alpha1_to_v1beta1() {
    let original = v1beta1::RolloutSpec {
        replicas: 4,
        selector: Default::default(),
        template: Default::default(),
        strategy: v1beta1::RolloutStrategy::default(),
        max_surge: Some("50%".to_string()),
        max_unavailable: Some("2".to_string()),
        progress_deadline_seconds: Some(900),
    };

    let converted = convert_to_v1alpha1(&original);
    let back = convert_to_v1beta1(&converted);

    // Replicas preserved
    assert_eq!(back.replicas, original.replicas);

    // New fields get DEFAULT values (not original) because v1alpha1 doesn't have them
    assert_eq!(back.max_surge, Some("25%".to_string())); // default, not "50%"
    assert_eq!(back.max_unavailable, Some("0".to_string())); // default, not "2"
    assert_eq!(back.progress_deadline_seconds, Some(600)); // default, not 900
}
