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

use vibe_check_model::{LaneId, LeafId, RequirementId};

use crate::exec::ProcessPlan;

/// One unit of schedulable work.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Leaf {
    /// Stable identifier, unique within a run.
    ///
    /// [`LeafId`] rather than a `String` because this value is also the
    /// artifact-name suffix and is interpolated through a job matrix, a shell,
    /// and a `--id` flag before it comes back as evidence. Its constraints, and
    /// why uniqueness is the planner's job rather than the constructor's, are
    /// documented on the type.
    pub id: LeafId,
    /// The requirement this answers.
    pub requirement: RequirementId,
    /// What to run.
    pub plan: ProcessPlan,
    /// Which lane, for grouping and budgets.
    pub lane: LaneId,
}

/// Where a scheduler put the work.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Dispatch {
    /// The leaves ran here, and their evidence is available now.
    Completed {
        /// Identifiers of leaves that ran to a conclusion.
        leaf_ids: Vec<LeafId>,
        /// Identifiers of leaves whose task panicked.
        ///
        /// Disjoint from `leaf_ids`, and together
        /// with it a complete account of everything that was dispatched. That
        /// totality is the point: without it a leaf that panicked is
        /// indistinguishable from a leaf that was never scheduled, and the two
        /// need different answers — the first has a defect to report against
        /// the tool, the second has a plan that never asked for it.
        ///
        /// This is not a second failure channel. A tool that runs and fails is
        /// an ordinary result and appears in `leaf_ids`; this list is only for
        /// the leaves whose *harness* came apart, which produces no evidence at
        /// all and therefore leaves a capability unverified.
        panicked: Vec<LeafId>,
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
