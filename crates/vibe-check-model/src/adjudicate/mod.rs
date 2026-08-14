//! Turning accumulated escalations into a verdict.
//!
//! The single rule this module exists to enforce is that scrutiny only ever
//! rises. See [`accumulator`] for how that is made structural rather than
//! conventional, and why that file must stay free of submodules.

mod accumulator;

pub use accumulator::{Adjudication, Adjudicator, Escalation};
