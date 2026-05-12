//! A concrete filter together with the per-instance toggles the executor uses.

use crate::{Direction, Filter};
use std::sync::Arc;
use uuid::Uuid;

/// One entry in a service's pipeline: a built filter plus the per-instance
/// toggles that control how the executor treats it.
// Four independent boolean toggles (enabled, dry_run, on_inbound, on_outbound)
// are the right representation here: each is orthogonal and maps directly to a
// UI checkbox. A state-machine refactor would obscure intent without benefit.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone)]
pub struct FilterInstance {
    /// Stable id used for logging and UI deep-links.
    pub id: Uuid,
    /// Operator-supplied display name.
    pub display_name: String,
    /// If false, the executor skips this instance entirely.
    pub enabled: bool,
    /// If true, a `Block` verdict is converted to a log-only entry.
    pub dry_run: bool,
    /// Whether the executor should run this instance on inbound traffic.
    pub on_inbound: bool,
    /// Whether the executor should run this instance on outbound traffic.
    pub on_outbound: bool,
    /// The built filter. `Arc` so the executor can clone cheaply.
    pub filter: Arc<dyn Filter>,
}

impl FilterInstance {
    /// True if this instance applies to packets flowing in `dir`.
    #[must_use]
    pub fn applies_to(&self, dir: Direction) -> bool {
        match dir {
            Direction::Inbound => self.on_inbound,
            Direction::Outbound => self.on_outbound,
        }
    }
}

impl std::fmt::Debug for FilterInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterInstance")
            .field("id", &self.id)
            .field("display_name", &self.display_name)
            .field("enabled", &self.enabled)
            .field("dry_run", &self.dry_run)
            .field("on_inbound", &self.on_inbound)
            .field("on_outbound", &self.on_outbound)
            .field("filter_kind", &self.filter.kind())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Packet, Verdict};

    struct Noop;
    impl Filter for Noop {
        fn kind(&self) -> &'static str {
            "noop"
        }
        fn accepts(&self, _: &Packet) -> bool {
            true
        }
        fn inspect(&self, _: &Packet) -> Verdict {
            Verdict::Pass
        }
    }

    fn instance(on_in: bool, on_out: bool) -> FilterInstance {
        FilterInstance {
            id: Uuid::new_v4(),
            display_name: "test".into(),
            enabled: true,
            dry_run: false,
            on_inbound: on_in,
            on_outbound: on_out,
            filter: Arc::new(Noop),
        }
    }

    #[test]
    fn applies_to_respects_direction_toggles() {
        let in_only = instance(true, false);
        assert!(in_only.applies_to(Direction::Inbound));
        assert!(!in_only.applies_to(Direction::Outbound));

        let out_only = instance(false, true);
        assert!(!out_only.applies_to(Direction::Inbound));
        assert!(out_only.applies_to(Direction::Outbound));

        let both = instance(true, true);
        assert!(both.applies_to(Direction::Inbound));
        assert!(both.applies_to(Direction::Outbound));

        let neither = instance(false, false);
        assert!(!neither.applies_to(Direction::Inbound));
        assert!(!neither.applies_to(Direction::Outbound));
    }

    #[test]
    fn debug_includes_filter_kind_not_internals() {
        let inst = instance(true, true);
        let s = format!("{inst:?}");
        assert!(s.contains("filter_kind"));
        assert!(s.contains("noop"));
    }
}
