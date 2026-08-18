//! Serde model of `service_lines.json` — the ordered service-line taxonomy
//! and the total unit → line mapping (EP-8, drafted for owner review).

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ServiceLinesFile {
    pub lines: Vec<LineDef>,
    pub assignments: Vec<Assignment>,
}

#[derive(Debug, Deserialize)]
pub struct LineDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Assignment {
    pub unit: String,
    pub line: String,
    /// Rates the unit→line DERIVATION (the unit's own identity keeps its
    /// provenance in units.json): verified | inferred | unverified.
    pub confidence: String,
    /// Quotes the `service` string the assignment was derived from.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// True for units with no public service identity — the compiler surfaces
    /// these as a warning list instead of silently binning them.
    #[serde(default)]
    pub needs_owner_ruling: bool,
}

pub fn parse_service_lines(json: &str) -> Result<ServiceLinesFile, serde_json::Error> {
    serde_json::from_str(json)
}
