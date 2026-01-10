//! Health check, metrics, and webhook endpoints for Kubernetes
//!
//! - `/healthz` - Liveness: Is the process alive?
//! - `/readyz` - Readiness: Is the controller ready to handle requests?
//! - `/metrics` - Prometheus metrics in text format
//! - `/convert` - CRD conversion webhook (v1alpha1 <-> v1beta1)

use crate::server::metrics::SharedMetrics;
use axum::{
    extract::State,
    http::{header::CONTENT_TYPE, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;

/// Shared state for readiness tracking
///
/// The controller sets this to ready once it's fully initialized
/// and connected to the Kubernetes API.
#[derive(Debug, Clone)]
pub struct ReadinessState {
    ready: Arc<std::sync::atomic::AtomicBool>,
}

impl ReadinessState {
    /// Create a new readiness state (initially not ready)
    pub fn new() -> Self {
        Self {
            ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Mark the controller as ready
    pub fn set_ready(&self) {
        self.ready.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Mark the controller as not ready (e.g., during shutdown)
    ///
    /// This causes the readiness probe to return 503, signaling to
    /// Kubernetes that the pod should no longer receive traffic.
    pub fn set_not_ready(&self) {
        self.ready.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Check if the controller is ready
    pub fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for ReadinessState {
    fn default() -> Self {
        Self::new()
    }
}

/// Combined server state for health and metrics endpoints
#[derive(Clone)]
pub struct ServerState {
    readiness: ReadinessState,
    metrics: SharedMetrics,
}

impl ServerState {
    /// Create new server state
    pub fn new(readiness: ReadinessState, metrics: SharedMetrics) -> Self {
        Self { readiness, metrics }
    }
}

/// Liveness probe handler
///
/// Always returns 200 OK - if this responds, the process is alive.
async fn healthz() -> StatusCode {
    StatusCode::OK
}

/// Readiness probe handler
///
/// Returns 200 OK if ready, 503 Service Unavailable if not.
async fn readyz(State(state): State<ServerState>) -> StatusCode {
    if state.readiness.is_ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Prometheus metrics handler
///
/// Returns metrics in Prometheus text format for scraping.
async fn metrics(State(state): State<ServerState>) -> impl IntoResponse {
    match state.metrics.encode() {
        Ok(body) => (
            StatusCode::OK,
            [(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to encode metrics: {}", e),
        )
            .into_response(),
    }
}

/// Build the router for health, metrics, and webhook endpoints
fn build_router(readiness: ReadinessState, metrics: SharedMetrics) -> Router {
    let state = ServerState::new(readiness, metrics);

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(self::metrics))
        .route("/convert", post(super::webhook::handle_convert))
        .route("/validate", post(super::webhook::handle_validate))
        .with_state(state)
}

/// Run the health server on the specified port (HTTP, no TLS)
///
/// This function starts an HTTP server that responds to:
/// - GET /healthz - Always returns 200 OK (liveness)
/// - GET /readyz - Returns 200 OK if ready, 503 Service Unavailable if not
/// - GET /metrics - Prometheus metrics in text format
///
/// # Arguments
/// * `port` - The port to listen on
/// * `readiness` - Shared state for readiness tracking
/// * `metrics` - Shared metrics registry for Prometheus
///
/// # Returns
/// This function runs forever until the server is shut down
pub async fn run_health_server(
    port: u16,
    readiness: ReadinessState,
    metrics: SharedMetrics,
) -> Result<(), std::io::Error> {
    let app = build_router(readiness, metrics);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    // Log after successful bind - server is actually listening
    info!(port = %port, "Health and metrics server listening (HTTP)");

    axum::serve(listener, app)
        .await
        .map_err(std::io::Error::other)
}

/// Run the health server with TLS (HTTPS)
///
/// This function starts an HTTPS server for secure webhook communication.
/// Used when the conversion webhook is enabled.
///
/// # Arguments
/// * `port` - The port to listen on (typically 8443 for HTTPS)
/// * `readiness` - Shared state for readiness tracking
/// * `metrics` - Shared metrics registry for Prometheus
/// * `tls_config` - rustls ServerConfig for TLS
///
/// # Returns
/// This function runs forever until the server is shut down
pub async fn run_health_server_tls(
    port: u16,
    readiness: ReadinessState,
    metrics: SharedMetrics,
    tls_config: std::sync::Arc<rustls::ServerConfig>,
) -> Result<(), std::io::Error> {
    use axum_server::tls_rustls::RustlsConfig;

    let app = build_router(readiness, metrics);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    // Convert Arc<ServerConfig> to RustlsConfig
    let config = RustlsConfig::from_config(tls_config);

    info!(port = %port, "Health, metrics, and webhook server listening (HTTPS)");

    axum_server::bind_rustls(addr, config)
        .serve(app.into_make_service())
        .await
}
