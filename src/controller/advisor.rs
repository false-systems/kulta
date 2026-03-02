//! AI advisor integration for progressive delivery decisions
//!
//! Follows the same trait-based pattern as `MetricsQuerier` (prometheus.rs):
//! - `AnalysisAdvisor` trait for abstraction
//! - `NoOpAdvisor` for Level 0/1 (no advisory calls)
//! - `HttpAdvisor` for Level 2+ (calls external AI service)
//! - `MockAdvisor` for testing
//!
//! The advisor never overrides threshold decisions at Level 2 — it only
//! provides recommendations that are logged alongside the threshold result.

use crate::crd::rollout::{Recommendation, RecommendedAction};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdvisorError {
    #[error("Advisory service unreachable: {0}")]
    Unreachable(String),

    #[error("Advisory service returned invalid response: {0}")]
    InvalidResponse(String),

    #[error("Advisory call timed out after {0:?}")]
    Timeout(Duration),
}

/// Everything the advisor needs to make a recommendation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisContext {
    pub rollout_name: String,
    pub namespace: String,
    pub strategy: String,
    pub current_step: Option<i32>,
    pub current_weight: Option<i32>,
    pub metrics_healthy: bool,
    pub phase: String,
    pub history: Vec<String>,
}

/// Trait for AI advisory integration
///
/// Production code uses `HttpAdvisor` which calls an external AI service.
/// Tests use `MockAdvisor` which returns preconfigured responses.
/// Default is `NoOpAdvisor` which returns Continue with zero confidence.
#[async_trait]
pub trait AnalysisAdvisor: Send + Sync {
    /// Request a recommendation from the advisor
    async fn advise(&self, context: &AnalysisContext) -> Result<Recommendation, AdvisorError>;

    /// Downcast support for testing
    fn as_any(&self) -> &dyn std::any::Any;
}

/// No-op advisor for Level 0/1 (default)
///
/// Returns Continue with zero confidence — the threshold decision is used as-is.
pub struct NoOpAdvisor;

#[async_trait]
impl AnalysisAdvisor for NoOpAdvisor {
    async fn advise(&self, _ctx: &AnalysisContext) -> Result<Recommendation, AdvisorError> {
        Ok(Recommendation {
            action: RecommendedAction::Continue,
            confidence: 0.0,
            reasoning: "no advisor configured".into(),
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// HTTP-based advisor for Level 2+ (production)
///
/// Calls an external AI advisory service with rollout context
/// and returns the recommendation. Times out gracefully.
pub struct HttpAdvisor {
    client: reqwest::Client,
    endpoint: String,
    timeout: Duration,
}

impl HttpAdvisor {
    pub fn new(endpoint: String, timeout: Duration) -> Self {
        let client = match reqwest::Client::builder().timeout(timeout).build() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to build advisor HTTP client, using default");
                reqwest::Client::new()
            }
        };
        Self {
            client,
            endpoint,
            timeout,
        }
    }
}

#[async_trait]
impl AnalysisAdvisor for HttpAdvisor {
    async fn advise(&self, context: &AnalysisContext) -> Result<Recommendation, AdvisorError> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(context)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    AdvisorError::Timeout(self.timeout)
                } else {
                    AdvisorError::Unreachable(e.to_string())
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AdvisorError::InvalidResponse(format!(
                "HTTP {}: {}",
                status,
                body.chars().take(200).collect::<String>()
            )));
        }

        let recommendation: Recommendation = response
            .json()
            .await
            .map_err(|e| AdvisorError::InvalidResponse(e.to_string()))?;

        Ok(recommendation)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Cache for HttpAdvisor instances, keyed by (endpoint, timeout_seconds).
///
/// Prevents constructing a new reqwest::Client on every reconcile call.
/// Thread-safe via Mutex — lock is held only briefly during lookup/insert.
#[derive(Default)]
pub struct AdvisorCache {
    cache: Mutex<HashMap<(String, u64), Arc<dyn AnalysisAdvisor>>>,
}

impl AdvisorCache {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }
}

/// Resolve the appropriate advisor for a Rollout's config
///
/// - Level Off/Context → NoOpAdvisor (no external calls)
/// - Level Advised/Planned/Driven with endpoint → cached HttpAdvisor
/// - Level Advised/Planned/Driven without endpoint → NoOpAdvisor (misconfigured, logged)
///
/// If `ctx.advisor` is not a NoOpAdvisor (e.g., MockAdvisor in tests),
/// it is returned as-is — test overrides always win.
///
/// HttpAdvisor instances are cached by (endpoint, timeout) to reuse
/// reqwest::Client connections across reconcile calls.
pub fn resolve_advisor(
    config: &crate::crd::rollout::AdvisorConfig,
    ctx_advisor: &Arc<dyn AnalysisAdvisor>,
    advisor_cache: &AdvisorCache,
) -> Arc<dyn AnalysisAdvisor> {
    use crate::crd::rollout::AdvisorLevel;

    // If the Context has a non-NoOp advisor (test mock), use it directly
    if !ctx_advisor.as_any().is::<NoOpAdvisor>() {
        return ctx_advisor.clone();
    }

    match config.level {
        AdvisorLevel::Off | AdvisorLevel::Context => Arc::new(NoOpAdvisor),
        AdvisorLevel::Advised | AdvisorLevel::Planned | AdvisorLevel::Driven => {
            match &config.endpoint {
                Some(endpoint) => {
                    let key = (endpoint.clone(), config.timeout_seconds);
                    if let Ok(cache) = advisor_cache.cache.lock() {
                        if let Some(advisor) = cache.get(&key) {
                            return advisor.clone();
                        }
                    }
                    let timeout = Duration::from_secs(config.timeout_seconds);
                    let advisor: Arc<dyn AnalysisAdvisor> =
                        Arc::new(HttpAdvisor::new(endpoint.clone(), timeout));
                    if let Ok(mut cache) = advisor_cache.cache.lock() {
                        cache.insert(key, advisor.clone());
                    }
                    advisor
                }
                None => {
                    tracing::warn!(
                        level = ?config.level,
                        "Advisor level requires endpoint but none configured, falling back to no-op"
                    );
                    Arc::new(NoOpAdvisor)
                }
            }
        }
    }
}

/// Mock advisor for testing
///
/// Returns a preconfigured recommendation. Thread-safe via Arc<Mutex<>>.
#[cfg(test)]
pub struct MockAdvisor {
    pub response: std::sync::Arc<std::sync::Mutex<Result<Recommendation, String>>>,
    pub call_count: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

#[cfg(test)]
impl MockAdvisor {
    pub fn new(recommendation: Recommendation) -> Self {
        Self {
            response: std::sync::Arc::new(std::sync::Mutex::new(Ok(recommendation))),
            call_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    pub fn new_failing(error_msg: &str) -> Self {
        Self {
            response: std::sync::Arc::new(std::sync::Mutex::new(Err(error_msg.to_string()))),
            call_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    pub fn calls(&self) -> u32 {
        self.call_count.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
#[async_trait]
impl AnalysisAdvisor for MockAdvisor {
    async fn advise(&self, _ctx: &AnalysisContext) -> Result<Recommendation, AdvisorError> {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let guard = self
            .response
            .lock()
            .map_err(|_| AdvisorError::Unreachable("lock poisoned".into()))?;

        match &*guard {
            Ok(rec) => Ok(rec.clone()),
            Err(msg) => Err(AdvisorError::Unreachable(msg.clone())),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_advisor_returns_continue() {
        let advisor = NoOpAdvisor;
        let ctx = AnalysisContext {
            rollout_name: "my-app".into(),
            namespace: "default".into(),
            strategy: "canary".into(),
            current_step: Some(1),
            current_weight: Some(20),
            metrics_healthy: true,
            phase: "Progressing".into(),
            history: vec![],
        };

        let rec = advisor.advise(&ctx).await.unwrap();
        assert_eq!(rec.action, RecommendedAction::Continue);
        assert_eq!(rec.confidence, 0.0);
    }

    #[tokio::test]
    async fn test_mock_advisor_returns_configured_response() {
        let advisor = MockAdvisor::new(Recommendation {
            action: RecommendedAction::Rollback,
            confidence: 0.95,
            reasoning: "high error rate detected".into(),
        });

        let ctx = AnalysisContext {
            rollout_name: "my-app".into(),
            namespace: "default".into(),
            strategy: "canary".into(),
            current_step: Some(2),
            current_weight: Some(40),
            metrics_healthy: false,
            phase: "Progressing".into(),
            history: vec![],
        };

        let rec = advisor.advise(&ctx).await.unwrap();
        assert_eq!(rec.action, RecommendedAction::Rollback);
        assert_eq!(rec.confidence, 0.95);
        assert_eq!(advisor.calls(), 1);
    }

    #[tokio::test]
    async fn test_mock_advisor_tracks_call_count() {
        let advisor = MockAdvisor::new(Recommendation {
            action: RecommendedAction::Continue,
            confidence: 0.8,
            reasoning: "looks good".into(),
        });

        let ctx = AnalysisContext {
            rollout_name: "test".into(),
            namespace: "default".into(),
            strategy: "canary".into(),
            current_step: None,
            current_weight: None,
            metrics_healthy: true,
            phase: "Progressing".into(),
            history: vec![],
        };

        let _ = advisor.advise(&ctx).await;
        let _ = advisor.advise(&ctx).await;
        let _ = advisor.advise(&ctx).await;
        assert_eq!(advisor.calls(), 3);
    }

    #[tokio::test]
    async fn test_mock_advisor_error() {
        let advisor = MockAdvisor::new_failing("connection refused");

        let ctx = AnalysisContext {
            rollout_name: "test".into(),
            namespace: "default".into(),
            strategy: "canary".into(),
            current_step: None,
            current_weight: None,
            metrics_healthy: true,
            phase: "Progressing".into(),
            history: vec![],
        };

        let result = advisor.advise(&ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("connection refused"));
    }

    #[test]
    fn test_resolve_advisor_off_returns_noop() {
        use crate::crd::rollout::{AdvisorConfig, AdvisorLevel};

        let config = AdvisorConfig {
            level: AdvisorLevel::Off,
            endpoint: Some("http://ai:8080".into()),
            timeout_seconds: 10,
        };
        let ctx_advisor: std::sync::Arc<dyn AnalysisAdvisor> = std::sync::Arc::new(NoOpAdvisor);

        let resolved = resolve_advisor(&config, &ctx_advisor, &AdvisorCache::new());
        assert!(resolved.as_any().is::<NoOpAdvisor>());
    }

    #[test]
    fn test_resolve_advisor_context_returns_noop() {
        use crate::crd::rollout::{AdvisorConfig, AdvisorLevel};

        let config = AdvisorConfig {
            level: AdvisorLevel::Context,
            endpoint: Some("http://ai:8080".into()),
            timeout_seconds: 10,
        };
        let ctx_advisor: std::sync::Arc<dyn AnalysisAdvisor> = std::sync::Arc::new(NoOpAdvisor);

        let resolved = resolve_advisor(&config, &ctx_advisor, &AdvisorCache::new());
        assert!(resolved.as_any().is::<NoOpAdvisor>());
    }

    #[test]
    fn test_resolve_advisor_advised_with_endpoint_returns_http() {
        use crate::crd::rollout::{AdvisorConfig, AdvisorLevel};

        let config = AdvisorConfig {
            level: AdvisorLevel::Advised,
            endpoint: Some("http://ai-advisor:8080/advise".into()),
            timeout_seconds: 5,
        };
        let ctx_advisor: std::sync::Arc<dyn AnalysisAdvisor> = std::sync::Arc::new(NoOpAdvisor);

        let resolved = resolve_advisor(&config, &ctx_advisor, &AdvisorCache::new());
        assert!(resolved.as_any().is::<HttpAdvisor>());
    }

    #[test]
    fn test_resolve_advisor_advised_without_endpoint_returns_noop() {
        use crate::crd::rollout::{AdvisorConfig, AdvisorLevel};

        let config = AdvisorConfig {
            level: AdvisorLevel::Advised,
            endpoint: None,
            timeout_seconds: 10,
        };
        let ctx_advisor: std::sync::Arc<dyn AnalysisAdvisor> = std::sync::Arc::new(NoOpAdvisor);

        let resolved = resolve_advisor(&config, &ctx_advisor, &AdvisorCache::new());
        // Falls back to NoOp when endpoint is missing
        assert!(resolved.as_any().is::<NoOpAdvisor>());
    }

    #[test]
    fn test_resolve_advisor_mock_override_wins() {
        use crate::crd::rollout::{AdvisorConfig, AdvisorLevel};

        let config = AdvisorConfig {
            level: AdvisorLevel::Advised,
            endpoint: Some("http://ai:8080".into()),
            timeout_seconds: 10,
        };
        // Context has a MockAdvisor — test override should win
        let mock = MockAdvisor::new(Recommendation {
            action: RecommendedAction::Rollback,
            confidence: 1.0,
            reasoning: "test".into(),
        });
        let ctx_advisor: std::sync::Arc<dyn AnalysisAdvisor> = std::sync::Arc::new(mock);

        let resolved = resolve_advisor(&config, &ctx_advisor, &AdvisorCache::new());
        // MockAdvisor should be returned, not HttpAdvisor
        assert!(resolved.as_any().is::<MockAdvisor>());
    }
}
