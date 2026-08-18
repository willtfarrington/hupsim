//! Serde model of `units.json` — the curated unit roster with room-generation
//! specs.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct UnitsFile {
    pub units: Vec<UnitEntry>,
}

#[derive(Debug, Deserialize)]
pub struct UnitEntry {
    pub id: String,
    pub name: String,
    pub service: String,
    pub unit_type: String,
    pub building: String,
    pub level: LevelSpec,
    #[serde(default)]
    pub elevator_core: Option<String>,
    pub rooms: RoomSpec,
    pub confidence: String,
    #[serde(default)]
    pub era: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// Room capability seeding rule (EP-8). Absent = no telemetry, no
    /// negative-pressure rooms (tiny test fixtures).
    #[serde(default)]
    pub capabilities: Option<CapabilitySpec>,
    /// Target staffing (EP-8). Absent = no target ratio compiled.
    #[serde(default)]
    pub staffing: Option<StaffingSpec>,
}

/// Unit-level room-capability rule: data, not code constants, so the owner
/// can correct per-unit as capabilities are verified.
#[derive(Debug, Deserialize)]
pub struct CapabilitySpec {
    /// Telemetry capability across the unit's rooms.
    pub telemetry: TelemetryRule,
    /// That many airborne-isolation rooms, seeded deterministically onto the
    /// FIRST rooms in generation order (no RNG).
    #[serde(default)]
    pub negative_pressure_first_n: u32,
    #[serde(default)]
    pub confidence: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryRule {
    All,
    None,
}

#[derive(Debug, Deserialize)]
pub struct StaffingSpec {
    /// Target patients per RN.
    pub target_rn_ratio: f32,
    #[serde(default)]
    pub confidence: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Floor level as authored: a number or a string like "G".
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum LevelSpec {
    Num(i32),
    Name(String),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum RoomSpec {
    /// HUP Main wayfinding ranges, e.g. "901-930, 951-955".
    Ranges {
        ranges: String,
        kind: String,
    },
    /// Pavilion floor × wing under the published 1-72 numbering scheme.
    PavilionWing {
        floor: u32,
        wing: PavilionWingName,
        kind: String,
        /// Explicit slot numbers (same syntax as `ranges`, e.g. "13-24"),
        /// overriding the wing's full block — for units occupying a partial
        /// wing or absorbing a neighbour wing's slots (EP-23: floor 12).
        #[serde(default)]
        slots: Option<String>,
    },
    /// Bed-count-only units (CHPS, ED, L&D…).
    Count {
        count: u32,
        #[serde(default)]
        number_prefix: String,
        kind: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PavilionWingName {
    City,
    Center,
    Campus,
}

impl PavilionWingName {
    /// Published Pavilion room-numbering scheme:
    /// City 1-12 & 61-72 · Center 13-24 & 49-60 · Campus 25-48.
    pub fn room_numbers(self) -> Vec<u32> {
        match self {
            PavilionWingName::City => (1..=12).chain(61..=72).collect(),
            PavilionWingName::Center => (13..=24).chain(49..=60).collect(),
            PavilionWingName::Campus => (25..=48).collect(),
        }
    }
}

/// Parse "901-930, 951-955" | "560-591" | "650" into individual numbers.
pub fn parse_ranges(s: &str) -> Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('-') {
            Some((a, b)) => {
                let a: u32 = a
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad range start in {part:?}"))?;
                let b: u32 = b
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad range end in {part:?}"))?;
                if b < a {
                    return Err(format!("descending range {part:?}"));
                }
                out.extend(a..=b);
            }
            None => out.push(
                part.parse()
                    .map_err(|_| format!("bad room number {part:?}"))?,
            ),
        }
    }
    Ok(out)
}

pub fn parse_units(json: &str) -> Result<UnitsFile, serde_json::Error> {
    serde_json::from_str(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_parse() {
        assert_eq!(parse_ranges("560-563").unwrap(), vec![560, 561, 562, 563]);
        assert_eq!(
            parse_ranges("901-903, 951-952").unwrap(),
            vec![901, 902, 903, 951, 952]
        );
        assert!(parse_ranges("930-901").is_err());
        assert!(parse_ranges("abc").is_err());
    }

    #[test]
    fn pavilion_wing_counts_total_72() {
        let total = PavilionWingName::City.room_numbers().len()
            + PavilionWingName::Center.room_numbers().len()
            + PavilionWingName::Campus.room_numbers().len();
        assert_eq!(total, 72);
        // No overlaps
        let mut all: Vec<u32> = PavilionWingName::City
            .room_numbers()
            .into_iter()
            .chain(PavilionWingName::Center.room_numbers())
            .chain(PavilionWingName::Campus.room_numbers())
            .collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 72);
        assert_eq!(all.first(), Some(&1));
        assert_eq!(all.last(), Some(&72));
    }
}
