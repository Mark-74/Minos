//! Tokio task that drains the bus's log channel into a `Storage` impl.

use std::sync::Arc;

use minos_core::LogEntry;
use minos_storage::Storage;
use tokio::sync::mpsc::UnboundedReceiver;

/// Spawn a task that drains `rx` and appends every received [`LogEntry`] to
/// `storage`. Returns the join handle; the task exits cleanly when every
/// producer half of the channel has been dropped (`rx.recv()` returns
/// `None`).
///
/// `Storage::append_log` is synchronous; calls run on the blocking pool via
/// `spawn_blocking` so the data plane never waits on disk IO.
pub fn spawn_log_writer<S>(
    mut rx: UnboundedReceiver<LogEntry>,
    storage: Arc<S>,
) -> tokio::task::JoinHandle<()>
where
    S: Storage + 'static,
{
    tokio::spawn(async move {
        while let Some(entry) = rx.recv().await {
            let s = Arc::clone(&storage);
            let res = tokio::task::spawn_blocking(move || s.append_log(&entry)).await;
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(error = %e, "append_log failed"),
                Err(join) => tracing::error!(error = %join, "log writer blocking task panicked"),
            }
        }
    })
}
