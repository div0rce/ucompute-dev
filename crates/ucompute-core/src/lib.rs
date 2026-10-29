//! Shared types for UCompute.
//!
//! Everything here is dependency-light on purpose: every other crate depends on it.

pub mod error;
pub mod manifest;
pub mod paths;
pub mod timeout;

pub use error::{ErrorCode, Result, UComputeError};
pub use manifest::{ExecutionManifest, HostSummary};
pub use paths::Paths;
pub use timeout::Timeouts;

/// Semantic version of the UCompute software, taken from the workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
