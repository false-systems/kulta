use futures::StreamExt;
use kube::runtime::controller::Action;
use kube::runtime::{watcher, Controller};
use kube::{Api, Client};
use kulta::controller::cdevents::CDEventsSink;
use kulta::controller::prometheus::PrometheusClient;
use kulta::controller::{reconcile, Context, ReconcileError};
use kulta::crd::rollout::Rollout;
use kulta::server::{
    build_rustls_config, create_metrics, initialize_tls, run_health_server, run_health_server_tls,
    run_leader_election, shutdown_channel, wait_for_signal, LeaderConfig, LeaderState,
    ReadinessState, DEFAULT_TLS_SECRET_NAME,
};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

/// Default port for health endpoints (HTTP)
const HEALTH_PORT: u16 = 8080;

/// Default port for webhook endpoints (HTTPS)
const WEBHOOK_PORT: u16 = 8443;

/// Check if leader election is enabled via env var
fn is_leader_election_enabled() -> bool {
    std::env::var("KULTA_LEADER_ELECTION")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

/// Check if webhook TLS is enabled via env var
fn is_webhook_tls_enabled() -> bool {
    std::env::var("KULTA_WEBHOOK_TLS")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

/// Get webhook service name from env (default: kulta-controller)
fn get_webhook_service_name() -> String {
    std::env::var("KULTA_SERVICE_NAME").unwrap_or_else(|_| "kulta-controller".to_string())
}

/// Get controller namespace from env (default: kulta-system)
fn get_controller_namespace() -> String {
    std::env::var("KULTA_NAMESPACE").unwrap_or_else(|_| "kulta-system".to_string())
}

/// Error policy for the controller
///
/// Determines how to handle reconciliation errors:
/// - Requeue after delay (exponential backoff)
///
/// Uses `warn!` since reconciliation errors are expected and trigger retries.
pub fn error_policy(rollout: Arc<Rollout>, error: &ReconcileError, ctx: Arc<Context>) -> Action {
    warn!("Reconcile error (will retry): {:?}", error);

    // Record error metric
    if let Some(ref metrics) = ctx.metrics {
        // Determine strategy from rollout spec for metric labeling
        let strategy = if rollout.spec.strategy.simple.is_some() {
            "simple"
        } else if rollout.spec.strategy.blue_green.is_some() {
            "blue_green"
        } else {
            "canary"
        };
        // Duration unknown for errors (didn't complete), use 0
        metrics.record_reconciliation_error(strategy, 0.0);
    }

    Action::requeue(Duration::from_secs(10))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Starting KULTA progressive delivery controller");

    // Create shutdown channel for coordinated shutdown
    let (shutdown_controller, shutdown_signal) = shutdown_channel();

    // Create readiness state (initially not ready)
    let readiness = ReadinessState::new();

    // Create metrics registry
    let metrics = create_metrics().expect("Failed to create metrics registry");
    info!("Prometheus metrics registry initialized");

    // Create leader state
    let leader_state = LeaderState::new();

    // Create Kubernetes client first (needed for TLS init)
    let client = match Client::try_default().await {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "Failed to create Kubernetes client");
            return Err(e.into());
        }
    };
    info!("Connected to Kubernetes cluster");

    // Initialize TLS if webhook is enabled
    let webhook_tls_enabled = is_webhook_tls_enabled();
    let tls_config = if webhook_tls_enabled {
        let service_name = get_webhook_service_name();
        let namespace = get_controller_namespace();

        info!(
            service = %service_name,
            namespace = %namespace,
            "Initializing webhook TLS certificates"
        );

        match initialize_tls(&client, &service_name, &namespace, DEFAULT_TLS_SECRET_NAME).await {
            Ok(bundle) => match build_rustls_config(&bundle) {
                Ok(config) => {
                    info!("Webhook TLS initialized successfully");
                    Some(config)
                }
                Err(e) => {
                    error!(error = ?e, "Failed to build TLS config");
                    return Err(anyhow::anyhow!("TLS config error: {}", e));
                }
            },
            Err(e) => {
                error!(error = ?e, "Failed to initialize TLS certificates");
                return Err(anyhow::anyhow!("TLS init error: {}", e));
            }
        }
    } else {
        info!("Webhook TLS disabled - running HTTP only");
        None
    };

    // Start health/webhook server in background
    let health_readiness = readiness.clone();
    let health_metrics = metrics.clone();
    let health_handle = if let Some(config) = tls_config {
        // HTTPS mode - webhook enabled
        tokio::spawn(async move {
            if let Err(e) =
                run_health_server_tls(WEBHOOK_PORT, health_readiness, health_metrics, config).await
            {
                warn!(error = %e, "HTTPS server failed");
            }
        })
    } else {
        // HTTP mode - no webhook
        tokio::spawn(async move {
            if let Err(e) = run_health_server(HEALTH_PORT, health_readiness, health_metrics).await {
                warn!(error = %e, "Health server failed");
            }
        })
    };

    let server_port = if webhook_tls_enabled {
        WEBHOOK_PORT
    } else {
        HEALTH_PORT
    };
    let server_mode = if webhook_tls_enabled { "HTTPS" } else { "HTTP" };
    info!(
        port = server_port,
        mode = server_mode,
        "Server task spawned"
    );

    // Start leader election if enabled
    let leader_election_enabled = is_leader_election_enabled();
    let leader_handle = if leader_election_enabled {
        let leader_client = client.clone();
        let leader_config = LeaderConfig::from_env();
        let leader_state_clone = leader_state.clone();
        let leader_shutdown = shutdown_signal.clone();

        info!(
            holder_id = %leader_config.holder_id,
            "Leader election enabled"
        );

        Some(tokio::spawn(async move {
            run_leader_election(
                leader_client,
                leader_config,
                leader_state_clone,
                leader_shutdown,
            )
            .await;
        }))
    } else {
        info!("Leader election disabled - running as single instance");
        // If no leader election, we're always the leader
        leader_state.set_leader(true);
        None
    };

    // Create API for Rollout resources
    let rollouts = Api::<Rollout>::all(client.clone());

    // Create CDEvents sink (configured from env vars)
    let cdevents_sink = CDEventsSink::new();
    info!(
        enabled = std::env::var("KULTA_CDEVENTS_ENABLED").unwrap_or_else(|_| "false".to_string()),
        "CDEvents sink configured"
    );

    // Create Prometheus client (configured from env var)
    let prometheus_address =
        std::env::var("KULTA_PROMETHEUS_ADDRESS").unwrap_or_else(|_| "".to_string());
    let prometheus_client = if prometheus_address.is_empty() {
        info!("Prometheus address not configured - metrics analysis disabled");
        PrometheusClient::new("http://localhost:9090".to_string()) // Dummy address, metrics will be skipped
    } else {
        info!(address = %prometheus_address, "Prometheus client configured");
        PrometheusClient::new(prometheus_address)
    };

    // Create controller context (with metrics for observability)
    let ctx = if leader_election_enabled {
        Arc::new(Context::new_with_leader(
            client.clone(),
            cdevents_sink,
            prometheus_client,
            leader_state.clone(),
            Some(metrics.clone()),
        ))
    } else {
        Arc::new(Context::new(
            client.clone(),
            cdevents_sink,
            prometheus_client,
            Some(metrics.clone()),
        ))
    };

    // Mark as ready - controller is initialized and about to start
    //
    // Note: Readiness indicates "controller is healthy and initialized", NOT "is the active leader".
    // All replicas report ready even if leader election is enabled. This is intentional because:
    // 1. Non-leaders may become leaders at any time if the current leader fails
    // 2. The controller gracefully skips reconciliation when not leader (no errors)
    // 3. Kubernetes services/traffic should route to all healthy replicas for HA
    readiness.set_ready();
    info!("Controller ready, starting reconciliation loop");

    // Create the controller stream
    // Note: error_policy already logs errors with warn!, so we only log success here
    let controller = Controller::new(rollouts, watcher::Config::default())
        .run(reconcile, error_policy, ctx)
        .for_each(|res| async move {
            if let Ok(o) = res {
                info!("Reconciled: {:?}", o);
            }
            // Errors are logged in error_policy, no duplicate logging
        });

    // Run controller until shutdown signal received
    tokio::select! {
        _ = controller => {
            info!("Controller stream ended");
        }
        signal = wait_for_signal() => {
            info!(signal = signal, "Initiating graceful shutdown");
            // Mark not ready so K8s stops sending traffic during shutdown
            readiness.set_not_ready();
        }
    }

    // Trigger shutdown for all components
    shutdown_controller.shutdown();

    // Graceful shutdown sequence
    info!("Stopping components...");

    if let Some(handle) = leader_handle {
        handle.abort();
    }
    health_handle.abort();

    info!("KULTA controller shut down gracefully");
    Ok(())
}

#[cfg(test)]
#[path = "main_test.rs"]
mod tests;
