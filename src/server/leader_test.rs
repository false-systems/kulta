//! Tests for leader election

use super::leader::*;
use chrono::Utc;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime;
use std::time::Duration;

/// Test LeaderState initial value
#[test]
fn test_leader_state_initially_not_leader() {
    let state = LeaderState::new();
    assert!(!state.is_leader(), "Should not be leader initially");
}

/// Test LeaderState transitions
#[test]
fn test_leader_state_transitions() {
    let state = LeaderState::new();

    assert!(!state.is_leader());

    state.set_leader(true);
    assert!(state.is_leader());

    state.set_leader(false);
    assert!(!state.is_leader());
}

/// Test LeaderState clones share state
#[test]
fn test_leader_state_clones_share_state() {
    let state = LeaderState::new();
    let state2 = state.clone();

    assert!(!state.is_leader());
    assert!(!state2.is_leader());

    state.set_leader(true);

    assert!(state.is_leader());
    assert!(state2.is_leader(), "Clone should reflect same leader state");
}

/// Test LeaderConfig constants and structure
///
/// Note: We avoid testing env var behavior here due to race conditions
/// in parallel test execution. The from_env() function is simple enough
/// that code review suffices.
#[test]
fn test_leader_config_constants() {
    // Test that default constants are set correctly
    let config = LeaderConfig {
        holder_id: "test-holder".to_string(),
        lease_name: "kulta-controller-leader".to_string(),
        lease_namespace: "kulta-system".to_string(),
        lease_duration_seconds: DEFAULT_LEASE_TTL.as_secs() as i32,
        renew_interval: DEFAULT_RENEW_INTERVAL,
    };

    assert_eq!(config.lease_name, "kulta-controller-leader");
    assert_eq!(
        config.lease_duration_seconds,
        DEFAULT_LEASE_TTL.as_secs() as i32
    );
    assert_eq!(config.renew_interval, DEFAULT_RENEW_INTERVAL);
}

/// Test LeaderConfig::from_env reads POD_NAME when set
#[test]
fn test_leader_config_from_env_with_pod_name() {
    // Set unique values to reduce collision risk with parallel tests
    std::env::set_var("POD_NAME", "test-pod-unique-12345");
    std::env::set_var("POD_NAMESPACE", "test-ns-unique-12345");

    let config = LeaderConfig::from_env();

    assert_eq!(config.holder_id, "test-pod-unique-12345");
    assert_eq!(config.lease_namespace, "test-ns-unique-12345");

    // Clean up immediately
    std::env::remove_var("POD_NAME");
    std::env::remove_var("POD_NAMESPACE");
}

/// Test LeaderConfig::from_env falls back to HOSTNAME when POD_NAME not set
#[test]
fn test_leader_config_from_env_hostname_fallback() {
    // Clear POD_NAME to trigger HOSTNAME fallback
    std::env::remove_var("POD_NAME");
    std::env::set_var("HOSTNAME", "test-hostname-unique-67890");

    let config = LeaderConfig::from_env();

    assert_eq!(config.holder_id, "test-hostname-unique-67890");

    // Clean up
    std::env::remove_var("HOSTNAME");
}

/// Test LeaderConfig::from_env generates UUID when no env vars set
#[test]
fn test_leader_config_from_env_uuid_fallback() {
    // Clear all identity env vars
    std::env::remove_var("POD_NAME");
    std::env::remove_var("HOSTNAME");

    let config = LeaderConfig::from_env();

    // Should get UUID fallback with "kulta-" prefix
    assert!(
        config.holder_id.starts_with("kulta-"),
        "Expected UUID fallback with kulta- prefix, got: {}",
        config.holder_id
    );
    // UUID is 36 chars, so "kulta-" + UUID = 42 chars
    assert_eq!(config.holder_id.len(), 42);
}

/// Test LeaderConfig::from_env uses default namespace when not set
#[test]
fn test_leader_config_from_env_default_namespace() {
    std::env::remove_var("POD_NAMESPACE");
    std::env::set_var("POD_NAME", "test-pod-for-namespace-test");

    let config = LeaderConfig::from_env();

    assert_eq!(config.lease_namespace, "kulta-system");

    // Clean up
    std::env::remove_var("POD_NAME");
}

/// Test default constants are reasonable
#[test]
fn test_lease_timing_constants() {
    // Lease TTL should be reasonable (not too short, not too long)
    assert!(DEFAULT_LEASE_TTL >= Duration::from_secs(10));
    assert!(DEFAULT_LEASE_TTL <= Duration::from_secs(60));

    // Renew interval should be roughly 1/3 of TTL
    assert!(DEFAULT_RENEW_INTERVAL < DEFAULT_LEASE_TTL);
    assert!(DEFAULT_RENEW_INTERVAL >= Duration::from_secs(3));
}

// ─────────────────────────────────────────────────────────────────────────────
// Lease expiry calculation tests
// ─────────────────────────────────────────────────────────────────────────────

/// Test lease is not expired when within TTL
#[test]
fn test_lease_not_expired_within_ttl() {
    let now = Utc::now();
    let renew_time = MicroTime(now - chrono::Duration::seconds(5));
    let lease_duration = 15; // 15 seconds TTL

    let expired = is_lease_expired(Some(&renew_time), Some(lease_duration), now);
    assert!(!expired, "Lease should not be expired 5s into 15s TTL");
}

/// Test lease is expired when past TTL
#[test]
fn test_lease_expired_past_ttl() {
    let now = Utc::now();
    let renew_time = MicroTime(now - chrono::Duration::seconds(20));
    let lease_duration = 15; // 15 seconds TTL

    let expired = is_lease_expired(Some(&renew_time), Some(lease_duration), now);
    assert!(expired, "Lease should be expired 20s into 15s TTL");
}

/// Test lease is expired exactly at boundary
#[test]
fn test_lease_expired_at_boundary() {
    let now = Utc::now();
    let renew_time = MicroTime(now - chrono::Duration::seconds(15));
    let lease_duration = 15; // Exactly at expiry

    let expired = is_lease_expired(Some(&renew_time), Some(lease_duration), now);
    // At exactly the boundary, now > expiry is false (now == expiry)
    assert!(!expired, "Lease should not be expired at exact boundary");
}

/// Test lease is expired when just past boundary
#[test]
fn test_lease_expired_just_past_boundary() {
    let now = Utc::now();
    let renew_time = MicroTime(now - chrono::Duration::seconds(16));
    let lease_duration = 15; // 1 second past expiry

    let expired = is_lease_expired(Some(&renew_time), Some(lease_duration), now);
    assert!(expired, "Lease should be expired 1s past boundary");
}

/// Test lease with no renew time is treated as expired
#[test]
fn test_lease_expired_no_renew_time() {
    let now = Utc::now();

    let expired = is_lease_expired(None, Some(15), now);
    assert!(
        expired,
        "Lease with no renew time should be treated as expired"
    );
}

/// Test lease with no duration is treated as expired
#[test]
fn test_lease_expired_no_duration() {
    let now = Utc::now();
    let renew_time = MicroTime(now);

    let expired = is_lease_expired(Some(&renew_time), None, now);
    assert!(
        expired,
        "Lease with no duration should be treated as expired"
    );
}

/// Test lease with neither field is treated as expired
#[test]
fn test_lease_expired_neither_field() {
    let now = Utc::now();

    let expired = is_lease_expired(None, None, now);
    assert!(
        expired,
        "Lease with neither renew time nor duration should be expired"
    );
}
