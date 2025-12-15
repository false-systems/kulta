//! Blue-Green deployment strategy
//!
//! Maintains two full environments (active and preview).
//! Traffic is 100% to active until promotion, then instant switch to preview.

use super::{RolloutStrategy, StrategyError};
use crate::controller::rollout::Context;
use crate::crd::rollout::{Rollout, RolloutStatus};
use async_trait::async_trait;

/// Blue-Green strategy handler
///
/// Implements blue-green deployment:
/// - Two full-size ReplicaSets (active + preview)
/// - Instant traffic cutover (no gradual shift)
/// - Preview environment for testing before promotion
/// - Optional auto-promotion after duration
pub struct BlueGreenStrategyHandler;

#[async_trait]
impl RolloutStrategy for BlueGreenStrategyHandler {
    fn name(&self) -> &'static str {
        "blue-green"
    }

    async fn reconcile_replicasets(
        &self,
        _rollout: &Rollout,
        _ctx: &Context,
    ) -> Result<(), StrategyError> {
        // TODO: Implement blue-green ReplicaSet reconciliation
        todo!("BlueGreen reconcile_replicasets not yet implemented")
    }

    async fn reconcile_traffic(
        &self,
        _rollout: &Rollout,
        _ctx: &Context,
    ) -> Result<(), StrategyError> {
        // TODO: Implement blue-green traffic routing
        todo!("BlueGreen reconcile_traffic not yet implemented")
    }

    fn compute_next_status(&self, _rollout: &Rollout) -> RolloutStatus {
        // TODO: Implement blue-green status computation
        todo!("BlueGreen compute_next_status not yet implemented")
    }

    fn supports_metrics_analysis(&self) -> bool {
        true // Blue-green can support metrics analysis
    }

    fn supports_manual_promotion(&self) -> bool {
        true // Blue-green supports manual promotion
    }
}
