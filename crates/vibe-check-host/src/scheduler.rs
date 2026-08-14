//! Deciding where work runs.
//!
//! # Why this is a port and not a design decision baked into the engine
//!
//! Heavy capabilities cannot share a `target/` directory even in principle —
//! Miri needs a different sysroot, `cargo-mutants` rewrites the source tree,
//! `loom` builds under a different `cfg`, and a feature powerset invalidates
//! itself on every combination. Running them as separate CI jobs is therefore a
//! strict win, and each gets its own cache scope.
//!
//! Cheap capabilities are the opposite: clippy, `cargo-deny`, and a public-API
//! diff all reuse one warm build. Splitting them into jobs multiplies cache
//! restore — tens of seconds each — to save a fraction of that in runtime.
//!
//! So there are two strategies, and which one applies is a property of the
//! environment rather than of the capability. Making it a trait means the local
//! path and the CI path share the code that actually runs the tool, and differ
//! only in who decides when. A future strategy for self-hosted runners is an
//! additional implementation, not a change to any of this.

use async_trait::async_trait;

use vibe_check_model::RequirementId;

use crate::exec::ProcessPlan;

/// One unit of schedulable work.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Leaf {
    /// Stable identifier, unique within a run.
    ///
    /// Also the artifact-name suffix, which must be unique per run — uploading
    /// two artifacts under one name is an error, not a merge.
    pub id: String,
    /// The requirement this answers.
    pub requirement: RequirementId,
    /// What to run.
    pub plan: ProcessPlan,
    /// Which lane, for grouping and budgets.
    pub lane: String,
}

/// Where a scheduler put the work.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Dispatch {
    /// The leaves ran here, and their evidence is available now.
    Completed {
        /// Identifiers of leaves that ran.
        leaf_ids: Vec<String>,
    },
    /// The leaves were handed to an external scheduler.
    ///
    /// Evidence will arrive later, as artifacts. The engine must not block
    /// waiting for it — in CI the work has not started yet, and the job that
    /// collects it has not been created.
    Deferred {
        /// An opaque payload for the external scheduler, e.g. a job matrix.
        payload: String,
    },
}

/// Decides where planned work runs.
#[async_trait]
pub trait Scheduler: Send + Sync {
    /// Dispatch a batch of leaves.
    async fn dispatch(&self, leaves: Vec<Leaf>) -> Dispatch;
}
