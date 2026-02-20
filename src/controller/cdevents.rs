//! CDEvents emission for rollout observability.
//! See the project documentation for specification.

use crate::crd::rollout::{Rollout, RolloutStatus};
use async_trait::async_trait;
use cloudevents::Event;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CDEventsError {
    #[error("cdevents error: {0}")]
    Generic(String),
}

/// Trait for sending CDEvents
///
/// Production code uses `HttpEventSink` which sends events via HTTP POST.
/// Tests use `MockEventSink` which stores events in memory for assertions.
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn send(&self, event: &Event) -> Result<(), CDEventsError>;
}

/// Production event sink that sends CloudEvents via HTTP POST
pub struct HttpEventSink {
    enabled: bool,
    sink_url: Option<String>,
}

impl Default for HttpEventSink {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpEventSink {
    /// Create a new HTTP event sink (production mode)
    ///
    /// Configuration from environment variables:
    /// - KULTA_CDEVENTS_ENABLED: "true" to enable CDEvents emission (default: false)
    /// - KULTA_CDEVENTS_SINK_URL: HTTP endpoint URL for CloudEvents (optional)
    pub fn new() -> Self {
        let enabled = std::env::var("KULTA_CDEVENTS_ENABLED")
            .unwrap_or_else(|_| "false".to_string())
            == "true";

        let sink_url = std::env::var("KULTA_CDEVENTS_SINK_URL").ok();

        HttpEventSink { enabled, sink_url }
    }
}

#[async_trait]
impl EventSink for HttpEventSink {
    async fn send(&self, event: &Event) -> Result<(), CDEventsError> {
        if !self.enabled {
            return Ok(()); // CDEvents disabled, skip
        }

        let Some(url) = &self.sink_url else {
            return Ok(()); // No sink URL configured, skip
        };

        // Send CloudEvent as JSON via HTTP POST
        let client = reqwest::Client::new();
        client
            .post(url)
            .header("Content-Type", "application/cloudevents+json")
            .json(event)
            .send()
            .await
            .map_err(|e| CDEventsError::Generic(format!("HTTP POST failed: {}", e)))?;

        Ok(())
    }
}

/// Mock event sink for testing - stores events in memory
#[cfg(test)]
pub struct MockEventSink {
    events: std::sync::Arc<std::sync::Mutex<Vec<Event>>>,
}

#[cfg(test)]
impl Default for MockEventSink {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl MockEventSink {
    pub fn new() -> Self {
        MockEventSink {
            events: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    #[allow(clippy::unwrap_used)]
    pub fn get_emitted_events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

#[cfg(test)]
#[async_trait]
impl EventSink for MockEventSink {
    async fn send(&self, event: &Event) -> Result<(), CDEventsError> {
        #[allow(clippy::unwrap_used)]
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}

/// Emit CDEvent based on status transition
///
/// This function determines which CDEvent to emit based on the phase transition
/// and sends it to the configured sink.
pub async fn emit_status_change_event(
    rollout: &Rollout,
    old_status: &Option<RolloutStatus>,
    new_status: &RolloutStatus,
    sink: &dyn EventSink,
) -> Result<(), CDEventsError> {
    use crate::crd::rollout::Phase;

    // Detect transition: None → Progressing/Completed/Preview/Experimenting = service.deployed
    // (Simple strategy goes directly to Completed, Canary goes to Progressing,
    // Blue-green goes to Preview, A/B Testing goes to Experimenting)
    let is_initialization = old_status.is_none()
        && matches!(
            new_status.phase,
            Some(Phase::Progressing)
                | Some(Phase::Completed)
                | Some(Phase::Preview)
                | Some(Phase::Experimenting)
        );

    // Detect A/B experiment conclusion: Experimenting → Concluded
    let is_experiment_concluded = match (old_status, &new_status.phase) {
        (Some(old), Some(Phase::Concluded)) => {
            matches!(old.phase, Some(Phase::Experimenting))
        }
        _ => false,
    };

    // Detect step progression: Progressing → Progressing (different step)
    let is_step_progression = match (old_status, &new_status.phase) {
        (Some(old), Some(Phase::Progressing)) => {
            matches!(old.phase, Some(Phase::Progressing))
                && old.current_step_index != new_status.current_step_index
        }
        _ => false,
    };

    // Detect rollback: Any → Failed
    let is_rollback = matches!(new_status.phase, Some(Phase::Failed));

    // Detect completion: Progressing → Completed
    let is_completion = matches!(new_status.phase, Some(Phase::Completed));

    if is_initialization {
        let event = build_service_deployed_event(rollout, new_status)?;
        sink.send(&event).await?;

        // For simple strategy (direct to Completed), also emit service.published
        if is_completion {
            let event = build_service_published_event(rollout, new_status)?;
            sink.send(&event).await?;
        }

        Ok(())
    } else if is_step_progression {
        let event = build_service_upgraded_event(rollout, new_status)?;
        sink.send(&event).await?;
        Ok(())
    } else if is_rollback {
        let event = build_service_rolledback_event(rollout, new_status)?;
        sink.send(&event).await?;
        Ok(())
    } else if is_experiment_concluded {
        let event = build_experiment_concluded_event(rollout, new_status)?;
        sink.send(&event).await?;
        Ok(())
    } else if is_completion {
        let event = build_service_published_event(rollout, new_status)?;
        sink.send(&event).await?;
        Ok(())
    } else {
        // No event for other transitions (yet)
        Ok(())
    }
}

/// Build a service.deployed CDEvent
fn build_service_deployed_event(
    rollout: &Rollout,
    status: &RolloutStatus,
) -> Result<Event, CDEventsError> {
    use cdevents_sdk::latest::service_deployed;
    use cdevents_sdk::{CDEvent, Subject};

    let image = extract_image_from_rollout(rollout)?;

    let namespace = rollout
        .metadata
        .namespace
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("rollout missing namespace".to_string()))?;
    let name = rollout
        .metadata
        .name
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("rollout missing name".to_string()))?;

    let cdevent = CDEvent::from(
        Subject::from(service_deployed::Content {
            artifact_id: image
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid artifact_id: {}", e)))?,
            environment: service_deployed::ContentEnvironment {
                id: format!("{}/{}", namespace, name).try_into().map_err(|e| {
                    CDEventsError::Generic(format!("Invalid environment id: {}", e))
                })?,
                source: Some(
                    format!(
                        "/apis/argoproj.io/v1alpha1/namespaces/{}/rollouts/{}",
                        namespace, name
                    )
                    .try_into()
                    .map_err(|e| {
                        CDEventsError::Generic(format!("Invalid environment source: {}", e))
                    })?,
                ),
            },
        })
        .with_id(
            format!("/rollouts/{}/initialization", name)
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid subject id: {}", e)))?,
        )
        .with_source(
            "https://kulta.io/controller"
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid subject source: {}", e)))?,
        ),
    )
    .with_id(
        uuid::Uuid::new_v4()
            .to_string()
            .try_into()
            .map_err(|e| CDEventsError::Generic(format!("Invalid event id: {}", e)))?,
    )
    .with_source(
        "https://kulta.io"
            .try_into()
            .map_err(|e| CDEventsError::Generic(format!("Invalid event source: {}", e)))?,
    )
    .with_custom_data(build_kulta_custom_data(rollout, status, "initialization"));

    let cloudevent: Event = cdevent
        .try_into()
        .map_err(|e| CDEventsError::Generic(format!("Failed to convert to CloudEvent: {}", e)))?;

    Ok(cloudevent)
}

/// Build a service.upgraded CDEvent
fn build_service_upgraded_event(
    rollout: &Rollout,
    status: &RolloutStatus,
) -> Result<Event, CDEventsError> {
    use cdevents_sdk::latest::service_upgraded;
    use cdevents_sdk::{CDEvent, Subject};

    // Extract image from rollout spec (artifact_id)
    let image = extract_image_from_rollout(rollout)?;

    // Extract namespace and name
    let namespace = rollout
        .metadata
        .namespace
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("Rollout missing namespace".to_string()))?;
    let name = rollout
        .metadata
        .name
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("Rollout missing name".to_string()))?;

    let step_index = status.current_step_index.unwrap_or(0);

    // Build CDEvent
    let cdevent = CDEvent::from(
        Subject::from(service_upgraded::Content {
            artifact_id: image
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid artifact_id: {}", e)))?,
            environment: service_upgraded::ContentEnvironment {
                id: format!("{}/{}", namespace, name).try_into().map_err(|e| {
                    CDEventsError::Generic(format!("Invalid environment id: {}", e))
                })?,
                source: Some(
                    format!(
                        "/apis/argoproj.io/v1alpha1/namespaces/{}/rollouts/{}",
                        namespace, name
                    )
                    .try_into()
                    .map_err(|e| {
                        CDEventsError::Generic(format!("Invalid environment source: {}", e))
                    })?,
                ),
            },
        })
        .with_id(
            format!("/rollouts/{}/step/{}", name, step_index)
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid subject id: {}", e)))?,
        )
        .with_source(
            "https://kulta.io/controller"
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid subject source: {}", e)))?,
        ),
    )
    .with_id(
        uuid::Uuid::new_v4()
            .to_string()
            .try_into()
            .map_err(|e| CDEventsError::Generic(format!("Invalid event id: {}", e)))?,
    )
    .with_source(
        "https://kulta.io"
            .try_into()
            .map_err(|e| CDEventsError::Generic(format!("Invalid event source: {}", e)))?,
    )
    .with_custom_data(build_kulta_custom_data(rollout, status, "step_advanced"));

    // Convert to CloudEvent
    let cloudevent: Event = cdevent
        .try_into()
        .map_err(|e| CDEventsError::Generic(format!("Failed to convert to CloudEvent: {}", e)))?;

    Ok(cloudevent)
}

/// Build a service.rolledback CDEvent
fn build_service_rolledback_event(
    rollout: &Rollout,
    status: &RolloutStatus,
) -> Result<Event, CDEventsError> {
    use cdevents_sdk::latest::service_rolledback;
    use cdevents_sdk::{CDEvent, Subject};

    let image = extract_image_from_rollout(rollout)?;

    let namespace = rollout
        .metadata
        .namespace
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("Rollout missing namespace".to_string()))?;
    let name = rollout
        .metadata
        .name
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("Rollout missing name".to_string()))?;

    let cdevent = CDEvent::from(
        Subject::from(service_rolledback::Content {
            artifact_id: image
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid artifact_id: {}", e)))?,
            environment: service_rolledback::ContentEnvironment {
                id: format!("{}/{}", namespace, name).try_into().map_err(|e| {
                    CDEventsError::Generic(format!("Invalid environment id: {}", e))
                })?,
                source: Some(
                    format!(
                        "/apis/argoproj.io/v1alpha1/namespaces/{}/rollouts/{}",
                        namespace, name
                    )
                    .try_into()
                    .map_err(|e| {
                        CDEventsError::Generic(format!("Invalid environment source: {}", e))
                    })?,
                ),
            },
        })
        .with_id(
            format!("/rollouts/{}/rollback", name)
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid subject id: {}", e)))?,
        )
        .with_source(
            "https://kulta.io/controller"
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid subject source: {}", e)))?,
        ),
    )
    .with_id(
        uuid::Uuid::new_v4()
            .to_string()
            .try_into()
            .map_err(|e| CDEventsError::Generic(format!("Invalid event id: {}", e)))?,
    )
    .with_source(
        "https://kulta.io"
            .try_into()
            .map_err(|e| CDEventsError::Generic(format!("Invalid event source: {}", e)))?,
    )
    .with_custom_data(build_kulta_custom_data(rollout, status, "analysis_failed"));

    let cloudevent: Event = cdevent
        .try_into()
        .map_err(|e| CDEventsError::Generic(format!("Failed to convert to CloudEvent: {}", e)))?;

    Ok(cloudevent)
}

/// Build a service.published CDEvent
fn build_service_published_event(
    rollout: &Rollout,
    status: &RolloutStatus,
) -> Result<Event, CDEventsError> {
    use cdevents_sdk::latest::service_published;
    use cdevents_sdk::{CDEvent, Subject};

    // Extract namespace and name
    let namespace = rollout
        .metadata
        .namespace
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("Rollout missing namespace".to_string()))?;
    let name = rollout
        .metadata
        .name
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("Rollout missing name".to_string()))?;

    // Build CDEvent
    let cdevent = CDEvent::from(
        Subject::from(service_published::Content {
            environment: Some(service_published::ContentEnvironment {
                id: format!("{}/{}", namespace, name).try_into().map_err(|e| {
                    CDEventsError::Generic(format!("Invalid environment id: {}", e))
                })?,
                source: Some(
                    format!(
                        "/apis/argoproj.io/v1alpha1/namespaces/{}/rollouts/{}",
                        namespace, name
                    )
                    .try_into()
                    .map_err(|e| {
                        CDEventsError::Generic(format!("Invalid environment source: {}", e))
                    })?,
                ),
            }),
        })
        .with_id(
            format!("/rollouts/{}/completed", name)
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid subject id: {}", e)))?,
        )
        .with_source(
            "https://kulta.io/controller"
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid subject source: {}", e)))?,
        ),
    )
    .with_id(
        uuid::Uuid::new_v4()
            .to_string()
            .try_into()
            .map_err(|e| CDEventsError::Generic(format!("Invalid event id: {}", e)))?,
    )
    .with_source(
        "https://kulta.io"
            .try_into()
            .map_err(|e| CDEventsError::Generic(format!("Invalid event source: {}", e)))?,
    )
    .with_custom_data(build_kulta_custom_data(rollout, status, "completed"));

    let cloudevent: Event = cdevent
        .try_into()
        .map_err(|e| CDEventsError::Generic(format!("Failed to convert to CloudEvent: {}", e)))?;

    Ok(cloudevent)
}

/// Build experiment.concluded CDEvent
///
/// Uses service.published as the base event type with experiment-specific custom data.
fn build_experiment_concluded_event(
    rollout: &Rollout,
    status: &RolloutStatus,
) -> Result<Event, CDEventsError> {
    use cdevents_sdk::latest::service_published;
    use cdevents_sdk::{CDEvent, Subject};

    let namespace = rollout
        .metadata
        .namespace
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("Rollout missing namespace".to_string()))?;
    let name = rollout
        .metadata
        .name
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("Rollout missing name".to_string()))?;

    let cdevent = CDEvent::from(
        Subject::from(service_published::Content {
            environment: Some(service_published::ContentEnvironment {
                id: format!("{}/{}", namespace, name).try_into().map_err(|e| {
                    CDEventsError::Generic(format!("Invalid environment id: {}", e))
                })?,
                source: Some(
                    format!(
                        "/apis/kulta.io/v1alpha1/namespaces/{}/rollouts/{}",
                        namespace, name
                    )
                    .try_into()
                    .map_err(|e| {
                        CDEventsError::Generic(format!("Invalid environment source: {}", e))
                    })?,
                ),
            }),
        })
        .with_id(
            format!("/rollouts/{}/experiment-concluded", name)
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid subject id: {}", e)))?,
        )
        .with_source(
            "https://kulta.io/controller"
                .try_into()
                .map_err(|e| CDEventsError::Generic(format!("Invalid subject source: {}", e)))?,
        ),
    )
    .with_id(
        uuid::Uuid::new_v4()
            .to_string()
            .try_into()
            .map_err(|e| CDEventsError::Generic(format!("Invalid event id: {}", e)))?,
    )
    .with_source(
        "https://kulta.io"
            .try_into()
            .map_err(|e| CDEventsError::Generic(format!("Invalid event source: {}", e)))?,
    )
    .with_custom_data(build_experiment_custom_data(rollout, status));

    let cloudevent: Event = cdevent
        .try_into()
        .map_err(|e| CDEventsError::Generic(format!("Failed to convert to CloudEvent: {}", e)))?;

    Ok(cloudevent)
}

/// Build experiment-specific custom data for CDEvents
fn build_experiment_custom_data(rollout: &Rollout, status: &RolloutStatus) -> serde_json::Value {
    let ab_experiment = status.ab_experiment.as_ref();

    let winner = ab_experiment
        .and_then(|ab| ab.winner.as_ref())
        .map(|v| format!("{:?}", v))
        .unwrap_or_else(|| "none".to_string());

    let conclusion_reason = ab_experiment
        .and_then(|ab| ab.conclusion_reason.as_ref())
        .map(|r| format!("{:?}", r))
        .unwrap_or_else(|| "unknown".to_string());

    let results: Vec<serde_json::Value> = ab_experiment
        .map(|ab| {
            ab.results
                .iter()
                .map(|r| {
                    json!({
                        "name": r.name,
                        "value_a": r.value_a,
                        "value_b": r.value_b,
                        "confidence": r.confidence,
                        "is_significant": r.is_significant,
                        "winner": r.winner
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    json!({
        "kulta": {
            "version": "v1",
            "rollout": {
                "name": rollout.metadata.name.as_deref().unwrap_or("unknown"),
                "namespace": rollout.metadata.namespace.as_deref().unwrap_or("default"),
                "uid": rollout.metadata.uid.as_deref().unwrap_or(""),
                "generation": rollout.metadata.generation.unwrap_or(0)
            },
            "strategy": "ab-testing",
            "experiment": {
                "started_at": ab_experiment.map(|ab| ab.started_at.as_str()).unwrap_or(""),
                "concluded_at": ab_experiment.and_then(|ab| ab.concluded_at.as_deref()).unwrap_or(""),
                "sample_size_a": ab_experiment.and_then(|ab| ab.sample_size_a).unwrap_or(0),
                "sample_size_b": ab_experiment.and_then(|ab| ab.sample_size_b).unwrap_or(0),
                "winner": winner,
                "conclusion_reason": conclusion_reason,
                "metrics": results
            },
            "decision": {
                "reason": "experiment_concluded"
            }
        }
    })
}

/// Build KULTA customData for CDEvents
fn build_kulta_custom_data(
    rollout: &Rollout,
    status: &RolloutStatus,
    decision_reason: &str,
) -> serde_json::Value {
    let strategy = if rollout.spec.strategy.canary.is_some() {
        "canary"
    } else if rollout.spec.strategy.blue_green.is_some() {
        "blue-green"
    } else if rollout.spec.strategy.ab_testing.is_some() {
        "ab-testing"
    } else {
        "simple"
    };

    let total_steps = rollout
        .spec
        .strategy
        .canary
        .as_ref()
        .map(|c| c.steps.len())
        .unwrap_or(0);

    json!({
        "kulta": {
            "version": "v1",
            "rollout": {
                "name": rollout.metadata.name.as_deref().unwrap_or("unknown"),
                "namespace": rollout.metadata.namespace.as_deref().unwrap_or("default"),
                "uid": rollout.metadata.uid.as_deref().unwrap_or(""),
                "generation": rollout.metadata.generation.unwrap_or(0)
            },
            "strategy": strategy,
            "step": {
                "index": status.current_step_index.unwrap_or(0),
                "total": total_steps,
                "traffic_weight": status.current_weight.unwrap_or(0)
            },
            "decision": {
                "reason": decision_reason
            }
        }
    })
}

/// Extract image from rollout's pod template
fn extract_image_from_rollout(rollout: &Rollout) -> Result<String, CDEventsError> {
    let containers = &rollout
        .spec
        .template
        .spec
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("pod template missing spec".to_string()))?
        .containers;

    let first_container = containers
        .first()
        .ok_or_else(|| CDEventsError::Generic("pod template has no containers".to_string()))?;

    let image = first_container
        .image
        .as_ref()
        .ok_or_else(|| CDEventsError::Generic("container missing image".to_string()))?;

    Ok(image.clone())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Tests can use unwrap/expect for brevity
#[path = "cdevents_test.rs"]
mod tests;
