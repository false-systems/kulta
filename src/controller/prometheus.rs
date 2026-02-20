//! Prometheus metrics integration for automated rollback
//!
//! This module handles querying Prometheus and evaluating metrics against thresholds.

use async_trait::async_trait;
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

    #[error("Invalid metric value: {0}")]
    InvalidValue(String),
}

/// Trait for querying Prometheus metrics
///
/// Production code uses `HttpPrometheusClient` which queries a real Prometheus server.
/// Tests use `MockPrometheusClient` which returns preconfigured responses.
#[async_trait]
pub trait MetricsQuerier: Send + Sync {
    /// Execute instant query against Prometheus
    async fn query_instant(&self, query: &str) -> Result<f64, PrometheusError>;

    /// Downcast support for testing (allows accessing mock-specific methods)
    fn as_any(&self) -> &dyn std::any::Any;

    /// Evaluate a metric by name against threshold
    async fn evaluate_metric(
        &self,
        metric_name: &str,
        rollout_name: &str,
        revision: &str,
        threshold: f64,
    ) -> Result<bool, PrometheusError> {
        let query = match metric_name {
            "error-rate" => build_error_rate_query(rollout_name, revision),
            "latency-p95" => build_latency_p95_query(rollout_name, revision),
            _ => {
                return Err(PrometheusError::InvalidQuery(format!(
                    "Unknown metric template: {}",
                    metric_name
                )))
            }
        };
        let value = self.query_instant(&query).await?;
        Ok(value < threshold)
    }

    /// Evaluate all metrics from analysis config
    async fn evaluate_all_metrics(
        &self,
        metrics: &[crate::crd::rollout::MetricConfig],
        rollout_name: &str,
        revision: &str,
    ) -> Result<bool, PrometheusError> {
        if metrics.is_empty() {
            return Ok(true);
        }
        for metric in metrics {
            let is_healthy = self
                .evaluate_metric(&metric.name, rollout_name, revision, metric.threshold)
                .await?;
            if !is_healthy {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Query A/B variant error rate
    async fn query_ab_error_rate(&self, service_name: &str) -> Result<f64, PrometheusError> {
        let query = build_ab_error_rate_query(service_name);
        self.query_instant(&query).await
    }

    /// Query A/B variant sample count
    async fn query_ab_sample_count(&self, service_name: &str) -> Result<i64, PrometheusError> {
        let query = build_ab_sample_count_query(service_name);
        let count = self.query_instant(&query).await?;
        Ok(count as i64)
    }
}

/// Build PromQL query for error rate metric
///
/// Calculates: (5xx errors / total requests) * 100
fn build_error_rate_query(rollout_name: &str, revision: &str) -> String {
    format!(
        r#"sum(rate(http_requests_total{{status=~"5..",rollout="{}",revision="{}"}}[2m])) / sum(rate(http_requests_total{{rollout="{}",revision="{}"}}[2m])) * 100"#,
        rollout_name, revision, rollout_name, revision
    )
}

/// Build PromQL query for A/B variant error rate
///
/// Queries by service name (variant_a_service or variant_b_service)
pub fn build_ab_error_rate_query(service_name: &str) -> String {
    format!(
        r#"sum(rate(http_requests_total{{status=~"5..",service="{}"}}[5m])) / sum(rate(http_requests_total{{service="{}"}}[5m]))"#,
        service_name, service_name
    )
}

/// Build PromQL query for A/B variant sample count
///
/// Returns total request count for a service
pub fn build_ab_sample_count_query(service_name: &str) -> String {
    format!(
        r#"sum(increase(http_requests_total{{service="{}"}}[1h]))"#,
        service_name
    )
}

/// Build PromQL query for latency p95 metric
///
/// Uses histogram_quantile to calculate 95th percentile
fn build_latency_p95_query(rollout_name: &str, revision: &str) -> String {
    format!(
        r#"histogram_quantile(0.95, rate(http_request_duration_seconds_bucket{{rollout="{}",revision="{}"}}[2m]))"#,
        rollout_name, revision
    )
}

/// Prometheus instant query response format
#[derive(Debug, Deserialize)]
struct PrometheusResponse {
    status: String,
    data: PrometheusData,
}

#[derive(Debug, Deserialize)]
struct PrometheusData {
    result: Vec<PrometheusResult>,
}

#[derive(Debug, Deserialize)]
struct PrometheusResult {
    value: (i64, String), // [timestamp, value_as_string]
}

/// Parse Prometheus instant query response and extract metric value
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

    // Reject NaN and infinity values
    if value.is_nan() {
        return Err(PrometheusError::InvalidValue("NaN".to_string()));
    }
    if value.is_infinite() {
        return Err(PrometheusError::InvalidValue("infinity".to_string()));
    }

    Ok(value)
}

/// Production Prometheus client that queries a real server
#[derive(Clone)]
pub struct HttpPrometheusClient {
    address: String,
}

impl HttpPrometheusClient {
    pub fn new(address: String) -> Self {
        Self { address }
    }
}

#[async_trait]
impl MetricsQuerier for HttpPrometheusClient {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn query_instant(&self, query: &str) -> Result<f64, PrometheusError> {
        let url = format!("{}/api/v1/query", self.address);
        let client = reqwest::Client::new();

        let response = client
            .get(&url)
            .query(&[("query", query)])
            .send()
            .await
            .map_err(|e| PrometheusError::HttpError(format!("HTTP request failed: {}", e)))?;

        let body = response
            .text()
            .await
            .map_err(|e| PrometheusError::HttpError(format!("Failed to read response: {}", e)))?;

        parse_prometheus_instant_query(&body)
    }
}

/// Mock Prometheus client for testing
///
/// Supports two modes:
/// - Single response: `set_mock_response()` sets one response returned for all queries
/// - Response queue: `enqueue_response()` / `enqueue_error()` for sequential multi-query tests
#[cfg(test)]
#[derive(Clone)]
pub struct MockPrometheusClient {
    mock_response: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    response_queue: std::sync::Arc<std::sync::Mutex<Vec<Result<f64, PrometheusError>>>>,
}

#[cfg(test)]
impl Default for MockPrometheusClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl MockPrometheusClient {
    pub fn new() -> Self {
        Self {
            mock_response: std::sync::Arc::new(std::sync::Mutex::new(None)),
            response_queue: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn set_mock_response(&self, response: String) {
        if let Ok(mut mock) = self.mock_response.lock() {
            *mock = Some(response);
        }
    }

    /// Enqueue a successful value to be returned by the next `query_instant` call
    pub fn enqueue_response(&self, value: f64) {
        if let Ok(mut queue) = self.response_queue.lock() {
            queue.push(Ok(value));
        }
    }

    /// Enqueue an error to be returned by the next `query_instant` call
    pub fn enqueue_error(&self, error: PrometheusError) {
        if let Ok(mut queue) = self.response_queue.lock() {
            queue.push(Err(error));
        }
    }
}

#[cfg(test)]
#[async_trait]
impl MetricsQuerier for MockPrometheusClient {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn query_instant(&self, _query: &str) -> Result<f64, PrometheusError> {
        // If queue has entries, use FIFO order
        if let Ok(mut queue) = self.response_queue.lock() {
            if !queue.is_empty() {
                return queue.remove(0);
            }
        }
        // Fall back to single mock response
        let mock = self
            .mock_response
            .lock()
            .map_err(|_| PrometheusError::HttpError("Lock poisoned".to_string()))?;
        let response = mock
            .as_ref()
            .ok_or_else(|| PrometheusError::HttpError("No mock response set".to_string()))?;
        parse_prometheus_instant_query(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_error_rate_query() {
        let rollout_name = "my-app";
        let revision = "canary";

        let query = build_error_rate_query(rollout_name, revision);

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

        assert!(query.contains("histogram_quantile"));
        assert!(query.contains("0.95"));
        assert!(query.contains(rollout_name));
        assert!(query.contains(revision));
    }

    #[test]
    fn test_parse_prometheus_response_with_data() {
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

    #[tokio::test]
    async fn test_prometheus_client_query_instant() {
        let client = MockPrometheusClient::new();

        let mock_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {},
                        "value": [1234567890, "12.5"]
                    }
                ]
            }
        }"#;
        client.set_mock_response(mock_response.to_string());

        let query = "rate(http_requests_total[2m])";
        let result = client.query_instant(query).await;

        match result {
            Ok(value) => assert_eq!(value, 12.5),
            Err(e) => panic!("Should successfully query, got error: {}", e),
        }
    }

    #[tokio::test]
    async fn test_prometheus_client_query_no_data() {
        let client = MockPrometheusClient::new();

        let mock_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": []
            }
        }"#;
        client.set_mock_response(mock_response.to_string());

        let query = "rate(http_requests_total[2m])";
        let result = client.query_instant(query).await;

        assert!(matches!(result, Err(PrometheusError::NoData)));
    }

    #[tokio::test]
    async fn test_evaluate_error_rate_healthy() {
        let client = MockPrometheusClient::new();

        let mock_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {},
                        "value": [1234567890, "2.5"]
                    }
                ]
            }
        }"#;
        client.set_mock_response(mock_response.to_string());

        let rollout_name = "my-app";
        let revision = "canary";
        let threshold = 5.0;

        let result = client
            .evaluate_metric("error-rate", rollout_name, revision, threshold)
            .await;

        match result {
            Ok(is_healthy) => assert!(is_healthy, "Error rate 2.5% should be healthy (< 5.0%)"),
            Err(e) => panic!("Should evaluate successfully, got error: {}", e),
        }
    }

    #[tokio::test]
    async fn test_evaluate_error_rate_unhealthy() {
        let client = MockPrometheusClient::new();

        let mock_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {},
                        "value": [1234567890, "8.0"]
                    }
                ]
            }
        }"#;
        client.set_mock_response(mock_response.to_string());

        let rollout_name = "my-app";
        let revision = "canary";
        let threshold = 5.0;

        let result = client
            .evaluate_metric("error-rate", rollout_name, revision, threshold)
            .await;

        match result {
            Ok(is_healthy) => assert!(!is_healthy, "Error rate 8.0% should be unhealthy (> 5.0%)"),
            Err(e) => panic!("Should evaluate successfully, got error: {}", e),
        }
    }

    #[tokio::test]
    async fn test_evaluate_all_metrics_all_healthy() {
        use crate::crd::rollout::MetricConfig;

        let client = MockPrometheusClient::new();

        let mock_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {},
                        "value": [1234567890, "2.5"]
                    }
                ]
            }
        }"#;
        client.set_mock_response(mock_response.to_string());

        let metrics = vec![
            MetricConfig {
                name: "error-rate".to_string(),
                threshold: 5.0,
                interval: None,
                failure_threshold: None,
                min_sample_size: None,
            },
            MetricConfig {
                name: "latency-p95".to_string(),
                threshold: 100.0,
                interval: None,
                failure_threshold: None,
                min_sample_size: None,
            },
        ];

        let rollout_name = "my-app";
        let revision = "canary";

        let result = client
            .evaluate_all_metrics(&metrics, rollout_name, revision)
            .await;

        match result {
            Ok(is_healthy) => assert!(is_healthy, "All metrics should be healthy"),
            Err(e) => panic!("Should evaluate successfully, got error: {}", e),
        }
    }

    #[tokio::test]
    async fn test_evaluate_all_metrics_one_unhealthy() {
        use crate::crd::rollout::MetricConfig;

        let client = MockPrometheusClient::new();

        let mock_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {},
                        "value": [1234567890, "8.0"]
                    }
                ]
            }
        }"#;
        client.set_mock_response(mock_response.to_string());

        let metrics = vec![MetricConfig {
            name: "error-rate".to_string(),
            threshold: 5.0,
            interval: None,
            failure_threshold: None,
            min_sample_size: None,
        }];

        let rollout_name = "my-app";
        let revision = "canary";

        let result = client
            .evaluate_all_metrics(&metrics, rollout_name, revision)
            .await;

        match result {
            Ok(is_healthy) => assert!(
                !is_healthy,
                "Should be unhealthy when error-rate exceeds threshold"
            ),
            Err(e) => panic!("Should evaluate successfully, got error: {}", e),
        }
    }

    #[tokio::test]
    async fn test_evaluate_all_metrics_empty_list() {
        let client = MockPrometheusClient::new();

        let metrics = vec![];
        let rollout_name = "my-app";
        let revision = "canary";

        let result = client
            .evaluate_all_metrics(&metrics, rollout_name, revision)
            .await;

        match result {
            Ok(is_healthy) => assert!(is_healthy, "Empty metrics list should be healthy"),
            Err(e) => panic!("Should evaluate successfully, got error: {}", e),
        }
    }

    #[tokio::test]
    async fn test_evaluate_metric_at_exactly_threshold_is_unhealthy() {
        let client = MockPrometheusClient::new();

        let mock_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {},
                        "value": [1234567890, "5.0"]
                    }
                ]
            }
        }"#;
        client.set_mock_response(mock_response.to_string());

        let rollout_name = "my-app";
        let revision = "canary";
        let threshold = 5.0;

        let result = client
            .evaluate_metric("error-rate", rollout_name, revision, threshold)
            .await;

        match result {
            Ok(is_healthy) => assert!(
                !is_healthy,
                "Error rate exactly at threshold (5.0%) should be unhealthy"
            ),
            Err(e) => panic!("Should evaluate successfully, got error: {}", e),
        }
    }

    #[test]
    fn test_parse_prometheus_response_nan_returns_error() {
        let json_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {},
                        "value": [1234567890, "NaN"]
                    }
                ]
            }
        }"#;

        let result = parse_prometheus_instant_query(json_response);
        assert!(
            matches!(result, Err(PrometheusError::InvalidValue(_))),
            "NaN value should return InvalidValue error"
        );
    }

    #[test]
    fn test_parse_prometheus_response_infinity_returns_error() {
        let json_response = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {},
                        "value": [1234567890, "+Inf"]
                    }
                ]
            }
        }"#;

        let result = parse_prometheus_instant_query(json_response);
        assert!(
            matches!(result, Err(PrometheusError::InvalidValue(_))),
            "+Inf value should return InvalidValue error"
        );
    }
}
