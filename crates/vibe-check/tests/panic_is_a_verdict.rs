//! A panic is a verdict, and its exit code is `1`.
//!
//! Every part of this is only provable in a real process. `catch_unwind`,
//! `set_hook`, and `std::process::exit` are all process-global, and the number a
//! CI pipeline actually branches on is the one the operating system reports —
//! not the `u8` a unit test can inspect. So this spawns the binary.
//!
//! The three numbers under test:
//!
//! - **not `101`**, which is what an escaping panic used to produce and which
//!   the table in `exit.rs` has never described;
//! - **not `0`**, because a crash that reads as a clean bill of health is the
//!   exact failure mode vibe-check exists to prevent elsewhere;
//! - **`1`**, the reserved "we could not produce a verdict" code.
//!
//! And the bundle beside them, because the exit code alone says only that
//! something went wrong. The comment and the check run are what gate a merge in
//! `mode: enforcing`, and they are rendered from this document.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::{Command, Output};

/// The environment variable that asks `run` to panic. Spelled out rather than
/// imported, so that renaming the constant without thinking breaks this test
/// instead of silently turning it into an assertion about a normal run.
const PANIC_HATCH: &str = "VIBE_CHECK_PANIC";

/// Run the binary once, with the panic hatch either set or absent.
fn run(panic: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vibe-check"));
    command.arg("classify");
    if panic {
        command.env(PANIC_HATCH, "1");
    } else {
        command.env_remove(PANIC_HATCH);
    }
    command.output().expect("the binary under test runs")
}

#[test]
fn a_panic_exits_one_and_not_a_hundred_and_one() {
    let output = run(true);
    let code = output.status.code();

    assert_ne!(
        code,
        Some(101),
        "101 is the default panic handler's code and is outside the exit-code \
         contract entirely; a pipeline branching on that contract cannot read it"
    );
    assert_ne!(
        code,
        Some(0),
        "a crash that exits 0 is an outage wearing a clean bill of health"
    );
    assert_eq!(
        code,
        Some(1),
        "a panic is `we could not produce a verdict`, which is exit 1\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_panic_still_emits_a_bundle_and_the_verdict_is_human() {
    let output = run(true);
    let stdout = String::from_utf8(output.stdout).expect("the bundle is UTF-8");
    let bundle: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("stdout is one JSON document, got {error}: {stdout}"));

    assert_eq!(
        bundle.pointer("/core/tier"),
        Some(&serde_json::json!("t2")),
        "a crash checked nothing, so nothing about the change is evidenced"
    );
    assert_eq!(
        bundle.pointer("/core/verdict"),
        Some(&serde_json::json!("human")),
        "the verdict a crash carries — which is not in conflict with exit 1, \
         because the two answer different questions"
    );
    assert_eq!(
        bundle.pointer("/schema_version"),
        Some(&serde_json::json!(1)),
        "a bundle no reader can parse is not a verdict"
    );
    assert_eq!(
        bundle.pointer("/generator/name"),
        Some(&serde_json::json!("vibe-check"))
    );
}

#[test]
fn the_bundle_carries_exactly_one_internal_panic_escalation_at_t2() {
    let output = run(true);
    let stdout = String::from_utf8(output.stdout).expect("the bundle is UTF-8");
    let bundle: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON document");

    let escalations = bundle
        .pointer("/adjudication/escalations")
        .and_then(serde_json::Value::as_array)
        .expect("the adjudication carries its escalations");
    assert_eq!(
        escalations.len(),
        1,
        "one cause, and nothing invented alongside it: {escalations:#?}"
    );

    let escalation = &escalations[0];
    assert_eq!(
        escalation.get("reason"),
        Some(&serde_json::json!("internal-panic"))
    );
    assert_eq!(escalation.get("to"), Some(&serde_json::json!("t2")));
    assert_eq!(
        escalation.get("from"),
        Some(&serde_json::json!("t0")),
        "the ledger must replay: it started at the bottom of the lattice"
    );

    let detail = escalation
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .expect("an escalation explains itself");
    assert!(
        detail.contains("panic"),
        "the detail must say what happened: {detail}"
    );
    assert!(
        detail.contains(".rs:"),
        "and where, so the crash is diagnosable from the bundle alone: {detail}"
    );
}

#[test]
fn the_panic_is_also_reported_on_stderr() {
    // Stdout is the machine-readable document, so the human-readable report has
    // to go somewhere else or one of the two audiences is left with nothing.
    let stderr = String::from_utf8(run(true).stderr).expect("diagnostics are UTF-8");
    assert!(
        stderr.contains("vibe-check panicked"),
        "the panic must be reported to a human too: {stderr}"
    );
}

#[test]
fn an_ordinary_failure_is_not_dressed_up_as_a_panic() {
    // The control. Without the hatch this command exits 1 as well — it is not
    // implemented yet — but through the error path, which writes no bundle. If
    // this ever started emitting one, the tests above would be passing on a
    // bundle that had nothing to do with a panic.
    let output = run(false);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "an unimplemented command writes no bundle: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("vibe-check panicked"),
        "and it did not panic"
    );
}
