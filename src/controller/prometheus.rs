//! Prometheus metrics integration for automated rollback
//!
//! This module handles querying Prometheus and evaluating metrics against thresholds.

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrometheusError {
    #[error("Prometheus HTTP error: {0}")]
    HttpError(String),

    #[error("Invalid query: {0}")]
    InvalidQuery(String),

    #[error("Failed to parse response: {0}")]
    ParseError(String),

    #[error("No data returned from Prometheus")]
    NoData,
}

/// Build PromQL query for error rate metric
///
/// Calculates: (5xx errors / total requests) * 100
#[allow(dead_code)] // Used in tests, will be used in production metrics analysis
fn build_error_rate_query(rollout_name: &str, revision: &str) -> String {
    format!(
        r#"sum(rate(http_requests_total{{status=~"5..",rollout="{}",revision="{}"}}[2m])) / sum(rate(http_requests_total{{rollout="{}",revision="{}"}}[2m])) * 100"#,
        rollout_name, revision, rollout_name, revision
    )
}

/// Build PromQL query for latency p95 metric
///
/// Uses histogram_quantile to calculate 95th percentile
#[allow(dead_code)] // Used in tests, will be used in production metrics analysis
fn build_latency_p95_query(rollout_name: &str, revision: &str) -> String {
    format!(
        r#"histogram_quantile(0.95, rate(http_request_duration_seconds_bucket{{rollout="{}",revision="{}"}}[2m]))"#,
        rollout_name, revision
    )
}

/// Prometheus instant query response format
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Used in parse_prometheus_instant_query, will be used in production
struct PrometheusResponse {
    status: String,
    data: PrometheusData,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Used in parse_prometheus_instant_query, will be used in production
struct PrometheusData {
    result: Vec<PrometheusResult>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Used in parse_prometheus_instant_query, will be used in production
struct PrometheusResult {
    value: (i64, String), // [timestamp, value_as_string]
}

/// Parse Prometheus instant query response and extract metric value
///
/// Parses the JSON response from Prometheus /api/v1/query endpoint
/// and returns the first metric value as f64.
#[allow(dead_code)] // Used in tests, will be used in production metrics analysis
fn parse_prometheus_instant_query(json_response: &str) -> Result<f64, PrometheusError> {
    let response: PrometheusResponse = serde_json::from_str(json_response)
        .map_err(|e| PrometheusError::ParseError(format!("Invalid JSON: {}", e)))?;

    if response.status != "success" {
        return Err(PrometheusError::HttpError(format!(
            "Prometheus query failed with status: {}",
            response.status
        )));
    }

    let result = response
        .data
        .result
        .first()
        .ok_or(PrometheusError::NoData)?;

    let value = result
        .value
        .1
        .parse::<f64>()
        .map_err(|e| PrometheusError::ParseError(format!("Invalid value: {}", e)))?;

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    // TDD Cycle 2 Part 1: RED - Test building PromQL query from template
    #[test]
    fn test_build_error_rate_query() {
        let rollout_name = "my-app";
        let revision = "canary";

        let query = build_error_rate_query(rollout_name, revision);

        // Should build query that calculates error rate for canary pods
        assert!(query.contains("http_requests_total"));
        assert!(query.contains(r#"status=~"5..""#));
        assert!(query.contains(rollout_name));
        assert!(query.contains(revision));
    }

    #[test]
    fn test_build_latency_p95_query() {
        let rollout_name = "my-app";
        let revision = "stable";

        let query = build_latency_p95_query(rollout_name, revision);

        // Should use histogram_quantile for p95
        assert!(query.contains("histogram_quantile"));
        assert!(query.contains("0.95"));
        assert!(query.contains(rollout_name));
        assert!(query.contains(revision));
    }

    // TDD Cycle 2 Part 2: RED - Test parsing Prometheus instant query response
    #[test]
    fn test_parse_prometheus_response_with_data() {
        // Valid Prometheus instant query response
        let json_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {},
                        "value": [1234567890, "5.2"]
                    }
                ]
            }
        }"#;

        match parse_prometheus_instant_query(json_response) {
            Ok(value) => assert_eq!(value, 5.2),
            Err(e) => panic!("Should parse valid response, got error: {}", e),
        }
    }

    #[test]
    fn test_parse_prometheus_response_no_data() {
        // Empty result (no data points)
        let json_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": []
            }
        }"#;

        let result = parse_prometheus_instant_query(json_response);
        assert!(matches!(result, Err(PrometheusError::NoData)));
    }

    #[test]
    fn test_parse_prometheus_response_invalid_json() {
        let json_response = "not valid json";

        let result = parse_prometheus_instant_query(json_response);
        assert!(matches!(result, Err(PrometheusError::ParseError(_))));
    }
}
