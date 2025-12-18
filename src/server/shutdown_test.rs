//! Tests for graceful shutdown handling

use super::shutdown::*;
use std::time::Duration;

/// Test that shutdown channel works correctly
#[tokio::test]
async fn test_shutdown_channel_initially_not_shutdown() {
    let (_controller, signal) = shutdown_channel();

    // Initially not shutdown
    assert!(!signal.is_shutdown());
}

/// Test that shutdown can be triggered
#[tokio::test]
async fn test_shutdown_channel_triggers_shutdown() {
    let (controller, signal) = shutdown_channel();

    assert!(!signal.is_shutdown());

    controller.shutdown();

    assert!(signal.is_shutdown());
}

/// Test that wait completes when shutdown is triggered
#[tokio::test]
async fn test_shutdown_wait_completes_on_signal() {
    let (controller, mut signal) = shutdown_channel();

    // Spawn task that triggers shutdown after delay
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        controller.shutdown();
    });

    // This should complete when shutdown is triggered
    let result = tokio::time::timeout(Duration::from_secs(1), signal.wait()).await;

    assert!(
        result.is_ok(),
        "wait() should complete when shutdown triggered"
    );
    assert!(signal.is_shutdown());
}

/// Test that cloned signals all receive shutdown
#[tokio::test]
async fn test_shutdown_signal_clones_share_state() {
    let (controller, signal) = shutdown_channel();
    let signal2 = signal.clone();
    let signal3 = signal.clone();

    assert!(!signal.is_shutdown());
    assert!(!signal2.is_shutdown());
    assert!(!signal3.is_shutdown());

    controller.shutdown();

    assert!(signal.is_shutdown());
    assert!(signal2.is_shutdown());
    assert!(signal3.is_shutdown());
}
