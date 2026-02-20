use crate::crd::rollout::Rollout;
use std::time::Duration;

/// Validate Rollout specification
///
/// Validates runtime constraints that cannot be enforced via CRD schema.
/// Used by both the reconcile loop (runtime) and the validating webhook (admission).
///
/// # Validation Rules
/// - `spec.replicas` must be >= 0
/// - Canary strategy: `canaryService` and `stableService` cannot be empty
/// - Canary strategy: `steps` must have at least one step
/// - Each step's `setWeight` must be 0-100
/// - `pause.duration` must be valid format (e.g., "30s", "5m")
///
/// # Arguments
/// * `rollout` - The Rollout resource to validate
///
/// # Returns
/// * `Ok(())` - Validation passed
/// * `Err(String)` - Validation error message
pub fn validate_rollout(rollout: &Rollout) -> Result<(), String> {
    // Validate replicas >= 0
    if rollout.spec.replicas < 0 {
        return Err(format!(
            "spec.replicas must be >= 0, got {}",
            rollout.spec.replicas
        ));
    }

    // Validate canary strategy if present
    if let Some(canary) = &rollout.spec.strategy.canary {
        // Validate canary service name is not empty
        if canary.canary_service.is_empty() {
            return Err("spec.strategy.canary.canaryService cannot be empty".to_string());
        }

        // Validate stable service name is not empty
        if canary.stable_service.is_empty() {
            return Err("spec.strategy.canary.stableService cannot be empty".to_string());
        }

        // Validate at least one step exists
        if canary.steps.is_empty() {
            return Err("spec.strategy.canary.steps must have at least one step".to_string());
        }

        // Validate each step
        for (i, step) in canary.steps.iter().enumerate() {
            // Validate setWeight is required and in 0-100 range
            match step.set_weight {
                Some(weight) => {
                    if !(0..=100).contains(&weight) {
                        return Err(format!(
                            "steps[{}].setWeight must be 0-100, got {}",
                            i, weight
                        ));
                    }
                }
                None => {
                    return Err(format!("steps[{}].setWeight is required", i));
                }
            }

            // Validate pause duration if present
            if let Some(pause) = &step.pause {
                if let Some(duration) = &pause.duration {
                    if parse_duration(duration).is_none() {
                        return Err(format!("steps[{}].pause.duration invalid: {}", i, duration));
                    }
                }
            }
        }

        // Validate traffic routing if present
        if let Some(traffic_routing) = &canary.traffic_routing {
            if let Some(gateway) = &traffic_routing.gateway_api {
                // Validate HTTPRoute name is not empty
                if gateway.http_route.is_empty() {
                    return Err(
                        "spec.strategy.canary.trafficRouting.gatewayAPI.httpRoute cannot be empty"
                            .to_string(),
                    );
                }
            }
        }
    }

    // Validate v1beta1 fields if present
    if let Some(max_surge) = &rollout.spec.max_surge {
        if !super::replicaset::is_valid_surge_format(max_surge) {
            return Err(format!(
                "spec.maxSurge invalid format '{}': must be percentage (e.g., '25%') or absolute number (e.g., '5')",
                max_surge
            ));
        }
    }

    if let Some(max_unavailable) = &rollout.spec.max_unavailable {
        if !super::replicaset::is_valid_surge_format(max_unavailable) {
            return Err(format!(
                "spec.maxUnavailable invalid format '{}': must be percentage (e.g., '25%') or absolute number (e.g., '0')",
                max_unavailable
            ));
        }
    }

    if let Some(deadline) = rollout.spec.progress_deadline_seconds {
        if deadline < 0 {
            return Err(format!(
                "spec.progressDeadlineSeconds must be >= 0, got {}",
                deadline
            ));
        }
    }

    Ok(())
}

/// Parse a duration string like "5m", "30s", "1h" into std::time::Duration
///
/// Supported formats:
/// - "30s" → 30 seconds (max 24h = 86400s)
/// - "5m" → 5 minutes (max 24h = 1440m)
/// - "2h" → 2 hours (max 1 week = 168h)
///
/// # Validation Rules
/// - Zero duration is rejected (minimum 1s)
/// - Seconds limited to 24h (86400s) - use hours for longer durations
/// - Minutes limited to 24h (1440m) - use hours for longer durations
/// - Hours limited to 1 week (168h) - prevents typos like "999999h"
///
/// # Arguments
/// * `duration_str` - Duration string to parse
///
/// # Returns
/// Some(Duration) if parse successful and within limits, None if invalid or out of range
pub fn parse_duration(duration_str: &str) -> Option<Duration> {
    let duration_str = duration_str.trim();

    if duration_str.is_empty() {
        return None;
    }

    // Get the last character (unit)
    let unit = duration_str.chars().last()?;

    // Get the numeric part
    let number_str = &duration_str[..duration_str.len() - 1];
    let number: u64 = number_str.parse().ok()?;

    // Reject zero duration
    if number == 0 {
        return None;
    }

    // Validate and convert based on unit
    match unit {
        's' => {
            // Seconds: max 24h (86400s)
            if number <= 86400 {
                Some(Duration::from_secs(number))
            } else {
                None // Reject: use hours for durations > 24h
            }
        }
        'm' => {
            // Minutes: max 24h (1440m)
            // Use checked_mul to prevent overflow
            if number <= 1440 {
                number.checked_mul(60).map(Duration::from_secs)
            } else {
                None // Reject: use hours for durations > 24h
            }
        }
        'h' => {
            // Hours: max 1 week (168h)
            // Use checked_mul to prevent overflow
            if number <= 168 {
                number.checked_mul(3600).map(Duration::from_secs)
            } else {
                None // Reject: likely a typo (e.g., "8760h" = 1 year)
            }
        }
        _ => None,
    }
}
