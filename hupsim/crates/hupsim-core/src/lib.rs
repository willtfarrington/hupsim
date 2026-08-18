//! hupsim-core — domain model for the HUP hospital-system simulator.
//!
//! Zero UI dependencies. Everything here is plain data + pure functions so the
//! Bevy app, the data compiler, and the (future) simulation models all share
//! one vocabulary.

pub mod aggregate;
pub mod color;
pub mod geo;
pub mod ids;
pub mod index;
pub mod layout;
pub mod matrix;
pub mod model;
pub mod ops;
pub mod patient;
pub mod provenance;
pub mod query;
pub mod route;
pub mod scenario;
pub mod workload;

pub mod prelude {
    pub use crate::aggregate::{AggregateMode, HospitalStats, UnitAggregate};
    pub use crate::color::{stability_color, Rgb, COLOR_BEDLESS, COLOR_OOS, COLOR_VACANT};
    pub use crate::ids::{NodeId, PatientId, RoomId, ServiceLineId, UnitId};
    pub use crate::index::HospitalIndex;
    pub use crate::matrix::{masked_mean_std, OccState, RoomMatrix};
    pub use crate::geo::GeoFrame;
    pub use crate::layout::{Bridge, BuildingLayout, CampusLayout, Footprint, InsetAnnotation};
    pub use crate::model::{
        Building, BuildingKind, Connection, ConnectionKind, ElevatorCore, FloorLevel,
        FloorSpec, Hospital, HospitalMeta, Room, RoomKind, RoomStatus, ServiceLine, Unit,
        UnitType,
    };
    pub use crate::ops::{self, OpError};
    pub use crate::patient::{Acuity, Patient, StabilityTier};
    pub use crate::provenance::{Confidence, Era, Provenance};
    pub use crate::query::{
        LocTier, PlacementContext, PlacementQuery, PlacementResult, PlacementWeights, TeleFit,
    };
    pub use crate::route::{
        Route, RouteEdge, RouteEdgeKind, RouteError, RouteGraph, RouteHop, RouteNode,
        RouteParams, TraversalPolicy,
    };
    pub use crate::scenario::{Scenario, ScenarioMeta, SCENARIO_SCHEMA_VERSION};
    pub use crate::workload::{
        compose, effective_rn_ratio, is_rn_staffed, room_load, RnLoadInput, WorkloadBreakdown,
        WorkloadWeights,
    };
}
