//! Tolerant serde mirror of the ad-hoc `hup_topology.json` property graph.
//! Only the fields the compiler consumes are modeled; everything else is
//! ignored so the source file can keep evolving.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct RawTopology {
    #[serde(default)]
    pub meta: RawMeta,
    #[serde(default)]
    pub nodes: Vec<RawNode>,
    #[serde(default)]
    pub edges: Vec<RawEdge>,
    #[serde(default)]
    pub elevator_cores: Vec<RawCore>,
}

#[derive(Debug, Deserialize, Default)]
pub struct RawMeta {
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub compiled: String,
    #[serde(default)]
    pub known_limitations: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct RawNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Optional display short name (e.g. "Clifton" for bldg.pavilion);
    /// overrides the compiler's id-derived default.
    #[serde(default)]
    pub short_name: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub built: Option<i32>,
    /// e.g. "G-7", "1-14" — authoritative floor labels for the wing.
    #[serde(default)]
    pub floor_span: Option<String>,
    /// Usually a bool, but PCAM is `"edge_case"` — hence Value.
    #[serde(default)]
    pub has_inpatient_beds: Option<serde_json::Value>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub confidence: Option<String>,
}

impl RawNode {
    pub fn beds_bool(&self) -> bool {
        match &self.has_inpatient_beds {
            Some(serde_json::Value::Bool(b)) => *b,
            // "edge_case" (PCAM research beds) counts as bedded here.
            Some(serde_json::Value::String(_)) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RawEdge {
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Reference to a `bridges[]` deck id in layout.json (two parallel
    /// Clifton↔PCAM bridges share a (from, to) pair, so the pair alone is
    /// ambiguous).
    #[serde(default)]
    pub layout: Option<String>,
    /// Sometimes an int, sometimes a string ("elevated over 34th St").
    #[serde(default)]
    pub level: Option<serde_json::Value>,
    #[serde(default)]
    pub levels: Vec<i32>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub confidence: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

impl RawEdge {
    /// Integer levels from either field form.
    pub fn levels_vec(&self) -> Vec<i32> {
        if !self.levels.is_empty() {
            return self.levels.clone();
        }
        match &self.level {
            Some(serde_json::Value::Number(n)) => {
                n.as_i64().map(|v| vec![v as i32]).unwrap_or_default()
            }
            _ => vec![],
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RawCore {
    pub core: String,
    #[serde(default)]
    pub serves_patient_floors: Vec<i32>,
    #[serde(default)]
    pub note: Option<String>,
}

pub fn parse_topology(json: &str) -> Result<RawTopology, serde_json::Error> {
    serde_json::from_str(json)
}
