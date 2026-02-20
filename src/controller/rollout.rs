pub mod reconcile;
pub mod replicaset;
pub mod status;
pub mod traffic;
pub mod validation;

// Re-export everything so external API is unchanged
pub use reconcile::*;
pub use replicaset::*;
pub use status::*;
pub use traffic::*;
pub use validation::*;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Tests can use unwrap/expect for brevity
#[path = "rollout_test.rs"]
mod tests;
