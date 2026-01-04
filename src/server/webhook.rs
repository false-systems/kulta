//! CRD Conversion Webhook for Rollout resources
//!
//! Handles conversion between v1alpha1 and v1beta1 versions of the Rollout CRD.
//! Kubernetes calls this webhook when it needs to convert between versions.
//!
//! ## Endpoints
//! - POST /convert - Kubernetes ConversionReview webhook
//!
//! ## Conversion Rules
//! - v1alpha1 -> v1beta1: Add defaults for maxSurge, maxUnavailable, progressDeadlineSeconds
//! - v1beta1 -> v1alpha1: Drop new fields

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::crd::conversion::{
    DEFAULT_MAX_SURGE, DEFAULT_MAX_UNAVAILABLE, DEFAULT_PROGRESS_DEADLINE_SECONDS,
};

/// Kubernetes ConversionReview request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionReview {
    pub api_version: String,
    pub kind: String,
    pub request: ConversionRequest,
}

/// The actual conversion request from Kubernetes
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionRequest {
    /// Unique ID for this request
    pub uid: String,
    /// Target API version (e.g., "kulta.io/v1beta1")
    pub desired_api_version: String,
    /// Objects to convert
    pub objects: Vec<Value>,
}

/// Result status for conversion
#[derive(Debug, Serialize, PartialEq)]
pub struct ConversionResult {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Response for a conversion request
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionResponse {
    pub uid: String,
    pub result: ConversionResult,
    pub converted_objects: Vec<Value>,
}

/// Full ConversionReview response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionReviewResponse {
    pub api_version: String,
    pub kind: String,
    pub response: ConversionResponse,
}

/// Extract version from apiVersion string (e.g., "kulta.io/v1beta1" -> "v1beta1")
fn extract_version(api_version: &str) -> Option<&str> {
    api_version.split('/').next_back()
}

/// Build a short context string (namespace/name) for error messages
fn object_context(obj: &Value) -> String {
    let metadata = obj.get("metadata");
    let name = metadata
        .and_then(|m| m.get("name"))
        .and_then(|n| n.as_str());
    let namespace = metadata
        .and_then(|m| m.get("namespace"))
        .and_then(|n| n.as_str());
    match (namespace, name) {
        (Some(ns), Some(n)) => format!(" (namespace: {}, name: {})", ns, n),
        (None, Some(n)) => format!(" (name: {})", n),
        (Some(ns), None) => format!(" (namespace: {})", ns),
        _ => String::new(),
    }
}

/// Convert a single Rollout object to the desired version
fn convert_object(obj: &Value, desired_version: &str) -> Result<Value, String> {
    let current_api_version = obj
        .get("apiVersion")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("Missing apiVersion{}", object_context(obj)))?;

    let current_version = extract_version(current_api_version).ok_or_else(|| {
        format!(
            "Invalid apiVersion format '{}'{}",
            current_api_version,
            object_context(obj)
        )
    })?;

    // Same version - no conversion needed
    if current_version == desired_version {
        return Ok(obj.clone());
    }

    // Start with a clone
    let mut converted = obj.clone();

    // Update apiVersion
    converted["apiVersion"] = json!(format!("kulta.io/{}", desired_version));

    match (current_version, desired_version) {
        ("v1alpha1", "v1beta1") => {
            // Add new fields with defaults
            if let Some(spec) = converted.get_mut("spec") {
                if spec.get("maxSurge").is_none() {
                    spec["maxSurge"] = json!(DEFAULT_MAX_SURGE);
                }
                if spec.get("maxUnavailable").is_none() {
                    spec["maxUnavailable"] = json!(DEFAULT_MAX_UNAVAILABLE);
                }
                if spec.get("progressDeadlineSeconds").is_none() {
                    spec["progressDeadlineSeconds"] = json!(DEFAULT_PROGRESS_DEADLINE_SECONDS);
                }
            }
            Ok(converted)
        }
        ("v1beta1", "v1alpha1") => {
            // Remove new fields
            if let Some(spec) = converted.get_mut("spec") {
                if let Some(spec_obj) = spec.as_object_mut() {
                    spec_obj.remove("maxSurge");
                    spec_obj.remove("maxUnavailable");
                    spec_obj.remove("progressDeadlineSeconds");
                }
            }
            Ok(converted)
        }
        _ => Err(format!(
            "Unsupported conversion: {} -> {}",
            current_version, desired_version
        )),
    }
}

/// Convert all objects in a request
pub fn convert_rollout(request: ConversionRequest) -> ConversionResponse {
    let desired_version = match extract_version(&request.desired_api_version) {
        Some(v) => v,
        None => {
            return ConversionResponse {
                uid: request.uid,
                result: ConversionResult {
                    status: "Failed".to_string(),
                    message: Some(format!(
                        "Invalid desired API version: {}",
                        request.desired_api_version
                    )),
                },
                converted_objects: vec![],
            };
        }
    };

    // Check if desired version is supported
    if desired_version != "v1alpha1" && desired_version != "v1beta1" {
        return ConversionResponse {
            uid: request.uid,
            result: ConversionResult {
                status: "Failed".to_string(),
                message: Some(format!("Unsupported API version: {}", desired_version)),
            },
            converted_objects: vec![],
        };
    }

    let mut converted_objects = Vec::with_capacity(request.objects.len());

    for obj in &request.objects {
        match convert_object(obj, desired_version) {
            Ok(converted) => converted_objects.push(converted),
            Err(e) => {
                return ConversionResponse {
                    uid: request.uid,
                    result: ConversionResult {
                        status: "Failed".to_string(),
                        message: Some(e),
                    },
                    converted_objects: vec![],
                };
            }
        }
    }

    ConversionResponse {
        uid: request.uid,
        result: ConversionResult {
            status: "Success".to_string(),
            message: None,
        },
        converted_objects,
    }
}

/// Axum handler for the /convert endpoint
pub async fn handle_convert(Json(review): Json<ConversionReview>) -> impl IntoResponse {
    info!(
        uid = %review.request.uid,
        desired_version = %review.request.desired_api_version,
        object_count = review.request.objects.len(),
        "Processing conversion request"
    );

    let response = convert_rollout(review.request);

    if response.result.status == "Failed" {
        warn!(
            uid = %response.uid,
            error = ?response.result.message,
            "Conversion failed"
        );
    } else {
        info!(
            uid = %response.uid,
            converted_count = response.converted_objects.len(),
            "Conversion successful"
        );
    }

    let review_response = ConversionReviewResponse {
        api_version: "apiextensions.k8s.io/v1".to_string(),
        kind: "ConversionReview".to_string(),
        response,
    };

    (StatusCode::OK, Json(review_response))
}

#[cfg(test)]
#[path = "webhook_test.rs"]
mod tests;
