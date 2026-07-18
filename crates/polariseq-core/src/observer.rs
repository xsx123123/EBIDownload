//! Download observer trait used to bridge the core download engine with the
//! CLI's terminal UI layer (status bar / completion accounting) without creating
//! a circular dependency (CLI depends on core, never the reverse).
//!
//! The core only ever holds an `Option<Arc<dyn DownloadObserver>>`; concrete
//! implementations live in the CLI crate (`UiManager`).

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// Metadata captured when a single download finishes successfully.
#[derive(Clone, Debug)]
pub struct CompletedInfo {
    pub id: String,
    pub total_bytes: u64,
    pub elapsed_secs: f64,
    pub avg_speed_bps: f64,
}

/// Observer the UI layer implements so the download engine can report
/// lifecycle events and share live byte counters.
///
/// All methods have default no-op implementations, so callers that don't care
/// about UI (tests, library embeds) can use `None` and pay no cost.
pub trait DownloadObserver: Send + Sync {
    /// Set the total number of items to be processed (for queued/active math).
    fn set_total(&self, _total: u64) {}

    /// Register an active download, returning a shared byte counter the engine
    /// will update as bytes flow. The UI sums live counters to compute speed.
    fn register(&self, _id: &str, _total: u64) -> Arc<AtomicU64> {
        Arc::new(AtomicU64::new(0))
    }

    /// Drop a previously registered active download from the live set.
    fn unregister(&self, _id: &str) {}

    /// Mark a download as completed and record its metadata.
    fn complete(&self, _info: CompletedInfo) {}

    /// Mark a download as failed.
    fn fail(&self, _id: &str) {}
}
