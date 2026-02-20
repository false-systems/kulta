use crate::crd::rollout::{Phase, Rollout};
use serde::{Deserialize, Serialize};

/// Get the service port from strategy configuration, defaulting to 80
pub fn default_service_port(configured: Option<i32>) -> i32 {
    configured.unwrap_or(80)
}

/// Simple representation of HTTPBackendRef for testing
///
/// This is a simplified version of Gateway API HTTPBackendRef
/// focused on what we need for weight-based traffic splitting
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HTTPBackendRef {
    /// Name of the Kubernetes Service
    pub name: String,

    /// Port number on the service
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,

    /// Weight for traffic splitting (0-100)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<i32>,
}

/// Build HTTPRoute backendRefs with weights from Rollout
///
/// Creates a list of backend references with calculated weights:
/// - Stable service with calculated stable_weight
/// - Canary service with calculated canary_weight
///
/// # Returns
/// Vec of HTTPBackendRef with correct weights for current rollout step
pub fn build_backend_refs_with_weights(rollout: &Rollout) -> Vec<HTTPBackendRef> {
    // Get canary strategy
    let canary_strategy = match &rollout.spec.strategy.canary {
        Some(strategy) => strategy,
        None => return vec![], // No canary strategy
    };

    // Calculate current weights
    let (stable_weight, canary_weight) = calculate_traffic_weights(rollout);

    let port = default_service_port(canary_strategy.port);

    vec![
        HTTPBackendRef {
            name: canary_strategy.stable_service.clone(),
            port: Some(port),
            weight: Some(stable_weight),
        },
        HTTPBackendRef {
            name: canary_strategy.canary_service.clone(),
            port: Some(port),
            weight: Some(canary_weight),
        },
    ]
}

/// Build Gateway API HTTPRouteRulesBackendRefs with weights from Rollout
///
/// Converts our simple HTTPBackendRef representation to the actual Gateway API
/// HTTPRouteRulesBackendRefs type used in HTTPRoute resources.
///
/// Supports both canary and blue-green strategies:
/// - Canary: Gradual traffic shift based on step weights
/// - Blue-green: 100/0 split, flips on promotion
///
/// # Returns
/// Vec of HTTPRouteRulesBackendRefs with correct weights for current rollout step
pub fn build_gateway_api_backend_refs(
    rollout: &Rollout,
) -> Vec<gateway_api::apis::standard::httproutes::HTTPRouteRulesBackendRefs> {
    use gateway_api::apis::standard::httproutes::HTTPRouteRulesBackendRefs;

    // Check for blue-green strategy first
    if let Some(blue_green) = &rollout.spec.strategy.blue_green {
        let (active_weight, preview_weight) = calculate_blue_green_weights(rollout);
        let port = default_service_port(blue_green.port);

        return vec![
            HTTPRouteRulesBackendRefs {
                name: blue_green.active_service.clone(),
                port: Some(port),
                weight: Some(active_weight),
                kind: Some("Service".to_string()),
                group: Some("".to_string()),
                namespace: None,
                filters: None,
            },
            HTTPRouteRulesBackendRefs {
                name: blue_green.preview_service.clone(),
                port: Some(port),
                weight: Some(preview_weight),
                kind: Some("Service".to_string()),
                group: Some("".to_string()),
                namespace: None,
                filters: None,
            },
        ];
    }

    // Get canary strategy
    let canary_strategy = match &rollout.spec.strategy.canary {
        Some(strategy) => strategy,
        None => return vec![],
    };

    // Calculate current weights
    let (stable_weight, canary_weight) = calculate_traffic_weights(rollout);
    let port = default_service_port(canary_strategy.port);

    vec![
        HTTPRouteRulesBackendRefs {
            name: canary_strategy.stable_service.clone(),
            port: Some(port),
            weight: Some(stable_weight),
            kind: Some("Service".to_string()),
            group: Some("".to_string()),
            namespace: None,
            filters: None,
        },
        HTTPRouteRulesBackendRefs {
            name: canary_strategy.canary_service.clone(),
            port: Some(port),
            weight: Some(canary_weight),
            kind: Some("Service".to_string()),
            group: Some("".to_string()),
            namespace: None,
            filters: None,
        },
    ]
}

/// Calculate traffic weights for blue-green strategy
///
/// Returns (active_weight, preview_weight):
/// - Preview phase: 100% active, 0% preview (testing preview env)
/// - Completed phase: 0% active, 100% preview (promoted)
/// - Other phases: 100% active, 0% preview (safe default)
pub fn calculate_blue_green_weights(rollout: &Rollout) -> (i32, i32) {
    let phase = rollout
        .status
        .as_ref()
        .and_then(|s| s.phase.as_ref())
        .cloned()
        .unwrap_or(Phase::Initializing);

    match phase {
        Phase::Completed => (0, 100), // Promoted: all traffic to preview (new active)
        _ => (100, 0),                // Preview/other: all traffic to active
    }
}

/// Update HTTPRoute's backend refs with weighted backends from Rollout
///
/// This function mutates the HTTPRoute by updating the first rule's backend_refs
/// with the weighted backends calculated from the Rollout's current step.
///
/// # Arguments
/// * `rollout` - The Rollout resource with traffic weights
/// * `httproute` - The HTTPRoute resource to update (mutated in place)
///
/// # Behavior
/// - Updates the first rule's backend_refs (assumes single rule)
/// - Replaces existing backend_refs with weighted stable + canary
/// - Uses build_gateway_api_backend_refs() for the conversion
pub fn update_httproute_backends(
    rollout: &Rollout,
    httproute: &mut gateway_api::apis::standard::httproutes::HTTPRoute,
) {
    // Get the weighted backend refs from rollout
    let backend_refs = build_gateway_api_backend_refs(rollout);

    // Update the first rule's backend_refs
    // (KULTA assumes HTTPRoute has exactly one rule - the traffic splitting rule)
    if let Some(rules) = httproute.spec.rules.as_mut() {
        if let Some(first_rule) = rules.first_mut() {
            first_rule.backend_refs = Some(backend_refs);
        }
    }
}

/// Calculate traffic weights for stable and canary based on Rollout status
///
/// Returns (stable_weight, canary_weight) as percentages
///
/// # Logic
/// - If no status or no currentStepIndex: 100% stable, 0% canary
/// - If currentStepIndex >= steps.len(): 100% canary, 0% stable (rollout complete)
/// - Otherwise: Use setWeight from steps[currentStepIndex]
pub fn calculate_traffic_weights(rollout: &Rollout) -> (i32, i32) {
    // Get canary strategy
    let canary_strategy = match &rollout.spec.strategy.canary {
        Some(strategy) => strategy,
        None => return (100, 0), // No canary strategy, 100% stable
    };

    // Get current step index from status
    let current_step_index = match &rollout.status {
        Some(status) => status.current_step_index.unwrap_or(-1),
        None => -1, // No status yet, 100% stable
    };

    // If no step is active, default to 100% stable
    if current_step_index < 0 {
        return (100, 0);
    }

    // If step index is beyond available steps, rollout is complete (100% canary)
    if current_step_index as usize >= canary_strategy.steps.len() {
        return (0, 100);
    }

    let canary_weight = canary_strategy.steps[current_step_index as usize]
        .set_weight
        .unwrap_or(0);
    let stable_weight = 100 - canary_weight;

    (stable_weight, canary_weight)
}
