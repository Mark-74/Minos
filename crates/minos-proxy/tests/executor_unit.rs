//! Example tests for the pipeline executor.

use std::sync::Arc;

use minos_core::{Direction, Filter, FilterInstance, Packet, Verdict};
use minos_proxy::execute;
use tokio::sync::mpsc;
use uuid::Uuid;

struct AlwaysBlock {
    reason: &'static str,
}
impl Filter for AlwaysBlock {
    fn kind(&self) -> &'static str {
        "always-block"
    }
    fn accepts(&self, _: &Packet) -> bool {
        true
    }
    fn inspect(&self, _: &Packet) -> Verdict {
        Verdict::Block {
            reason: self.reason.to_string(),
        }
    }
}

struct AlwaysPass;
impl Filter for AlwaysPass {
    fn kind(&self) -> &'static str {
        "always-pass"
    }
    fn accepts(&self, _: &Packet) -> bool {
        true
    }
    fn inspect(&self, _: &Packet) -> Verdict {
        Verdict::Pass
    }
}

fn instance(filter: Arc<dyn Filter>, dry_run: bool, on_in: bool, on_out: bool) -> FilterInstance {
    FilterInstance {
        id: Uuid::nil(),
        display_name: "test".into(),
        enabled: true,
        dry_run,
        on_inbound: on_in,
        on_outbound: on_out,
        filter,
    }
}

#[test]
fn empty_pipeline_passes() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let bytes = b"hi";
    let p = Packet::Raw {
        bytes,
        direction: Direction::Inbound,
    };
    assert!(matches!(execute(&p, "svc", &[], &tx), Verdict::Pass));
}

#[test]
fn block_short_circuits_remaining_filters() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let blocker = Arc::new(AlwaysBlock { reason: "nope" });
    let passer = Arc::new(AlwaysPass);
    let pipeline = vec![
        instance(blocker, false, true, true),
        instance(passer, false, true, true),
    ];
    let bytes = b"hi";
    let p = Packet::Raw {
        bytes,
        direction: Direction::Inbound,
    };
    let v = execute(&p, "svc", &pipeline, &tx);
    assert!(matches!(v, Verdict::Block { .. }));
    let entry = rx.try_recv().expect("one log entry");
    assert_eq!(entry.reason, "nope");
    assert!(!entry.dry_run);
    assert!(rx.try_recv().is_err());
}

#[test]
fn dry_run_match_emits_log_but_continues() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let blocker = Arc::new(AlwaysBlock {
        reason: "would-block",
    });
    let pipeline = vec![instance(blocker, true, true, true)];
    let bytes = b"hi";
    let p = Packet::Raw {
        bytes,
        direction: Direction::Inbound,
    };
    let v = execute(&p, "svc", &pipeline, &tx);
    assert!(matches!(v, Verdict::Pass));
    let entry = rx.try_recv().expect("dry-run log entry");
    assert!(entry.dry_run);
    assert!(rx.try_recv().is_err());
}

#[test]
fn disabled_filter_is_skipped() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut inst = instance(Arc::new(AlwaysBlock { reason: "x" }), false, true, true);
    inst.enabled = false;
    let bytes = b"hi";
    let p = Packet::Raw {
        bytes,
        direction: Direction::Inbound,
    };
    let v = execute(&p, "svc", &[inst], &tx);
    assert!(matches!(v, Verdict::Pass));
    assert!(rx.try_recv().is_err());
}

#[test]
fn direction_off_skips() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    // inbound off, outbound on
    let inst = instance(Arc::new(AlwaysBlock { reason: "x" }), false, false, true);
    let bytes = b"hi";
    let p = Packet::Raw {
        bytes,
        direction: Direction::Inbound,
    };
    let v = execute(&p, "svc", &[inst], &tx);
    assert!(matches!(v, Verdict::Pass));
    assert!(rx.try_recv().is_err());
}
