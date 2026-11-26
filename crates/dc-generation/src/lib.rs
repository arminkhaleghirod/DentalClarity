//! The generation pipeline.
//!
//! Stages are described in `docs/AI_PIPELINE.md`. This crate currently holds
//! the deterministic validators; prompt construction, section generation and
//! the review pass land in milestone M4.

pub mod validate;
