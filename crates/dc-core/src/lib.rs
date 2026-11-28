//! Domain model and API types for DentalClarity.
//!
//! This crate performs no I/O and compiles for both native targets and
//! `wasm32-unknown-unknown`, so the server and the web app share one
//! definition of every type that crosses the API.
//!
//! Facts that come from the practice management system (procedures, teeth,
//! phases, fees) live in [`plan::PlanSnapshot`] and are never produced by a
//! language model. Generated text is tracked separately with a
//! [`provenance::Provenance`] so the UI can always say where a sentence came
//! from.

pub mod analytics;
pub mod api;
pub mod color;
pub mod content;
pub mod mode;
pub mod money;
pub mod plan;
pub mod present;
pub mod presentation;
pub mod procedure;
pub mod provenance;
pub mod tooth;

pub use mode::Mode;
pub use money::Money;
pub use plan::{PlanSnapshot, PlannedProcedure};
pub use procedure::ProcedureCode;
pub use tooth::ToothRef;
