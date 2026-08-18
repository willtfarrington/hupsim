//! Newtype string identifiers. Strings (not integers) because they come from a
//! human-curated property graph (`wing.founders`, `room.pavilion.8.17`) and
//! must stay legible in scenario JSON diffs.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
    };
}

id_type!(
    /// A node in the campus graph: campus, building complex, building, or wing.
    NodeId
);
id_type!(
    /// A clinical unit (e.g. `unit.founders.14`).
    UnitId
);
id_type!(
    /// A single patient-care or boarding room (e.g. `room.hup_main.560`).
    RoomId
);
id_type!(
    /// A synthetic patient.
    PatientId
);
id_type!(
    /// A service-line umbrella (e.g. `line.medicine`), from service_lines.json.
    ServiceLineId
);
