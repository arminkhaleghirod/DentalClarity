//! The five patient screens: At a glance, Journey, Procedures, Costs and
//! Next steps (mockups 2d, 2e, 2f; the rest follow docs/UI_SPEC.md section 5.11).
//! One file per screen.

mod at_a_glance;
mod common;
mod costs;
mod journey;
mod next_steps;
mod procedures;

pub use at_a_glance::AtAGlance;
pub use costs::Costs;
pub use journey::Journey;
pub use next_steps::NextSteps;
pub use procedures::Procedures;
