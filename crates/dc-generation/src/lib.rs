//! The generation pipeline.
//!
//! Stages are described in `docs/AI_PIPELINE.md`. Plan facts become a
//! [`facts::FactSheet`] with opaque references; prompts are built only from
//! that sheet and the knowledge base; every model reply is parsed strictly,
//! checked by the deterministic validators, reviewed by a second call, and
//! repaired at most twice before it can be shown to a patient.

pub mod facts;
pub mod notes;
pub mod pipeline;
pub mod prompt;
pub mod schema;
pub mod validate;
pub mod variant;

#[cfg(test)]
mod test_support;

pub use pipeline::{Event, Generated, Models, Progress, SectionResult, SectionState, generate};
