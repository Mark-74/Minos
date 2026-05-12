//! The core `Filter` trait every filter type implements.

use crate::{Packet, Verdict};

/// A single filter in a pipeline. Implementations answer "does this packet
/// match my rule?" and nothing else — toggles like dry-run, direction, and
/// instance metadata are handled by the pipeline executor (in `minos-proxy`).
///
/// Implementations must be `Send + Sync` so the pipeline can run them from
/// any tokio worker. The trait is sync; filters that need blocking IO (e.g.
/// the python sidecar) wrap it in `tokio::task::block_in_place`.
pub trait Filter: Send + Sync {
    /// Stable identifier for the filter kind, e.g. `"regex"`. Must match
    /// the corresponding `FilterKind::NAME`.
    fn kind(&self) -> &'static str;

    /// Whether this filter is interested in inspecting the given packet.
    /// E.g. an HTTP-header filter returns false on `Packet::Raw`. The
    /// executor will skip `inspect` if this returns false.
    fn accepts(&self, p: &Packet) -> bool;

    /// Inspect the packet and return a verdict.
    fn inspect(&self, p: &Packet) -> Verdict;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Direction;

    /// Toy filter used to exercise the trait shape from inside this crate.
    /// Real filter impls live in `minos-filters`.
    struct AlwaysPass;
    impl Filter for AlwaysPass {
        fn kind(&self) -> &'static str {
            "always_pass"
        }
        fn accepts(&self, _: &Packet) -> bool {
            true
        }
        fn inspect(&self, _: &Packet) -> Verdict {
            Verdict::Pass
        }
    }

    struct BlockOnSubstr {
        needle: Vec<u8>,
    }
    impl Filter for BlockOnSubstr {
        fn kind(&self) -> &'static str {
            "block_on_substr"
        }
        fn accepts(&self, p: &Packet) -> bool {
            matches!(p, Packet::Raw { .. })
        }
        fn inspect(&self, p: &Packet) -> Verdict {
            let bytes = match p {
                Packet::Raw { bytes, .. } => *bytes,
                Packet::Http { .. } => return Verdict::Pass,
            };
            if bytes.windows(self.needle.len()).any(|w| w == self.needle) {
                Verdict::Block {
                    reason: "matched".into(),
                }
            } else {
                Verdict::Pass
            }
        }
    }

    #[test]
    fn always_pass_passes_everything() {
        let f = AlwaysPass;
        let pkt = Packet::Raw {
            bytes: b"anything",
            direction: Direction::Inbound,
        };
        assert!(f.accepts(&pkt));
        assert_eq!(f.inspect(&pkt), Verdict::Pass);
        assert_eq!(f.kind(), "always_pass");
    }

    #[test]
    fn block_on_substr_only_accepts_raw() {
        let f = BlockOnSubstr {
            needle: b"evil".to_vec(),
        };
        let raw = Packet::Raw {
            bytes: b"hello evil world",
            direction: Direction::Inbound,
        };
        let req = crate::ParsedHttp::new("GET", "/", vec![], b"evil".to_vec());
        let http = Packet::Http {
            req: &req,
            direction: Direction::Inbound,
        };
        assert!(f.accepts(&raw));
        assert!(!f.accepts(&http));
        assert_eq!(
            f.inspect(&raw),
            Verdict::Block {
                reason: "matched".into()
            }
        );
    }

    #[test]
    fn dyn_dispatch_works() {
        let filters: Vec<Box<dyn Filter>> = vec![
            Box::new(AlwaysPass),
            Box::new(BlockOnSubstr {
                needle: b"x".into(),
            }),
        ];
        for f in &filters {
            let _ = f.kind();
        }
    }
}
