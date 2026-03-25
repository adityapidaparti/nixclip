//! Periodic maintenance: prune old entries and garbage-collect orphan blobs.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use nixclip_core::error::Result;
use nixclip_core::PruneStats;

use crate::AppState;

/// Run the periodic pruning loop.
///
/// Every 5 minutes, prunes entries that exceed the configured retention period
/// or entry count, and garbage-collects orphan blobs.
pub async fn run(state: Arc<AppState>) -> Result<()> {
    let mut interval = tokio::time::interval(Duration::from_secs(300));

    // The first tick completes immediately — skip it so we don't prune on
    // startup (the store just opened and may still be warming up).
    interval.tick().await;

    loop {
        interval.tick().await;

        match run_once(&state) {
            Ok(stats) => {
                if stats.entries_deleted > 0 || stats.blobs_deleted > 0 {
                    info!(
                        entries_deleted = stats.entries_deleted,
                        blobs_deleted = stats.blobs_deleted,
                        bytes_freed = stats.bytes_freed,
                        "pruning completed"
                    );
                }
            }
            Err(e) => {
                warn!(error = %e, "pruning failed");
            }
        }
    }
}

/// Run a single prune pass.
///
/// This performs both the standard retention/max-entries pruning **and**
/// ephemeral-entry cleanup (entries flagged as ephemeral that have exceeded
/// their TTL).
pub fn run_once(state: &AppState) -> Result<PruneStats> {
    let config = state.config.blocking_read();
    let general = config.general.clone();
    drop(config);

    let store = state
        .store
        .lock()
        .map_err(|e| nixclip_core::NixClipError::Config(format!("store mutex poisoned: {e}")))?;

    // Standard retention / max-entries pruning.
    let mut stats = store.prune(&general)?;

    // Ephemeral entry cleanup — delete entries marked ephemeral whose
    // created_at is older than the configured TTL.
    let ephemeral_stats = store.prune_ephemeral(general.ephemeral_ttl_hours)?;

    if ephemeral_stats.entries_deleted > 0 {
        info!(
            entries_deleted = ephemeral_stats.entries_deleted,
            "ephemeral entries pruned"
        );
    }

    // Merge the ephemeral stats into the overall stats.
    stats.entries_deleted += ephemeral_stats.entries_deleted;
    stats.blobs_deleted += ephemeral_stats.blobs_deleted;
    stats.bytes_freed += ephemeral_stats.bytes_freed;

    Ok(stats)
}
