//! A subprocess runner that runs nothing.
//!
//! Capability logic is about turning a tool's output into a judgement. Testing
//! that should not require the tool to be installed — `cargo-mutants`, `miri`,
//! and `loom` are all things a contributor may not have, and a test suite that
//! silently changes behaviour depending on what is on `PATH` is worse than one
//! that skips.
//!
//! [`FakeExec`] answers from canned output and records the plans it was given,
//! so a test can also assert *how* a tool would have been invoked — which is
//! where the interesting mistakes live, such as forgetting to disable retries.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use vibe_check_host::exec::{Exec, ExecError, ProcessOutput, ProcessPlan};

/// Canned process output, keyed by program name.
#[derive(Debug, Default)]
pub struct FakeExec {
    responses: BTreeMap<String, ProcessOutput>,
    calls: Mutex<Vec<ProcessPlan>>,
}

impl FakeExec {
    /// A runner that knows about no programs.
    ///
    /// Running anything returns [`ExecError::NotFound`], which is the honest
    /// answer: a test that did not say what a tool outputs has not decided what
    /// the tool does.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register output for a program.
    #[must_use]
    pub fn with_output(mut self, program: &str, output: ProcessOutput) -> Self {
        self.responses.insert(program.to_owned(), output);
        self
    }

    /// Register a successful run producing `stdout`.
    #[must_use]
    pub fn with_success(self, program: &str, stdout: impl Into<Vec<u8>>) -> Self {
        self.with_output(
            program,
            ProcessOutput {
                exit_code: Some(0),
                stdout: stdout.into(),
                stderr: Vec::new(),
                duration_ms: 1,
                timed_out: false,
            },
        )
    }

    /// The plans that were dispatched, in order.
    ///
    /// # Panics
    /// Panics if the lock was poisoned by a panic in another test thread.
    #[must_use]
    pub fn calls(&self) -> Vec<ProcessPlan> {
        self.calls.lock().expect("call log lock").clone()
    }
}

#[async_trait]
impl Exec for FakeExec {
    async fn run(&self, plan: &ProcessPlan) -> Result<ProcessOutput, ExecError> {
        self.calls.lock().expect("call log lock").push(plan.clone());
        self.responses
            .get(&plan.program)
            .cloned()
            .ok_or_else(|| ExecError::NotFound {
                program: plan.program.clone(),
                detail: "no canned output registered for this program".into(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_canned_output() {
        let exec = FakeExec::new().with_success("cargo", b"ok".to_vec());
        let plan = ProcessPlan::new("cargo", ["test".to_owned()]);
        let out = exec.run(&plan).await.expect("run");
        assert!(out.succeeded());
        assert_eq!(out.stdout, b"ok");
    }

    #[tokio::test]
    async fn an_unregistered_program_is_an_error_not_a_silent_success() {
        // If this returned a zero exit code, a capability whose tool was never
        // configured would look like it passed.
        let exec = FakeExec::new();
        let err = exec
            .run(&ProcessPlan::new("miri", []))
            .await
            .expect_err("not found");
        assert!(matches!(err, ExecError::NotFound { .. }));
    }

    #[tokio::test]
    async fn records_how_the_tool_would_have_been_invoked() {
        // The assertion that catches a flake probe quietly leaving retries on.
        let exec = FakeExec::new().with_success("cargo", Vec::new());
        let plan = ProcessPlan::new("cargo", ["nextest".to_owned(), "run".to_owned()]);
        exec.run(&plan).await.expect("run");

        let calls = exec.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].args, ["nextest", "run"]);
        assert!(calls[0].determinism.retries_disabled);
    }
}
