use crate::crd::rollout::{Phase, Rollout, RolloutStatus};
use chrono::{DateTime, Utc};
use std::time::Duration;
use tracing::warn;

use super::validation::parse_duration;

/// Check if progress deadline has been exceeded
///
/// A rollout is considered stuck if:
/// - It's in Progressing or Preview phase
/// - progress_started_at is set
/// - Current time exceeds progress_started_at + deadline_seconds
///
/// # Arguments
/// * `status` - Current rollout status
/// * `deadline_seconds` - The progressDeadlineSeconds value
///
/// # Returns
/// true if the rollout has exceeded its progress deadline
pub fn is_progress_deadline_exceeded(
    status: &RolloutStatus,
    deadline_seconds: i32,
    now: DateTime<Utc>,
) -> bool {
    // Only check for active rollouts (Progressing or Preview)
    match &status.phase {
        Some(Phase::Progressing) | Some(Phase::Preview) => {}
        _ => return false,
    }

    // Need a start time to compare against
    let start_time = match &status.progress_started_at {
        Some(t) => t,
        None => return false,
    };

    // Parse the timestamp
    let started = match chrono::DateTime::parse_from_rfc3339(start_time) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(e) => {
            warn!(error = %e, timestamp = %start_time, "Failed to parse progress_started_at timestamp");
            return false;
        }
    };

    let elapsed = now.signed_duration_since(started);

    elapsed.num_seconds() > deadline_seconds as i64
}

/// Initialize RolloutStatus for a new Rollout
///
/// For simple strategy:
/// - phase = "Completed" (no steps to progress through)
///
/// For blue-green strategy:
/// - phase = "Preview"
/// - pause_start_time set for auto-promotion timer
///
/// For canary strategy:
/// - current_step_index = 0 (first step)
/// - phase = "Progressing"
/// - current_weight from first step's setWeight
///
/// # Arguments
/// * `rollout` - The Rollout to initialize status for
///
/// # Returns
/// RolloutStatus with initial values
pub fn initialize_rollout_status(rollout: &Rollout, now: DateTime<Utc>) -> RolloutStatus {
    // Check for simple strategy first
    if rollout.spec.strategy.simple.is_some() {
        // Simple strategy: no steps, just deploy and complete
        return RolloutStatus {
            phase: Some(Phase::Completed),
            current_step_index: None,
            current_weight: None,
            message: Some("Simple rollout completed: all replicas updated".to_string()),
            ..Default::default()
        };
    }

    // Check for blue-green strategy
    if rollout.spec.strategy.blue_green.is_some() {
        // Blue-green strategy: preview RS ready, awaiting promotion
        // Set pause_start_time to track when preview started (for auto-promotion timer)
        return RolloutStatus {
            phase: Some(Phase::Preview),
            current_step_index: None,
            current_weight: None,
            message: Some("Blue-green rollout: preview environment ready".to_string()),
            pause_start_time: Some(now.to_rfc3339()),
            ..Default::default()
        };
    }

    // Get canary strategy
    let canary_strategy = match &rollout.spec.strategy.canary {
        Some(strategy) => strategy,
        None => {
            // No strategy defined - return default status
            return RolloutStatus::default();
        }
    };

    // Get first step
    let first_step = canary_strategy.steps.first();

    // Get weight from first step (step 0)
    let first_step_weight = first_step.and_then(|step| step.set_weight).unwrap_or(0);

    let pause_start_time = first_step
        .filter(|step| step.pause.is_some())
        .map(|_| now.to_rfc3339());

    RolloutStatus {
        current_step_index: Some(0),
        current_weight: Some(first_step_weight),
        phase: Some(Phase::Progressing),
        message: Some(format!(
            "Starting canary rollout at step 0 ({}% traffic)",
            first_step_weight
        )),
        pause_start_time,
        progress_started_at: Some(now.to_rfc3339()),
        ..Default::default()
    }
}

/// Check if rollout should progress to next step
///
/// Returns true if:
/// - Current step has no pause defined
/// - Phase is not "Paused"
/// - Promote annotation is present (manual override)
/// - Timed pause duration has elapsed
///
/// # Arguments
/// * `rollout` - The Rollout to check
///
/// # Returns
/// true if should progress, false if should wait
pub fn should_progress_to_next_step(rollout: &Rollout, now: DateTime<Utc>) -> bool {
    // Get current status
    let status = match &rollout.status {
        Some(status) => status,
        None => return false, // No status yet, can't progress
    };

    // If phase is Paused, don't progress
    if status.phase == Some(Phase::Paused) {
        return false;
    }

    // Get current step index
    let current_step_index = match status.current_step_index {
        Some(idx) => idx,
        None => return false, // No step index, can't progress
    };

    // Get canary strategy
    let canary_strategy = match &rollout.spec.strategy.canary {
        Some(strategy) => strategy,
        None => return false, // No canary strategy
    };

    // Get current step
    let current_step = match canary_strategy.steps.get(current_step_index as usize) {
        Some(step) => step,
        None => return false, // Invalid step index
    };

    // Check if current step has pause
    if let Some(pause) = &current_step.pause {
        // Check for manual promotion annotation
        if has_promote_annotation(rollout) {
            return true; // Manual promotion overrides pause
        }

        // If pause has duration, check if elapsed
        if let Some(duration_str) = &pause.duration {
            if let Some(duration) = parse_duration(duration_str) {
                // Check if pause started
                if let Some(pause_start_str) = &status.pause_start_time {
                    // Parse pause start time (RFC3339)
                    match DateTime::parse_from_rfc3339(pause_start_str) {
                        Ok(pause_start) => {
                            let elapsed = now.signed_duration_since(pause_start);

                            // If duration elapsed, can progress
                            if elapsed.num_seconds() >= duration.as_secs() as i64 {
                                return true;
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, timestamp = %pause_start_str,
                                "Failed to parse pause_start_time timestamp, treating as still paused");
                        }
                    }
                }
            }
        }

        // Pause is active and duration not elapsed
        return false;
    }

    // No pause - can progress
    true
}

/// Compute the desired status for a Rollout
///
/// This is the main function called by reconcile() to determine what status
/// should be written to K8s. It orchestrates initialization and progression.
///
/// Logic:
/// - If no status: initialize with step 0
/// - If status exists and should progress: advance to next step
/// - Otherwise: keep current status
///
/// # Arguments
/// * `rollout` - The Rollout to compute status for
///
/// # Returns
/// The desired RolloutStatus that should be written to K8s
pub fn compute_desired_status(rollout: &Rollout, now: DateTime<Utc>) -> RolloutStatus {
    // If no status, initialize
    if rollout.status.is_none() {
        return initialize_rollout_status(rollout, now);
    }

    // If should progress, advance to next step
    if should_progress_to_next_step(rollout, now) {
        return advance_to_next_step(rollout, now);
    }

    // Otherwise, return current status (no change)
    // This should always exist since we checked is_none() above, but use unwrap_or_default for safety
    rollout.status.as_ref().cloned().unwrap_or_default()
}

/// Advance rollout to next step
///
/// Calculates new status with:
/// - current_step_index incremented
/// - current_weight from new step
/// - phase = "Completed" if last step, else "Progressing"
///
/// # Arguments
/// * `rollout` - The Rollout to advance
///
/// # Returns
/// New RolloutStatus with updated step
pub fn advance_to_next_step(rollout: &Rollout, now: DateTime<Utc>) -> RolloutStatus {
    // Get current status
    let current_status = match &rollout.status {
        Some(status) => status,
        None => {
            // No status yet - initialize
            return initialize_rollout_status(rollout, now);
        }
    };

    // Get current step index
    let current_step_index = current_status.current_step_index.unwrap_or(-1);
    let next_step_index = current_step_index + 1;

    // Get canary strategy
    let canary_strategy = match &rollout.spec.strategy.canary {
        Some(strategy) => strategy,
        None => {
            // No canary strategy - return current status
            return current_status.clone();
        }
    };

    // Check if next step exists
    if next_step_index as usize >= canary_strategy.steps.len() {
        // Reached end of steps - mark as completed
        return RolloutStatus {
            current_step_index: Some(next_step_index),
            current_weight: Some(100),
            phase: Some(Phase::Completed),
            message: Some("Rollout completed: 100% traffic to canary".to_string()),
            ..current_status.clone()
        };
    }

    // Get weight from next step
    let next_step = &canary_strategy.steps[next_step_index as usize];
    let next_weight = next_step.set_weight.unwrap_or(0);

    // Check if this is the final step (100% canary)
    let (phase, message) = if next_weight == 100 {
        (
            Phase::Completed,
            "Rollout completed: 100% traffic to canary".to_string(),
        )
    } else {
        (
            Phase::Progressing,
            format!(
                "Advanced to step {} ({}% traffic)",
                next_step_index, next_weight
            ),
        )
    };

    // Check if next step has pause - set pause start time
    let pause_start_time = if next_step.pause.is_some() {
        // Set pause start time to now (RFC3339)
        Some(now.to_rfc3339())
    } else {
        // Clear pause start time if no pause
        None
    };

    RolloutStatus {
        current_step_index: Some(next_step_index),
        current_weight: Some(next_weight),
        phase: Some(phase),
        message: Some(message),
        pause_start_time,
        ..current_status.clone()
    }
}

/// Calculate optimal requeue interval based on rollout pause state
///
/// This function reduces unnecessary API calls by calculating the next check time
/// based on the pause duration and elapsed time.
///
/// # Arguments
/// * `pause_start` - Optional pause start timestamp
/// * `pause_duration` - Optional pause duration
///
/// # Returns
/// * Optimal requeue interval (minimum 5s, maximum 300s)
///
/// # Examples
/// ```ignore
/// use chrono::{Utc, Duration as ChronoDuration};
/// use std::time::Duration;
///
/// // Paused with 10s duration, 2s elapsed
/// let pause_start = Utc::now() - ChronoDuration::seconds(2);
/// let pause_duration = Duration::from_secs(10);
/// let interval = calculate_requeue_interval(Some(&pause_start), Some(pause_duration));
/// assert!(interval.as_secs() >= 8 && interval.as_secs() <= 10);
///
/// // Not paused
/// let interval = calculate_requeue_interval(None, None);
/// assert_eq!(interval, Duration::from_secs(30));
/// ```
pub(crate) fn calculate_requeue_interval(
    pause_start: Option<&DateTime<Utc>>,
    pause_duration: Option<Duration>,
    now: DateTime<Utc>,
) -> Duration {
    const MIN_REQUEUE: Duration = Duration::from_secs(5); // Minimum 5s
    const MAX_REQUEUE: Duration = Duration::from_secs(300); // Maximum 5min
    const DEFAULT_REQUEUE: Duration = Duration::from_secs(30); // Default 30s

    match (pause_start, pause_duration) {
        (Some(start), Some(duration)) => {
            // Calculate elapsed time since pause started
            let elapsed = now.signed_duration_since(*start);
            let elapsed_secs = elapsed.num_seconds().max(0) as u64;

            // Calculate remaining time until pause completes
            let remaining_secs = duration.as_secs().saturating_sub(elapsed_secs);

            // Clamp to MIN..MAX range
            let optimal = Duration::from_secs(remaining_secs);
            optimal.clamp(MIN_REQUEUE, MAX_REQUEUE)
        }
        _ => {
            // No pause or manual pause → use default interval
            DEFAULT_REQUEUE
        }
    }
}

/// Helper to extract pause information from Rollout and RolloutStatus
pub(crate) fn calculate_requeue_interval_from_rollout(
    rollout: &Rollout,
    status: &RolloutStatus,
    now: DateTime<Utc>,
) -> Duration {
    let pause_start = status
        .pause_start_time
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    // Get current step's pause duration
    let pause_duration = status.current_step_index.and_then(|step_index| {
        rollout
            .spec
            .strategy
            .canary
            .as_ref()
            .and_then(|canary| canary.steps.get(step_index as usize))
            .and_then(|step| step.pause.as_ref())
            .and_then(|pause| pause.duration.as_ref())
            .and_then(|dur_str| parse_duration(dur_str))
    });

    calculate_requeue_interval(pause_start.as_ref(), pause_duration, now)
}

/// Check if Rollout has the promote annotation (kulta.io/promote=true)
///
/// This annotation is used to manually promote a rollout that is paused.
/// When present with value "true", the controller will progress to the next step
/// regardless of pause duration.
///
/// Used by canary strategy to skip pause and by blue-green to transition Preview → Completed.
///
/// # Arguments
/// * `rollout` - The Rollout to check
///
/// # Returns
/// true if annotation exists with value "true", false otherwise
pub fn has_promote_annotation(rollout: &Rollout) -> bool {
    rollout
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get("kulta.io/promote"))
        .map(|value| value == "true")
        .unwrap_or(false)
}
