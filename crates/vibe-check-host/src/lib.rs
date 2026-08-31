//! Side-effect ports.
//!
//! Everything that touches the network, the filesystem, a subprocess, or the
//! clock is declared here as a trait and implemented elsewhere. This is the only
//! crate in the workspace whose traits are `async`.
//!
//! That boundary is what makes the rest of the system testable and replayable:
//! classification, policy resolution, and adjudication are pure functions over
//! values these ports produce, so they can be exercised with no runtime, no
//! network, and no git repository — and a recorded bundle can be re-adjudicated
//! months later and checked against the verdict it originally got.
//!
//! # Authority is expressed as types, not flags
//!
//! Two splits carry real weight:
//!
//! - [`ForgeRead`](forge::ForgeRead) versus [`ForgeWrite`](forge::ForgeWrite):
//!   an anti-gaming probe holds only the former, so posting a comment is not
//!   something it can do, rather than something it is trusted not to do.
//! - Capabilities describe [`ProcessPlan`](exec::ProcessPlan) values and never
//!   hold an [`Exec`](exec::Exec), so running a program is likewise not
//!   available to them.
//!
//! Both are the same idea: withhold the capability instead of checking a
//! permission, because a check can be wrong and an absent method cannot be
//! called.

// See the note in `vibe-check-model`: library code must not panic, because a
// panic in the adjudicator is a non-verdict and a non-verdict reads as a pass.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod clock;
pub mod escape;
pub mod exec;
pub mod forge;
pub mod scheduler;
pub mod vcs;

pub use clock::{Clock, FixedClock, SystemClock};
pub use escape::{ADJUDICATIONS_REF, ESCAPES_REF, EscapeError, EscapeStore};
pub use exec::{
    Determinism, EnvPolicy, Exec, ExecError, NetworkPolicy, ProcessOutput, ProcessPlan,
};
pub use forge::{
    Artifact, ArtifactMeta, CheckConclusion, CheckRequest, CheckRun, CommentId, CommentMarker,
    ForgeError, ForgeRead, ForgeResult, ForgeWrite, NullForge, PullRequest, RepoId, RunRef,
};
pub use scheduler::{Dispatch, Leaf, Scheduler};
pub use vcs::{ChangeKind, FileChange, Vcs, VcsError};
