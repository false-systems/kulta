//! HTTP server for health and metrics endpoints
//!
//! Provides Kubernetes health probes:
//! - `/healthz` - Liveness probe (process is running)
//! - `/readyz` - Readiness probe (controller is ready to serve)
//!
//! Also provides graceful shutdown handling for SIGTERM/SIGINT.

mod health;
pub mod shutdown;

pub use health::{run_health_server, ReadinessState};
pub use shutdown::{shutdown_channel, wait_for_signal, ShutdownController, ShutdownSignal};

#[cfg(test)]
#[path = "health_test.rs"]
mod health_tests;

#[cfg(test)]
#[path = "shutdown_test.rs"]
mod shutdown_tests;
