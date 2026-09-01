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
//!
//! # This test requires debug assertions
//!
//! The panic is triggered through `VIBE_CHECK_PANIC`, which is compiled in
//! under `#[cfg(debug_assertions)]` and out of anything shipped. `cargo test`
//! builds under the `test` profile, which inherits `dev`, and
//! `CARGO_BIN_EXE_vibe-check` builds the binary under that same profile — so
//! the hatch is here. Under `cargo test --release` it is not, and these tests
//! fail with a bundle-less exit 1. That is the tradeoff spelled out on
//! `PANIC_HATCH`, and the gate (`mise run test`) does not pass `--release`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::{Command, Output};

/// The environment variable that asks `run` to panic. Spelled out rather than
/// imported, so that renaming the constant without thinking breaks this test
/// instead of silently turning it into an assertion about a normal run.
const PANIC_HATCH: &str = "VIBE_CHECK_PANIC";

/// Which of the two binaries to spawn.
///
/// Both are under test, and the shim is the one that had a hole: it reads the
/// command line itself, so it can fail in ways `vibe-check` cannot. A test
/// suite that only ever spawned `vibe-check` would keep saying "both binaries
/// call the same guarded function" while one of them crashed before reaching
/// it.
#[derive(Clone, Copy)]
enum Binary {
    /// `vibe-check`, invoked directly.
    Direct,
    /// `cargo-vibe-check`, invoked the way cargo invokes it — with the
    /// subcommand name back in `argv[1]`.
    CargoShim,
}

impl Binary {
    /// A command for this binary, with cargo's inserted argument where the shim
    /// would see it.
    fn command(self) -> Command {
        match self {
            Self::Direct => Command::new(env!("CARGO_BIN_EXE_vibe-check")),
            Self::CargoShim => {
                let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-vibe-check"));
                command.arg("vibe-check");
                command
            }
        }
    }
}

/// Run `vibe-check` once, with the panic hatch either set or absent.
fn run(panic: bool) -> Output {
    run_as(Binary::Direct, panic, &[])
}

/// Run either binary once, with the panic hatch either set or absent.
fn run_as(binary: Binary, panic: bool, extra: &[&std::ffi::OsStr]) -> Output {
    let mut command = binary.command();
    command.args(extra);
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

#[test]
fn the_cargo_shim_takes_the_same_path() {
    // `cargo vibe-check` is the same invocation through a different entry
    // point, and the claim this branch makes is that both binaries leave
    // through one guarded function. Asserted rather than assumed: only the
    // shim strips an argument, so only the shim can get that wrong.
    let output = run_as(Binary::CargoShim, true, &[]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).expect("the bundle is UTF-8");
    let bundle: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON document");
    assert_eq!(
        bundle.pointer("/core/verdict"),
        Some(&serde_json::json!("human"))
    );
    assert_eq!(
        bundle.pointer("/adjudication/escalations/0/reason"),
        Some(&serde_json::json!("internal-panic"))
    );
}

#[test]
#[cfg(unix)]
fn a_non_utf8_argument_is_rejected_rather_than_crashing() {
    // The hole this test exists to close. `std::env::args()` panics on a
    // non-UTF-8 argument, and it is read *before* the guard is entered — so
    // the panic escaped as 101 with no bundle, in the one binary no test
    // spawned. Both must now report a usage error instead.
    use std::os::unix::ffi::OsStrExt;

    let invalid = std::ffi::OsStr::from_bytes(b"--base=\xff");
    for (name, binary) in [
        ("vibe-check", Binary::Direct),
        ("cargo-vibe-check", Binary::CargoShim),
    ] {
        let output = run_as(binary, false, &[invalid]);
        let code = output.status.code();

        assert_ne!(
            code,
            Some(101),
            "{name}: a malformed command line must not crash the process"
        );
        assert_eq!(
            code,
            Some(2),
            "{name}: a command line clap cannot read is a usage error\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stdout.is_empty(),
            "{name}: a usage error writes no bundle"
        );
    }
}

#[test]
#[cfg(unix)]
fn a_panic_that_cannot_write_its_report_still_exits_one() {
    // The regression this guards is worth spelling out, because it is invisible
    // from every other test here: `color_eyre`'s own panic hook is a single
    // `eprintln!`, and `eprintln!` panics when the write fails. A panic raised
    // *inside* a panic hook is not catchable — the runtime prints "thread
    // panicked while processing panic. aborting." and calls `abort_internal`,
    // so `catch_unwind` never sees it, no bundle is written, and the shell
    // reports 134. That is a code the table in `exit.rs` has never described,
    // arriving from the one path whose entire job is to be legible.
    //
    // `/dev/full` is the cheapest honest full disk: every write returns ENOSPC.
    // Redirecting stderr to it is exactly a crash on a machine that has run out
    // of room for logs, which is when a crash is most likely in the first place.
    for (name, binary) in [
        ("vibe-check", Binary::Direct),
        ("cargo-vibe-check", Binary::CargoShim),
    ] {
        let Ok(full) = std::fs::OpenOptions::new().write(true).open("/dev/full") else {
            // Not every unix has it (containers with a minimal /dev, some BSDs).
            // Skipping is right: the alternative is a gate that fails for a
            // reason that has nothing to do with the code under test.
            eprintln!("skipping: /dev/full is not available");
            return;
        };

        let mut command = binary.command();
        command.arg("classify");
        command.env(PANIC_HATCH, "1");
        command.stdout(std::process::Stdio::null());
        command.stderr(std::process::Stdio::from(full));

        let status = command.status().expect("the binary under test runs");
        assert_ne!(
            status.code(),
            None,
            "{name}: the process was killed by a signal rather than exiting; a \
             signal is outside every exit table, and an unwritable stderr must \
             not cause one"
        );
        assert_eq!(
            status.code(),
            Some(1),
            "{name}: losing the crash report costs the diagnostic, not the \
             exit code"
        );
    }
}

#[test]
fn the_bundle_carries_no_digest_it_did_not_compute() {
    // `bundle_id` and `verdict_digest` are digests, and a crash computed
    // neither. They must be absent in a way a consumer can *see* is absent:
    // a store keyed on `bundle_id` and a replay recomputing `verdict_digest`
    // both need to tell "no digest" from "a digest that did not match".
    let stdout = String::from_utf8(run(true).stdout).expect("the bundle is UTF-8");
    let bundle: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON document");

    for field in ["bundle_id", "verdict_digest"] {
        let value = bundle
            .pointer(&format!("/core/{field}"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("core.{field} is present and a string"));
        assert!(
            !value.starts_with("blake3:"),
            "core.{field} must not look like a digest this run did not compute: {value}"
        );
        assert_ne!(
            value, "unknown",
            "core.{field} is a digest, not an identity field, so it must not \
             carry the identity sentinel"
        );
    }
}
