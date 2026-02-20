use super::reconcile::ReconcileError;
use crate::crd::rollout::Rollout;
use k8s_openapi::api::apps::v1::{ReplicaSet, ReplicaSetSpec};
use k8s_openapi::api::core::v1::PodTemplateSpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::api::{Api, ObjectMeta, Patch, PatchParams, PostParams};
use tracing::{debug, error, info};

/// Compute a stable 10-character hash for a PodTemplateSpec
///
/// Inspired by Kubernetes' pod-template-hash label concept, using FNV-1a:
/// - Serialize the template to JSON (deterministic)
/// - Hash the JSON bytes
/// - Return 10-character hex string
///
/// # Errors
/// Returns SerializationError if PodTemplateSpec cannot be serialized to JSON
pub fn compute_pod_template_hash(template: &PodTemplateSpec) -> Result<String, ReconcileError> {
    let json = serde_json::to_string(template)
        .map_err(|e| ReconcileError::SerializationError(e.to_string()))?;

    // FNV-1a (deterministic across processes, unlike DefaultHasher/SipHash)
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in json.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }

    Ok(format!("{hash:x}")[..10].to_string())
}

/// Calculate how to split total replicas between stable and canary
///
/// Given total replicas and canary weight percentage, calculates:
/// - canary_replicas = ceil(total * weight / 100)
/// - stable_replicas = total - canary_replicas
///
/// # Arguments
/// * `total_replicas` - Total number of replicas desired (from rollout.spec.replicas)
/// * `canary_weight` - Percentage of traffic to canary (0-100)
///
/// # Returns
/// Tuple of (stable_replicas, canary_replicas)
///
/// # Examples
/// ```ignore
/// let (stable, canary) = calculate_replica_split(3, 0);
/// assert_eq!(stable, 3); // 0% weight → all stable
/// assert_eq!(canary, 0);
///
/// let (stable, canary) = calculate_replica_split(3, 50);
/// assert_eq!(stable, 1); // 50% of 3 → 1 stable, 2 canary (ceil)
/// assert_eq!(canary, 2);
/// ```
pub fn calculate_replica_split(total_replicas: i32, canary_weight: i32) -> (i32, i32) {
    // Calculate canary replicas (ceiling to ensure at least 1 if weight > 0)
    let canary_replicas = if canary_weight == 0 {
        0
    } else if canary_weight == 100 {
        total_replicas
    } else {
        ((total_replicas as f64 * canary_weight as f64) / 100.0).ceil() as i32
    };

    // Stable gets the remainder
    let stable_replicas = total_replicas - canary_replicas;

    (stable_replicas, canary_replicas)
}

/// Validate surge/unavailable value format
///
/// Returns true if the value is a valid format:
/// - Percentage: "0%" to "100%" (non-negative)
/// - Absolute: non-negative integer
pub(crate) fn is_valid_surge_format(value: &str) -> bool {
    if let Some(percent_str) = value.strip_suffix('%') {
        match percent_str.parse::<i32>() {
            Ok(percent) => (0..=100).contains(&percent),
            Err(_) => false,
        }
    } else {
        match value.parse::<i32>() {
            Ok(abs) => abs >= 0,
            Err(_) => false,
        }
    }
}

/// Parse a surge value (percentage like "25%" or absolute like "5")
///
/// Returns the absolute number of pods based on total_replicas.
/// Invalid values (negative, out of range, or malformed) return 0.
///
/// # Examples
/// ```ignore
/// assert_eq!(parse_surge_value("25%", 10), 3);  // 25% of 10 = 2.5 -> ceil = 3
/// assert_eq!(parse_surge_value("5", 10), 5);   // absolute 5
/// assert_eq!(parse_surge_value("0%", 10), 0);  // 0%
/// assert_eq!(parse_surge_value("-5", 10), 0);  // negative -> 0
/// ```
pub fn parse_surge_value(value: &str, total_replicas: i32) -> i32 {
    if let Some(percent_str) = value.strip_suffix('%') {
        // Percentage value - must be 0-100
        match percent_str.parse::<i32>() {
            Ok(percent) if (0..=100).contains(&percent) => {
                ((total_replicas as f64 * percent as f64) / 100.0).ceil() as i32
            }
            _ => 0, // Invalid percentage (negative, >100, or parse error)
        }
    } else {
        // Absolute value - must be non-negative
        match value.parse::<i32>() {
            Ok(abs) if abs >= 0 => abs,
            _ => 0, // Negative or parse error
        }
    }
}

/// Calculate replica split with maxSurge and maxUnavailable support
///
/// Unlike `calculate_replica_split`, this version can create more total pods
/// than `total_replicas` to enable faster rollouts.
///
/// # Arguments
/// * `total_replicas` - Desired replica count (spec.replicas)
/// * `canary_weight` - Traffic weight for canary (0-100)
/// * `max_surge` - Max extra pods allowed (e.g., "25%" or "5"). None uses default "25%"
/// * `max_unavailable` - Max pods that can be unavailable (e.g., "0" or "25%"). None uses "0"
///
/// # Returns
/// Tuple of (stable_replicas, canary_replicas)
pub fn calculate_replica_split_with_surge(
    total_replicas: i32,
    canary_weight: i32,
    max_surge: Option<&str>,
    max_unavailable: Option<&str>,
) -> (i32, i32) {
    let surge = parse_surge_value(max_surge.unwrap_or("25%"), total_replicas);
    let unavailable = parse_surge_value(max_unavailable.unwrap_or("0"), total_replicas);

    // Calculate ideal canary replicas based on weight
    let ideal_canary = if canary_weight == 0 {
        0
    } else if canary_weight == 100 {
        total_replicas
    } else {
        ((total_replicas as f64 * canary_weight as f64) / 100.0).ceil() as i32
    };

    // With surge, we can keep more replicas during transition
    // Total max = total_replicas + surge
    let max_total = total_replicas + surge;
    let min_total = (total_replicas - unavailable).max(0);

    // Start from the ideal split based on weight
    let mut canary_replicas = ideal_canary;
    let mut stable_replicas = (total_replicas - ideal_canary).max(0);

    // Ensure we don't exceed the maximum allowed replicas
    let mut total = stable_replicas + canary_replicas;
    if total > max_total {
        let mut excess = total - max_total;

        // Prefer scaling down canary replicas first (they're the new deployment)
        let canary_reduction = canary_replicas.min(excess);
        canary_replicas -= canary_reduction;
        excess -= canary_reduction;

        // If still exceeding, scale down stable replicas
        if excess > 0 {
            stable_replicas = (stable_replicas - excess).max(0);
        }
    }

    // Ensure we don't go below the minimum required replicas
    total = stable_replicas + canary_replicas;
    if total < min_total {
        let deficit = min_total - total;
        // Prefer adding stable replicas to satisfy availability constraints
        stable_replicas += deficit;
    }

    (stable_replicas, canary_replicas)
}

/// Ensure a ReplicaSet exists (create if missing)
///
/// This function is idempotent - it will:
/// - Return Ok if ReplicaSet already exists
/// - Create ReplicaSet if it doesn't exist (404)
/// - Return Err on other API errors
pub async fn ensure_replicaset_exists(
    rs_api: &Api<ReplicaSet>,
    rs: &ReplicaSet,
    rs_type: &str,
    replicas: i32,
) -> Result<(), ReconcileError> {
    let rs_name = rs
        .metadata
        .name
        .as_ref()
        .ok_or(ReconcileError::ReplicaSetMissingName)?;

    match rs_api.get(rs_name).await {
        Ok(existing) => {
            // Check if replicas need scaling
            let current_replicas = existing.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);

            if current_replicas != replicas {
                // Replicas need updating - scale the ReplicaSet
                info!(
                    replicaset = ?rs_name,
                    rs_type = rs_type,
                    current = current_replicas,
                    desired = replicas,
                    "Scaling ReplicaSet"
                );

                let scale_patch = serde_json::json!({
                    "spec": {
                        "replicas": replicas
                    }
                });

                rs_api
                    .patch(
                        rs_name,
                        &PatchParams::default(),
                        &Patch::Merge(&scale_patch),
                    )
                    .await?;

                info!(
                    replicaset = ?rs_name,
                    rs_type = rs_type,
                    replicas = replicas,
                    "ReplicaSet scaled successfully"
                );
            } else {
                // Already at correct scale
                debug!(
                    replicaset = ?rs_name,
                    rs_type = rs_type,
                    replicas = replicas,
                    "ReplicaSet already at correct scale"
                );
            }
        }
        Err(kube::Error::Api(err)) if err.code == 404 => {
            // Not found, create it
            info!(
                replicaset = ?rs_name,
                rs_type = rs_type,
                replicas = replicas,
                "Creating ReplicaSet"
            );

            rs_api.create(&PostParams::default(), rs).await?;

            info!(
                replicaset = ?rs_name,
                rs_type = rs_type,
                "ReplicaSet created successfully"
            );
        }
        Err(e) => {
            error!(
                error = ?e,
                replicaset = ?rs_name,
                rs_type = rs_type,
                "Failed to get ReplicaSet"
            );
            return Err(ReconcileError::KubeError(e));
        }
    }

    Ok(())
}

/// Core ReplicaSet builder used by all strategy-specific builders
///
/// Creates a ReplicaSet with:
/// - Labels: pod-template-hash, rollouts.kulta.io/type, rollouts.kulta.io/managed
/// - Name: `{rollout-name}-{rs_type}` if `with_suffix` is true, else `{rollout-name}`
/// - Spec: from Rollout's template
///
/// The `rollouts.kulta.io/managed=true` label prevents Kubernetes Deployment
/// controllers from adopting KULTA-managed ReplicaSets.
fn build_replicaset_core(
    rollout: &Rollout,
    rs_type: &str,
    replicas: i32,
    with_suffix: bool,
) -> Result<ReplicaSet, ReconcileError> {
    let rollout_name = rollout
        .metadata
        .name
        .as_ref()
        .ok_or(ReconcileError::MissingName)?;
    let namespace = rollout.metadata.namespace.clone();

    let pod_template_hash = compute_pod_template_hash(&rollout.spec.template)?;

    let mut template = rollout.spec.template.clone();
    let mut labels = template
        .metadata
        .as_ref()
        .and_then(|m| m.labels.clone())
        .unwrap_or_default();

    labels.insert("pod-template-hash".to_string(), pod_template_hash.clone());
    labels.insert("rollouts.kulta.io/type".to_string(), rs_type.to_string());
    labels.insert("rollouts.kulta.io/managed".to_string(), "true".to_string());

    let mut template_metadata = template.metadata.take().unwrap_or_default();
    template_metadata.labels = Some(labels.clone());
    template.metadata = Some(template_metadata);

    let selector = LabelSelector {
        match_labels: Some(labels.clone()),
        ..Default::default()
    };

    let rs_name = if with_suffix {
        format!("{}-{}", rollout_name, rs_type)
    } else {
        rollout_name.clone()
    };

    Ok(ReplicaSet {
        metadata: ObjectMeta {
            name: Some(rs_name),
            namespace,
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(ReplicaSetSpec {
            replicas: Some(replicas),
            selector,
            template: Some(template),
            ..Default::default()
        }),
        status: None,
    })
}

/// Build a ReplicaSet for canary strategy (stable or canary)
///
/// Name: `{rollout-name}-{rs_type}` (e.g., "my-app-stable", "my-app-canary")
pub fn build_replicaset(
    rollout: &Rollout,
    rs_type: &str,
    replicas: i32,
) -> Result<ReplicaSet, ReconcileError> {
    build_replicaset_core(rollout, rs_type, replicas, true)
}

/// Build a ReplicaSet for simple strategy (no suffix)
///
/// Name: `{rollout-name}` (no type suffix)
pub fn build_replicaset_for_simple(
    rollout: &Rollout,
    replicas: i32,
) -> Result<ReplicaSet, ReconcileError> {
    build_replicaset_core(rollout, "simple", replicas, false)
}

/// Build ReplicaSets for blue-green strategy
///
/// Creates two full-size ReplicaSets:
/// - Active: `{rollout-name}-active` (receives production traffic)
/// - Preview: `{rollout-name}-preview` (for testing before promotion)
pub fn build_replicasets_for_blue_green(
    rollout: &Rollout,
    replicas: i32,
) -> Result<(ReplicaSet, ReplicaSet), ReconcileError> {
    let active_rs = build_replicaset_core(rollout, "active", replicas, true)?;
    let preview_rs = build_replicaset_core(rollout, "preview", replicas, true)?;
    Ok((active_rs, preview_rs))
}

/// Build ReplicaSets for A/B testing strategy
///
/// Creates two full-size ReplicaSets:
/// - Variant A: `{rollout-name}-variant-a` (control group)
/// - Variant B: `{rollout-name}-variant-b` (experiment group)
pub fn build_replicasets_for_ab_testing(
    rollout: &Rollout,
    replicas: i32,
) -> Result<(ReplicaSet, ReplicaSet), ReconcileError> {
    let variant_a_rs = build_replicaset_core(rollout, "variant-a", replicas, true)?;
    let variant_b_rs = build_replicaset_core(rollout, "variant-b", replicas, true)?;
    Ok((variant_a_rs, variant_b_rs))
}
