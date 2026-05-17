//! The filter pipeline executor. Pure function: no IO, no tokio runtime.

use minos_core::{FilterInstance, LogEntry, Packet, Verdict};
use tokio::sync::mpsc::UnboundedSender;

use crate::sample::{default_sample_cap, sample_from_packet};

/// Run a packet through a list of filter instances and return a [`Verdict`].
///
/// Filters that are disabled, that do not apply to the packet's direction, or
/// that decline the packet (`accepts() == false`) are skipped. A `Block` from
/// a dry-run instance emits a [`LogEntry`] on `log` and continues; a `Block`
/// from a non-dry-run instance emits a [`LogEntry`] and short-circuits.
///
/// `log` is the producer half of the bus's mpsc channel. Send failures (the
/// receiver dropped during shutdown) are intentionally ignored: the verdict
/// is what matters on the hot path.
#[must_use]
pub fn execute(
    packet: &Packet<'_>,
    service: &str,
    pipeline: &[FilterInstance],
    log: &UnboundedSender<LogEntry>,
) -> Verdict {
    let direction = packet.direction();
    for instance in pipeline {
        if !instance.enabled {
            continue;
        }
        if !instance.applies_to(direction) {
            continue;
        }
        if !instance.filter.accepts(packet) {
            continue;
        }
        if let Verdict::Block { reason } = instance.filter.inspect(packet) {
            let entry = LogEntry {
                id: None,
                ts: now_ms(),
                service: service.to_string(),
                direction,
                filter_id: instance.id,
                rule_kind: instance.filter.kind().to_string(),
                dry_run: instance.dry_run,
                reason: reason.clone(),
                sample: sample_from_packet(packet, default_sample_cap()),
            };
            let _ = log.send(entry);
            if !instance.dry_run {
                return Verdict::Block { reason };
            }
        }
    }
    Verdict::Pass
}

#[must_use]
#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
fn now_ms() -> i64 {
    // u128 ms since UNIX_EPOCH fits in i64 until year ~292,277,000.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
