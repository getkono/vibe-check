//! Test doubles and fixture builders.
//!
//! vibe-check's input is a git diff and its output is a verdict, which makes it
//! awkward to test unless the awkward parts are built once and shared. This
//! crate is that shared scaffolding:
//!
//! - [`TestRepo`] materializes a real git repository from plain files, with
//!   **reproducible commit hashes**, so a golden file can contain a real hash.
//! - [`FakeForge`] answers reads from canned values and records writes, so tests
//!   assert on what vibe-check *decided to do* without touching a network.
//! - [`FakeExec`] returns canned tool output and records the plans it was given,
//!   so capability logic is testable without `miri` or `cargo-mutants` installed.
//!
//! None of this exercises real HTTP behaviour — authentication, pagination,
//! retry, rate limiting. Those need a real client against a local server, and
//! are a separate layer.

// Unlike the other crates, the ban on `unwrap`/`expect`/`panic` is lifted for
// this crate's *library* code, not just its tests. Everything here is test
// scaffolding used only as a dev-dependency: a fixture that cannot build its
// repository, or a poisoned lock in a test harness, means the test environment
// is broken. Failing loudly at that point is the useful behaviour — threading a
// `Result` out to every call site would make every assertion noisier in order to
// handle a case that only ever means "something else already panicked".
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod exec;
pub mod forge;
pub mod repo;

pub use exec::FakeExec;
pub use forge::{FakeForge, Mutation};
pub use repo::TestRepo;
