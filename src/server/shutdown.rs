//! Graceful shutdown handling for KULTA controller
//!
//! Handles SIGTERM and SIGINT signals for clean shutdown:
//! - Stops accepting new reconciliations
//! - Waits for in-flight work to complete
//! - Cleans up resources

use tokio::sync::watch;
use tracing::info;

/// Shutdown signal sender/receiver pair
///
/// The sender is used to trigger shutdown, the receiver is used to wait for it.
#[derive(Clone)]
pub struct ShutdownSignal {
    receiver: watch::Receiver<bool>,
}

impl ShutdownSignal {
    /// Wait for shutdown signal
    pub async fn wait(&mut self) {
        // Wait until the value becomes true
        while !*self.receiver.borrow() {
            if self.receiver.changed().await.is_err() {
                // Sender dropped, treat as shutdown
                break;
            }
        }
    }

    /// Check if shutdown was signaled (non-blocking)
    pub fn is_shutdown(&self) -> bool {
        *self.receiver.borrow()
    }
}

/// Controller for triggering shutdown
pub struct ShutdownController {
    sender: watch::Sender<bool>,
}

impl ShutdownController {
    /// Trigger shutdown
    pub fn shutdown(&self) {
        let _ = self.sender.send(true);
        info!("Shutdown signal sent");
    }
}

/// Create a new shutdown signal pair
///
/// Returns (controller, signal) where:
/// - controller: Used to trigger shutdown
/// - signal: Cloned and passed to components that need to listen
pub fn shutdown_channel() -> (ShutdownController, ShutdownSignal) {
    let (sender, receiver) = watch::channel(false);
    (ShutdownController { sender }, ShutdownSignal { receiver })
}

/// Wait for SIGTERM or SIGINT signal
///
/// This function blocks until a termination signal is received.
/// Returns the signal name that was received.
///
/// # Panics
/// Panics if signal handlers cannot be registered (OS resource exhaustion).
#[cfg(unix)]
pub async fn wait_for_signal() -> &'static str {
    use tokio::signal::unix::{signal, SignalKind};
    use tracing::error;

    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "Failed to register SIGTERM handler");
            panic!("Cannot register SIGTERM handler: {}", e);
        }
    };
    let mut sigint = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "Failed to register SIGINT handler");
            panic!("Cannot register SIGINT handler: {}", e);
        }
    };

    tokio::select! {
        _ = sigterm.recv() => {
            info!("Received SIGTERM");
            "SIGTERM"
        }
        _ = sigint.recv() => {
            info!("Received SIGINT");
            "SIGINT"
        }
    }
}

/// Wait for Ctrl+C signal (Windows)
///
/// # Panics
/// Panics if Ctrl+C handler cannot be registered.
#[cfg(not(unix))]
pub async fn wait_for_signal() -> &'static str {
    use tracing::error;

    if let Err(e) = tokio::signal::ctrl_c().await {
        error!(error = %e, "Failed to wait for Ctrl+C");
        panic!("Cannot wait for Ctrl+C: {}", e);
    }
    info!("Received Ctrl+C");
    "CTRL_C"
}
