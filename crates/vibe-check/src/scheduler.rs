//! Running planned work on this machine.
//!
//! The local strategy: run every leaf here, concurrently, and return the results
//! immediately. This is what `cargo vibe-check` uses, and what
//! `--scheduler local` forces in CI for anyone who would rather use one large
//! runner than a fan-out of small ones.
//!
//! The CI strategy — emitting a job matrix and collecting evidence from
//! artifacts later — is a different implementation of the same trait. What
//! matters is that the code which actually invokes a tool is shared: the two
//! differ in *when and where* work happens, never in *what runs*, so a
//! capability cannot behave one way locally and another way in CI.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::task::JoinSet;
use vibe_check_host::exec::Exec;
use vibe_check_host::scheduler::{Dispatch, Leaf, Scheduler};
use vibe_check_model::LeafId;

/// Runs leaves on this machine.
pub struct LocalScheduler {
    exec: Arc<dyn Exec>,
    concurrency: usize,
}

impl LocalScheduler {
    /// A scheduler using `exec`, with a default concurrency of half the
    /// available parallelism.
    ///
    /// Half rather than all: the tools being run are themselves parallel
    /// (`cargo` spawns per-crate jobs), so saturating the machine at this level
    /// mostly produces contention. One is the floor.
    #[must_use]
    pub fn new(exec: Arc<dyn Exec>) -> Self {
        let concurrency = std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1));
        Self { exec, concurrency }
    }

    /// Override the concurrency limit.
    #[must_use]
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }
}

#[async_trait]
impl Scheduler for LocalScheduler {
    async fn dispatch(&self, leaves: Vec<Leaf>) -> Dispatch {
        let mut set = JoinSet::new();
        let mut queue = leaves.into_iter();
        let mut completed = Vec::new();
        let mut panicked = Vec::new();

        // Keep at most `concurrency` in flight, starting a new one each time one
        // finishes, rather than spawning everything and letting the executor
        // sort it out — these are subprocesses, and oversubscribing them thrashes.
        for leaf in queue.by_ref().take(self.concurrency) {
            spawn_leaf(&mut set, Arc::clone(&self.exec), leaf);
        }
        while let Some(joined) = set.join_next().await {
            // Every task returns its own identifier, panic or not, so the only
            // `Err` left here is a cancelled task — and nothing in this function
            // cancels one. Dropping it silently is what this used to do to a
            // panic, and the whole point is that a leaf is never lost without
            // being named.
            match joined {
                Ok((id, Outcome::Ran)) => completed.push(id),
                Ok((id, Outcome::Panicked)) => panicked.push(id),
                Err(error) => tracing::error!(%error, "a leaf task did not return an identifier"),
            }
            if let Some(leaf) = queue.next() {
                spawn_leaf(&mut set, Arc::clone(&self.exec), leaf);
            }
        }

        // Sorted, because completion order depends on how long each tool
        // happened to take, and nothing downstream may depend on that.
        completed.sort();
        panicked.sort();
        Dispatch::Completed {
            leaf_ids: completed,
            panicked,
        }
    }
}

/// Whether a leaf's task finished or came apart.
///
/// A `bool` would be two states with no names, and the caller sorts the
/// identifier into one of two lists on the strength of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    /// The task finished, whatever the tool itself decided.
    Ran,
    /// The task panicked and produced no evidence.
    Panicked,
}

fn spawn_leaf(set: &mut JoinSet<(LeafId, Outcome)>, exec: Arc<dyn Exec>, leaf: Leaf) {
    set.spawn(async move {
        // The identifier is taken before the work, so that a panic still has
        // something to be attributed to. Recovering it from `JoinError` instead
        // would need the task's own id threaded back through a side table.
        let id = leaf.id.clone();
        match crate::panic::caught(run_leaf(exec, leaf)).await {
            Ok(()) => (id, Outcome::Ran),
            Err(_) => {
                // `error!`, not `warn!`: a tool that fails is ordinary, but a
                // panic inside our own harness is a defect in vibe-check, and
                // the leaf it happened under is the only clue to which
                // capability lost its evidence.
                tracing::error!(leaf = %id, "leaf panicked; its evidence is lost");
                (id, Outcome::Panicked)
            }
        }
    });
}

/// Run one leaf and log what happened.
async fn run_leaf(exec: Arc<dyn Exec>, leaf: Leaf) {
    // A tool that fails to run is not an error here. It becomes an
    // unverified capability further up, which escalates. Returning early
    // would abandon the other leaves and lose evidence we already have.
    match exec.run(&leaf.plan).await {
        Ok(output) => tracing::debug!(
            leaf = %leaf.id,
            exit = ?output.exit_code,
            timed_out = output.timed_out,
            "leaf finished"
        ),
        Err(error) => tracing::warn!(leaf = %leaf.id, %error, "leaf could not run"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibe_check_host::exec::{ExecError, ProcessOutput, ProcessPlan};
    use vibe_check_model::{LaneId, RequirementId};
    use vibe_check_testkit::FakeExec;

    /// An `Exec` that panics for one program and succeeds for everything else.
    ///
    /// A panicking `Exec` rather than a panicking leaf, because a panic in the
    /// port is the shape this actually takes in production: a parser meeting
    /// output it did not expect, or an index into a slice that was shorter than
    /// the tool's documentation promised.
    struct PanickingExec {
        /// The program whose invocation panics.
        on: &'static str,
    }

    #[async_trait]
    impl Exec for PanickingExec {
        async fn run(&self, plan: &ProcessPlan) -> Result<ProcessOutput, ExecError> {
            assert_ne!(plan.program, self.on, "a deliberate panic inside a leaf");
            Ok(ProcessOutput {
                exit_code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
                duration_ms: 0,
                timed_out: false,
            })
        }
    }

    fn leaf(id: &str, program: &str) -> Leaf {
        Leaf {
            id: LeafId::new_checked(id).expect("a well-formed fixture leaf id"),
            requirement: RequirementId::new(format!("req_{id}")),
            plan: ProcessPlan::new(program, []),
            lane: LaneId::new("cheap"),
        }
    }

    /// Leaf identifiers as plain strings, for assertions.
    ///
    /// `LeafId` has no `PartialEq<&str>`, deliberately — comparing an unchecked
    /// string to a checked one is the comparison the type exists to make
    /// awkward.
    fn ids(leaf_ids: &[LeafId]) -> Vec<&str> {
        leaf_ids.iter().map(LeafId::as_str).collect()
    }

    #[tokio::test]
    async fn runs_every_leaf() {
        let exec = Arc::new(FakeExec::new().with_success("cargo", Vec::new()));
        let scheduler = LocalScheduler::new(Arc::clone(&exec) as Arc<dyn Exec>);
        let dispatch = scheduler
            .dispatch(vec![
                leaf("a", "cargo"),
                leaf("b", "cargo"),
                leaf("c", "cargo"),
            ])
            .await;
        let Dispatch::Completed { leaf_ids, panicked } = dispatch else {
            panic!("local scheduling completes in place");
        };
        assert!(ids(&panicked).is_empty(), "nothing here panicked");
        assert_eq!(ids(&leaf_ids), ["a", "b", "c"]);
        assert_eq!(exec.calls().len(), 3);
    }

    #[tokio::test]
    async fn results_are_ordered_independently_of_completion() {
        let exec = Arc::new(FakeExec::new().with_success("cargo", Vec::new()));
        let scheduler = LocalScheduler::new(Arc::clone(&exec) as Arc<dyn Exec>);
        let dispatch = scheduler
            .dispatch(vec![
                leaf("z", "cargo"),
                leaf("m", "cargo"),
                leaf("a", "cargo"),
            ])
            .await;
        let Dispatch::Completed { leaf_ids, panicked } = dispatch else {
            panic!("local scheduling completes in place");
        };
        assert!(ids(&panicked).is_empty(), "nothing here panicked");
        // Sorted, not in completion order: a verdict must not depend on which
        // tool happened to finish first.
        assert_eq!(ids(&leaf_ids), ["a", "m", "z"]);
    }

    #[tokio::test]
    async fn a_failing_tool_does_not_abandon_the_others() {
        // `miri` has no canned output, so running it errors. The other leaves
        // must still complete — their evidence is real and losing it would
        // escalate capabilities that were perfectly well answered.
        let exec = Arc::new(FakeExec::new().with_success("cargo", Vec::new()));
        let scheduler = LocalScheduler::new(Arc::clone(&exec) as Arc<dyn Exec>);
        let dispatch = scheduler
            .dispatch(vec![
                leaf("good", "cargo"),
                leaf("missing", "miri"),
                leaf("also-good", "cargo"),
            ])
            .await;
        let Dispatch::Completed { leaf_ids, panicked } = dispatch else {
            panic!("local scheduling completes in place");
        };
        assert!(ids(&panicked).is_empty(), "nothing here panicked");
        assert_eq!(ids(&leaf_ids), ["also-good", "good", "missing"]);
    }

    #[tokio::test]
    async fn respects_the_concurrency_limit() {
        let exec = Arc::new(FakeExec::new().with_success("cargo", Vec::new()));
        let scheduler = LocalScheduler::new(Arc::clone(&exec) as Arc<dyn Exec>).with_concurrency(1);
        let leaves: Vec<_> = (0..5).map(|i| leaf(&format!("l{i}"), "cargo")).collect();
        let dispatch = scheduler.dispatch(leaves).await;
        let Dispatch::Completed { leaf_ids, panicked } = dispatch else {
            panic!("local scheduling completes in place");
        };
        assert!(ids(&panicked).is_empty(), "nothing here panicked");
        assert_eq!(leaf_ids.len(), 5);
    }

    #[tokio::test]
    async fn concurrency_is_never_zero() {
        // A zero limit would take zero leaves from the queue and hang forever.
        let exec = Arc::new(FakeExec::new().with_success("cargo", Vec::new()));
        let scheduler = LocalScheduler::new(Arc::clone(&exec) as Arc<dyn Exec>).with_concurrency(0);
        let dispatch = scheduler.dispatch(vec![leaf("only", "cargo")]).await;
        let Dispatch::Completed { leaf_ids, panicked } = dispatch else {
            panic!("local scheduling completes in place");
        };
        assert!(ids(&panicked).is_empty(), "nothing here panicked");
        assert_eq!(ids(&leaf_ids), ["only"]);
    }

    #[tokio::test]
    async fn a_panicking_leaf_is_named_rather_than_dropped() {
        // The hole this closes: a panicking task used to yield `Err(JoinError)`
        // and be discarded, so the leaf appeared in no list at all and was
        // indistinguishable from one that was never scheduled.
        let exec = Arc::new(PanickingExec { on: "boom" });
        let scheduler = LocalScheduler::new(exec as Arc<dyn Exec>);
        let dispatch = scheduler
            .dispatch(vec![
                leaf("good", "cargo"),
                leaf("bad", "boom"),
                leaf("also-good", "cargo"),
            ])
            .await;
        let Dispatch::Completed { leaf_ids, panicked } = dispatch else {
            panic!("local scheduling completes in place");
        };

        assert_eq!(ids(&panicked), ["bad"]);
        // And the other two still ran: a panic in one leaf must not abandon
        // evidence that was already paid for.
        assert_eq!(ids(&leaf_ids), ["also-good", "good"]);
    }

    #[tokio::test]
    async fn every_dispatched_leaf_is_accounted_for_exactly_once() {
        // The property the two lists exist to provide. Their union is the whole
        // batch and they do not overlap, so a caller can attribute every gap
        // rather than inferring one from a missing identifier.
        let exec = Arc::new(PanickingExec { on: "boom" });
        let scheduler = LocalScheduler::new(exec as Arc<dyn Exec>).with_concurrency(1);
        let leaves = vec![
            leaf("a", "cargo"),
            leaf("b", "boom"),
            leaf("c", "cargo"),
            leaf("d", "boom"),
        ];
        let dispatch = scheduler.dispatch(leaves).await;
        let Dispatch::Completed { leaf_ids, panicked } = dispatch else {
            panic!("local scheduling completes in place");
        };

        assert_eq!(ids(&leaf_ids), ["a", "c"]);
        assert_eq!(ids(&panicked), ["b", "d"]);
        let mut all = ids(&leaf_ids);
        all.extend(ids(&panicked));
        all.sort_unstable();
        assert_eq!(
            all,
            ["a", "b", "c", "d"],
            "nothing dispatched is unaccounted for"
        );
    }
}
