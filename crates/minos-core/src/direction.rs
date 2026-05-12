//! Inbound vs outbound direction of a packet relative to the protected service.

use serde::{Deserialize, Serialize};

/// Direction of a packet flowing through the firewall, relative to the
/// protected service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// From the network into the service.
    Inbound,
    /// From the service back to the network.
    Outbound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_serde() {
        for dir in [Direction::Inbound, Direction::Outbound] {
            let json = serde_json::to_string(&dir).unwrap();
            let back: Direction = serde_json::from_str(&json).unwrap();
            assert_eq!(dir, back);
        }
    }

    #[test]
    fn serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Direction::Inbound).unwrap(),
            "\"inbound\""
        );
        assert_eq!(
            serde_json::to_string(&Direction::Outbound).unwrap(),
            "\"outbound\""
        );
    }
}
