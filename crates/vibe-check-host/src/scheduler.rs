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

use std::collections::BTreeSet;

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
    /// documented on the type; [`Leaves`] is where the batch-wide half of that
    /// is enforced.
    pub id: LeafId,
    /// The requirement this answers.
    pub requirement: RequirementId,
    /// What to run.
    pub plan: ProcessPlan,
    /// Which lane, for grouping and budgets.
    pub lane: LaneId,
}

/// Why a batch of leaves was rejected.
///
/// Carries the offending identifier rather than only the fact of a collision:
/// the batch is minted by a planner from a policy document, and "which id"
/// is the difference between a message that gets fixed and one that gets
/// shrugged at. Follows [`LeafIdError`](vibe_check_model::LeafIdError) in that.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[error(
    "leaf id `{id}` appears more than once in one batch; a leaf id is also the \
     artifact-name suffix, and uploading two artifacts under one name is an \
     error rather than a merge"
)]
#[non_exhaustive]
pub struct DuplicateLeafId {
    /// The identifier that appeared more than once.
    pub id: LeafId,
}

/// A batch of leaves whose identifiers are distinct.
///
/// # Why the batch is a type
///
/// [`LeafId`] guarantees each identifier is well-formed, and says so itself
/// that uniqueness "is a property of a *set* of ids and no constructor can
/// enforce it". This is that set. Uniqueness matters because the id is also
/// the artifact-name suffix: two leaves sharing one id upload two artifacts
/// under one name, which a forge treats as an error rather than as a merge —
/// and if it did merge them, evidence for one capability would arrive
/// attributed to another.
///
/// [`Scheduler::dispatch`] takes a `Leaves` rather than a `Vec<Leaf>` so that
/// the check happens once, at the only place a batch can be built, instead of
/// being a rule each implementation is trusted to remember. It is the same
/// move as [`LeafId::new_checked`] being the only constructor, and as
/// `ForgeRead` and `ForgeWrite` being separate traits: withhold the
/// capability rather than check a permission, because a check can be
/// forgotten and an absent constructor cannot be called.
///
/// A future [`Deferred`](Dispatch::Deferred) scheduler is where this pays.
/// It does not run the leaves, it emits a job matrix naming them, and a
/// duplicate id there is discovered hours later as a failed artifact upload
/// in a job nobody is watching, with the capability it belonged to left
/// unverified.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Leaves(Vec<Leaf>);

impl Leaves {
    /// Build a batch, rejecting one that repeats an identifier.
    ///
    /// # Errors
    /// Returns [`DuplicateLeafId`] naming the first identifier seen twice.
    pub fn new(leaves: Vec<Leaf>) -> Result<Self, DuplicateLeafId> {
        // `BTreeSet`, not a hash set: `clippy.toml` bans the latter workspace-
        // wide because its iteration order reaches digests. Nothing iterates
        // this one, but keeping to the sanctioned container costs nothing and
        // leaves no `#[allow]` for a later reader to have to evaluate.
        let mut seen = BTreeSet::new();
        for leaf in &leaves {
            if !seen.insert(&leaf.id) {
                return Err(DuplicateLeafId {
                    id: leaf.id.clone(),
                });
            }
        }
        Ok(Self(leaves))
    }

    /// The leaves, in the order they were given.
    #[must_use]
    pub fn as_slice(&self) -> &[Leaf] {
        &self.0
    }

    /// How many leaves are in the batch.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the batch is empty.
    ///
    /// An empty batch is well-formed — a plan that asks for nothing is not an
    /// error — so this is a question, not a guard.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<Vec<Leaf>> for Leaves {
    type Error = DuplicateLeafId;

    fn try_from(leaves: Vec<Leaf>) -> Result<Self, Self::Error> {
        Self::new(leaves)
    }
}

impl IntoIterator for Leaves {
    type Item = Leaf;
    type IntoIter = std::vec::IntoIter<Leaf>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Leaves {
    type Item = &'a Leaf;
    type IntoIter = std::slice::Iter<'a, Leaf>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
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
    ///
    /// Takes [`Leaves`] rather than a `Vec<Leaf>` so an implementation cannot
    /// be handed a batch that repeats an identifier.
    async fn dispatch(&self, leaves: Leaves) -> Dispatch;
}

#[cfg(test)]
mod tests {
    use std::task::{Context, Poll, Waker};

    use super::*;

    fn leaf(id: &str, lane: &str) -> Leaf {
        Leaf {
            id: LeafId::new_checked(id).expect("a well-formed fixture leaf id"),
            requirement: RequirementId::new(format!("req_{id}")),
            plan: ProcessPlan::new("cargo", ["check".to_owned()]),
            lane: LaneId::new(lane),
        }
    }

    /// Poll a dispatch future exactly once, without a runtime and without a
    /// waker that can ever wake it.
    ///
    /// The point is that it *cannot* drive a future to completion. Anything
    /// that comes back `Ready` here was already finished when it was handed
    /// over, which is the property [`Dispatch::Deferred`] is documented to
    /// have and the reason the engine may not block on one. The floor
    /// assertion for this helper is
    /// `the_poll_helper_can_observe_a_future_that_is_not_ready`, below: without
    /// it, a helper that returned `Ready` unconditionally would pass every
    /// other test in this module.
    fn poll_once(scheduler: &dyn Scheduler, leaves: Leaves) -> Poll<Dispatch> {
        let mut future = scheduler.dispatch(leaves);
        let mut context = Context::from_waker(Waker::noop());
        future.as_mut().poll(&mut context)
    }

    /// Hands the batch to an external scheduler and returns immediately.
    ///
    /// What `ActionsScheduler` will be in M4: it emits a job matrix and does
    /// not await anything, because in CI the work has not started and the job
    /// that collects the evidence does not exist yet.
    struct DeferringScheduler;

    #[async_trait]
    impl Scheduler for DeferringScheduler {
        async fn dispatch(&self, leaves: Leaves) -> Dispatch {
            let payload = leaves
                .as_slice()
                .iter()
                .map(|leaf| leaf.id.as_str())
                .collect::<Vec<_>>()
                .join(",");
            Dispatch::Deferred { payload }
        }
    }

    /// Runs the batch here and reports what happened.
    struct CompletingScheduler;

    #[async_trait]
    impl Scheduler for CompletingScheduler {
        async fn dispatch(&self, leaves: Leaves) -> Dispatch {
            Dispatch::Completed {
                leaf_ids: leaves.into_iter().map(|leaf| leaf.id).collect(),
                panicked: Vec::new(),
            }
        }
    }

    /// Never finishes. Exists only to prove `poll_once` can say `Pending`.
    struct BlockingScheduler;

    #[async_trait]
    impl Scheduler for BlockingScheduler {
        async fn dispatch(&self, _leaves: Leaves) -> Dispatch {
            std::future::pending::<()>().await;
            unreachable!("a pending future never resolves")
        }
    }

    fn batch(ids: &[&str]) -> Leaves {
        Leaves::new(ids.iter().map(|id| leaf(id, "cheap")).collect())
            .expect("fixture ids are distinct")
    }

    #[test]
    fn a_leaf_carries_its_lane_as_a_lane_id() {
        // The first acceptance criterion of #7, which the type already meets:
        // `lane` is a `LaneId`, not an arbitrary `String`. Asserted rather than
        // changed, so that widening it back to a string is a test failure.
        let leaf = leaf("miri-core-0", "heavy");
        let lane: LaneId = leaf.lane.clone();
        assert_eq!(lane.as_str(), "heavy");
        assert_eq!(leaf.lane, LaneId::new("heavy"));
        // And it is not interchangeable with the requirement or the id, which
        // are their own newtypes over the same underlying string type.
        assert_eq!(leaf.id.as_str(), "miri-core-0");
        assert_eq!(leaf.requirement.as_str(), "req_miri-core-0");
    }

    #[test]
    fn a_batch_of_distinct_ids_is_accepted() {
        // The floor for the rejection tests below: a guard that rejected every
        // batch would satisfy them all and stop the scheduler working, so the
        // accepting case has to be asserted alongside.
        let leaves = Leaves::new(vec![
            leaf("a", "cheap"),
            leaf("b", "cheap"),
            leaf("c", "heavy"),
        ])
        .expect("distinct ids");
        assert_eq!(leaves.len(), 3);
        assert!(!leaves.is_empty());
        assert_eq!(
            leaves
                .as_slice()
                .iter()
                .map(|leaf| leaf.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"],
            "the batch preserves the order it was given"
        );
    }

    #[test]
    fn a_repeated_leaf_id_is_rejected_and_named() {
        // Non-vacuous by construction: the same three leaves are accepted in
        // `a_batch_of_distinct_ids_is_accepted` above with only the third id
        // changed, so this failure is caused by the repetition and by nothing
        // else about the fixture.
        let error = Leaves::new(vec![
            leaf("a", "cheap"),
            leaf("b", "cheap"),
            leaf("a", "heavy"),
        ])
        .expect_err("a repeated id is not a batch");
        assert_eq!(error.id.as_str(), "a");
        assert!(
            error.to_string().contains("artifact-name suffix"),
            "the message says why a duplicate is not a merge: {error}"
        );
    }

    #[test]
    fn duplicate_detection_reads_the_id_and_nothing_else() {
        // Two leaves that differ in every other field still collide, because
        // it is the id alone that becomes the artifact-name suffix.
        let mut second = leaf("shared", "heavy");
        second.requirement = RequirementId::new("something-else");
        second.plan = ProcessPlan::new("miri", ["test".to_owned()]);
        let error = Leaves::new(vec![leaf("shared", "cheap"), second])
            .expect_err("differing elsewhere does not make two ids distinct");
        assert_eq!(error.id.as_str(), "shared");

        // And the converse: identical work under two ids is a legitimate batch.
        let a = leaf("a", "cheap");
        let mut b = leaf("b", "cheap");
        b.requirement = a.requirement.clone();
        b.plan = a.plan.clone();
        assert!(
            Leaves::new(vec![a, b]).is_ok(),
            "only the id decides whether two leaves collide"
        );
    }

    #[test]
    fn the_first_repetition_is_the_one_reported() {
        let error = Leaves::new(vec![
            leaf("a", "cheap"),
            leaf("b", "cheap"),
            leaf("b", "cheap"),
            leaf("a", "cheap"),
        ])
        .expect_err("two repetitions are still a rejection");
        assert_eq!(
            error.id.as_str(),
            "b",
            "the error names the first collision, in batch order"
        );
    }

    #[test]
    fn an_empty_batch_is_well_formed() {
        // A plan that asks for nothing is not an error, and a scheduler handed
        // one has nothing to dispatch. Rejecting it here would turn "no
        // capability applies to this diff" into a hard failure.
        let leaves = Leaves::new(Vec::new()).expect("an empty batch has no duplicates");
        assert!(leaves.is_empty());
        assert_eq!(leaves.len(), 0);
    }

    #[test]
    fn try_from_is_the_same_check() {
        assert!(Leaves::try_from(vec![leaf("a", "cheap")]).is_ok());
        assert_eq!(
            Leaves::try_from(vec![leaf("a", "cheap"), leaf("a", "cheap")])
                .expect_err("still rejected through TryFrom")
                .id
                .as_str(),
            "a"
        );
    }

    #[test]
    fn the_poll_helper_can_observe_a_future_that_is_not_ready() {
        // The floor for `a_deferred_dispatch_is_ready_without_being_driven`.
        // A `poll_once` that answered `Ready` unconditionally would make that
        // test assert nothing at all, so the helper is shown here failing to
        // resolve a future that genuinely blocks.
        assert!(
            poll_once(&BlockingScheduler, batch(&["a"])).is_pending(),
            "a dispatch that awaits cannot be resolved by one poll"
        );
    }

    #[test]
    fn a_deferred_dispatch_is_ready_without_being_driven() {
        // The documented rule on `Dispatch::Deferred`: the work has not started
        // and the job that collects its evidence does not exist, so the engine
        // must not block waiting. Asserted as the observable form of that — the
        // dispatch future resolves on its first poll, under a waker that can
        // never wake it and with no runtime to drive it.
        let Poll::Ready(dispatch) = poll_once(&DeferringScheduler, batch(&["a", "b"])) else {
            panic!("a deferring scheduler must not block its caller");
        };
        let Dispatch::Deferred { payload } = dispatch else {
            panic!("this scheduler defers");
        };
        assert_eq!(payload, "a,b");
    }

    #[test]
    fn deferred_names_no_leaf_that_ran() {
        // The distinction the two variants exist to carry. `Deferred` has no
        // `leaf_ids` field at all, so there is no route from a deferral to a
        // claim that anything produced evidence; the payload is opaque and
        // addressed to the external scheduler, not to the adjudicator.
        let Poll::Ready(Dispatch::Deferred { payload }) =
            poll_once(&DeferringScheduler, batch(&["only"]))
        else {
            panic!("this scheduler defers");
        };
        // Matching exhaustively is the point: a caller cannot accidentally read
        // a deferral as evidence, because the shape it would have to destructure
        // is not there.
        let dispatch = Dispatch::Deferred { payload };
        let accounted: Option<usize> = match &dispatch {
            Dispatch::Completed { leaf_ids, panicked } => Some(leaf_ids.len() + panicked.len()),
            Dispatch::Deferred { .. } => None,
        };
        assert_eq!(accounted, None, "a deferral accounts for no leaf");
    }

    #[test]
    fn completed_accounts_for_every_leaf_it_was_given() {
        let Poll::Ready(dispatch) = poll_once(&CompletingScheduler, batch(&["a", "b", "c"])) else {
            panic!("an in-place scheduler resolves without a runtime here");
        };
        let Dispatch::Completed { leaf_ids, panicked } = dispatch else {
            panic!("this scheduler completes in place");
        };
        assert_eq!(
            leaf_ids.iter().map(LeafId::as_str).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert!(panicked.is_empty());
    }

    #[test]
    fn the_two_variants_are_not_equal() {
        // They are distinguishable as values, not merely by their fields: a
        // scheduler that returned an empty `Completed` for work it had actually
        // deferred would be claiming three leaves ran and produced nothing.
        let completed = Dispatch::Completed {
            leaf_ids: Vec::new(),
            panicked: Vec::new(),
        };
        let deferred = Dispatch::Deferred {
            payload: String::new(),
        };
        assert_ne!(completed, deferred);
    }
}
