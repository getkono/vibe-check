//! The model crate may not read a clock, and the absence is asserted here.
//!
//! `DecisionTime` is built so that the only instant a decision can depend on is
//! a commit's committer date: no `now()`, no `Default`, no `From<Timestamp>`.
//! That argument holds only while nothing *else* in this crate reads the
//! current moment — a single `Timestamp::now()` somewhere in `resolution.rs`
//! would make the type a formality, because a caller could then get a fresh
//! instant without ever naming one.
//!
//! `clippy.toml` bans `Timestamp::now`, `Zoned::now`, `SystemTime::now` and
//! `Instant::now` workspace-wide, so this test is not the first line of
//! defence. It is the second, and it guards a different thing: the lint checks
//! resolved paths against a list, so it is silent about a clock reached through
//! a dependency nobody listed, an `#[allow]` added in the same diff, or a method
//! named `now` on some future type of our own. This test does not care how the
//! moment is obtained — it asserts that the *text* of this crate never calls
//! anything called `now`.
//!
//! It is vacuous today, deliberately. The model crate contains no such call and
//! none of its six dependencies would tempt one. It ships as a prospective
//! guard, in the style of `no_evidence_from_status`: the change it exists to
//! stop is one that nothing else fails on, that makes the type checker no
//! unhappier, and that reads in review as a small convenience.
//!
//! Scanning text rather than parsing it is a deliberate divergence from the
//! `syn`-based guards being written elsewhere in this crate's `tests`. A parser
//! sees calls; the claim here is broader than calls — a `fn now(` *declared* in
//! this crate is as much a violation as one invoked, since it hands every
//! caller the shortcut the type refuses to provide.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use camino::{Utf8Path, Utf8PathBuf};

/// The 1-based line numbers on which `now(` appears in code.
///
/// Comment lines are skipped, and skipped rather than deleted so the numbers
/// reported are the ones in the file. Skipping them is required, not cosmetic:
/// this crate's own prose argues about `now()` at length — `time.rs` names the
/// method several times explaining why it is absent — so a scanner over raw
/// text would report the sentences that state the rule as violations of it.
///
/// Line comments only, matching `no_evidence_from_status`. The workspace uses
/// no block comments, and `clippy.toml` plus review are what keep it that way.
fn wall_clock_lines(source: &str) -> Vec<usize> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//") && contains_now_call(line))
        .map(|(index, _)| index + 1)
        .collect()
}

/// Whether a line calls or declares something named `now`.
fn contains_now_call(line: &str) -> bool {
    let mut cursor = 0usize;
    while let Some(offset) = line[cursor..].find("now(") {
        let start = cursor + offset;
        cursor = start + "now(".len();

        let preceded_by_identifier = line[..start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        if !preceded_by_identifier {
            return true;
        }
    }
    false
}

/// Every `.rs` file under this crate's `src`, in a stable order.
///
/// Library sources only. This file lives in `tests/`, so it is outside its own
/// scan by construction — which is what lets the samples below spell out the
/// call they forbid in order to prove the scanner sees it.
fn model_sources() -> Vec<(Utf8PathBuf, String)> {
    let src = Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    let mut files = Vec::new();
    collect_rs(&src, &mut files);
    files.sort();

    assert!(
        files.len() > 5,
        "expected the model crate's sources under {src}, found {} — if the \
         layout moved, this test is scanning nothing and proving nothing",
        files.len()
    );

    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("a listed source file is readable");
            (path, text)
        })
        .collect()
}

/// Recursively collect `.rs` paths under `dir`.
///
/// Via camino, so a source file whose path is not UTF-8 is skipped loudly by
/// the walk rather than lossily renamed — the same reason the workspace bans
/// `std::path` outright. Directory order is filesystem-dependent, which is why
/// `read_dir` is disallowed elsewhere; the caller sorts so that a failure names
/// files in the same order on every machine.
fn collect_rs(dir: &Utf8Path, out: &mut Vec<Utf8PathBuf>) {
    let Ok(entries) = dir.read_dir_utf8() else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(path, out);
        } else if path.extension() == Some("rs") {
            out.push(path.to_owned());
        }
    }
}

#[test]
fn the_model_crate_never_reads_a_clock() {
    let mut offenders = Vec::new();

    for (path, source) in model_sources() {
        for line in wall_clock_lines(&source) {
            offenders.push(format!("{path}:{line}"));
        }
    }

    assert!(
        offenders.is_empty(),
        "the model crate may not read the current moment:\n{}\n\
         \n\
         Time-dependent decisions compare against the head commit's committer \
         date, carried as `DecisionTime`, so that re-evaluating last month's \
         pull request gives the verdict it had. A verdict that changes because \
         of when it was asked cannot be replayed, cannot be audited, and cannot \
         be appealed.\n\
         \n\
         The wall clock is read in exactly one place in this workspace: \
         `vibe-check-host`'s `clock` module, whose every output is display-only \
         and on the digest's exclusion list. If a decision here needs a time, it \
         needs a `DecisionTime` threaded in from the caller.",
        offenders.join("\n")
    );
}

#[test]
fn the_scanner_sees_the_call_that_matters() {
    // The specific change this file exists to prevent, run through the scanner
    // to prove the assertion above is reachable rather than vacuous.
    let sample = "let at = jiff::Timestamp::now();";

    assert_eq!(wall_clock_lines(sample), vec![1]);
}

#[test]
fn the_scanner_ignores_prose_and_lookalike_identifiers() {
    let sample = "\
/// There is deliberately no `now()` on this type.
// let evaded = Timestamp::now();
fn snow(depth: u8) {}
let known = knowns.get(key);
struct Now(Timestamp);
";

    assert!(
        wall_clock_lines(sample).is_empty(),
        "a doc comment arguing about `now()`, a commented-out call, and \
         identifiers that merely end in those letters are not clock reads"
    );
}

#[test]
fn the_scanner_sees_a_declaration_as_well_as_a_call() {
    // Broader than the lint on purpose: a `now` this crate defines hands every
    // caller the shortcut `DecisionTime` refuses to provide, and `clippy.toml`
    // cannot ban a path that does not exist yet.
    let sample = "\
impl DecisionTime {
    pub fn now() -> Self {
        Self(Timestamp::now())
    }
}
";

    assert_eq!(
        wall_clock_lines(sample),
        vec![2, 3],
        "both the declaration and the call inside it are violations"
    );
}
