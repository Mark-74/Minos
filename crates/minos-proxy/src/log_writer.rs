//! Tokio task that drains the bus's log channel into a `Storage` impl.

use std::sync::Arc;

use minos_core::LogEntry;
use minos_storage::Storage;
use tokio::sync::mpsc::UnboundedReceiver;

/// Spawn a task that drains `rx`, appends every received [`LogEntry`] to
/// `storage`, then fans the entry out to live subscribers via
/// `broadcast_tx`. Returns the join handle; the task exits cleanly when
/// every producer half of the channel has been dropped (`rx.recv()` returns
/// `None`).
///
/// `Storage::append_log` is synchronous; calls run on the blocking pool via
/// `spawn_blocking` so the data plane never waits on disk IO. The broadcast
/// send happens only after a successful persist; a send error means there
/// are no live subscribers, which is fine and ignored.
pub fn spawn_log_writer<S>(
    mut rx: UnboundedReceiver<LogEntry>,
    storage: Arc<S>,
    broadcast_tx: tokio::sync::broadcast::Sender<LogEntry>,
) -> tokio::task::JoinHandle<()>
where
    S: Storage + 'static,
{
    tokio::spawn(async move {
        while let Some(entry) = rx.recv().await {
            let s = Arc::clone(&storage);
            // Clone before moving into spawn_blocking so the entry survives
            // for the broadcast fan-out below.
            let entry_for_storage = entry.clone();
            let res = tokio::task::spawn_blocking(move || s.append_log(&entry_for_storage)).await;
            match res {
                Ok(Ok(())) => {
                    // Persisted; fan out to live subscribers. No subscribers
                    // means send fails — that is expected, so ignore it.
                    let _ = broadcast_tx.send(entry);
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "append_log failed; skipping broadcast");
                }
                Err(join) => tracing::error!(error = %join, "log writer blocking task panicked"),
            }
        }
    })
}
