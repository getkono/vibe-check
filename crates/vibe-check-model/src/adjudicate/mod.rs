//! Turning accumulated escalations into a verdict.
//!
//! The single rule this module exists to enforce is that scrutiny only ever
//! rises. See [`accumulator`] for how that is made structural rather than
//! conventional, and why that file must stay free of submodules.
//!
//! That rule is now enforced twice, independently. A run accumulates into two
//! ledgers — see [`enforcement`] — because "advisory" must not become the
//! system's first downward operation. Both ledgers are the same only-rises
//! accumulator; only one of them becomes a verdict.

mod accumulator;
mod enforcement;

pub use accumulator::{Adjudication, Adjudicator, Escalation};
pub use enforcement::{Adjudicators, AdvisoryAdjudication, EnforcedAdjudication, Enforcement};
