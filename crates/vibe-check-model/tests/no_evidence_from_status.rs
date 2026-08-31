//! The workspace-wide guard on where evidence can come from.
//!
//! Two of the strongest claims in this codebase are made in prose, in two
//! different crates, and mean the same thing:
//!
//! - `evidence.rs`: "There is no `From<CheckRun> for Evidence` anywhere in the
//!   workspace, and adding one would be the single most damaging change someone
//!   could make to this codebase."
//! - `forge.rs`: "Adding `impl From<CheckRun> for Artifact` would be the single
//!   most damaging change available in this workspace. There is no such impl,
//!   and there must never be one."
//!
//! They are the load-bearing half of "unparseable means unverified". A check run
//! is a *name and a colour*: `conclusion: success` says a job someone configured
//! reported success, and says nothing whatsoever about what was measured. The
//! whole adoption design refuses to treat that as evidence — and the refusal is
//! implemented as an absence, because `Artifact` cannot be built without bytes
//! and `Evidence` cannot be built except from a successful parse.
//!
//! An absence is invisible. Nothing fails when someone adds the missing
//! conversion; the type checker is *happier* afterwards, the diff looks like a
//! small ergonomic improvement, and every adopted capability quietly starts
//! trusting a green tick. So the absence is asserted here, against the source
//! text of every crate, the same way `accumulator_invariants` asserts the
//! properties of the adjudicator that are likewise not expressible as types.
//!
//! This test lives in `vibe-check-model` because that is where `Evidence`'s
//! constructor monopoly is defined, but it deliberately scans the whole
//! workspace: the impl it is looking for would most naturally be written in
//! whichever crate learns about check runs, which is not this one.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use camino::{Utf8Path, Utf8PathBuf};

/// Types that must never appear as the target of a `From` impl.
///
/// `Evidence` because it may only be built by `Evidence::from_parsed`, which
/// takes a `ParsedEvidence`, which can only come from a parse that succeeded.
/// `Artifact` because it may only be built from bytes that were actually
/// downloaded and hashed. A `From` impl targeting either is a second
/// constructor, and a second constructor is the whole hole.
const FORBIDDEN_TARGETS: [&str; 2] = ["Evidence", "Artifact"];

/// Types that must never appear as the *source* of a `From` impl.
///
/// Listed separately from the targets because the danger is not symmetric: a
/// conversion *out of* a check run is suspect wherever it lands, since the only
/// honest thing to do with one is read its name for a diagnostic.
///
/// `WorkflowRun` and `RunStatus` are here for the same reason and are not
/// covered by the target list above. A workflow run carries a `CheckConclusion`
/// and a lifecycle status, so a conversion out of one launders the same colour
/// through a different type — and it need not target `Evidence` or `Artifact` to
/// do damage. `impl From<WorkflowRun> for Adoption`, or for anything else that
/// downstream code treats as an answer, is the same mistake wearing the name of
/// the type that made the filters writable.
const FORBIDDEN_SOURCES: [&str; 5] = [
    "CheckRun",
    "CheckConclusion",
    "CheckRequest",
    "WorkflowRun",
    "RunStatus",
];

/// One `impl From<Source> for Target` found in the source text.
#[derive(PartialEq, Eq, Debug)]
struct FromImpl {
    source: String,
    target: String,
}

/// Drop comment lines.
///
/// Required, not cosmetic: the two doc comments quoted at the top of this file
/// both spell out the exact impl they forbid, so a scanner over raw text would
/// report the sentences explaining the rule as violations of it.
///
/// Line comments only, matching `accumulator_invariants`. The workspace uses no
/// block comments, and `clippy.toml` plus review are what keep it that way.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Collapse whitespace, so an impl rustfmt wrapped across lines reads as one.
fn normalized(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `From<` at `at` is the trait of an `impl`, rather than a bound.
///
/// `where T: From<u8>` and `impl From<u8> for T` both contain `From<`, and only
/// the second declares a conversion. Accepts both `impl From<` and the
/// generic `impl<T> From<` forms.
fn is_impl_position(code: &str, at: usize) -> bool {
    let before = code[..at].trim_end();
    if before.ends_with("impl") {
        return true;
    }
    // `impl<T> From<…>`: step back over the generic parameter list.
    let Some(without_close) = before.strip_suffix('>') else {
        return false;
    };
    let mut depth = 1usize;
    for (index, character) in without_close.char_indices().rev() {
        match character {
            '>' => depth += 1,
            '<' => {
                depth -= 1;
                if depth == 0 {
                    return without_close[..index].trim_end().ends_with("impl");
                }
            }
            _ => {}
        }
    }
    false
}

/// The end of the angle-bracketed argument beginning at `open`.
fn matching_angle(code: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in code[open..].char_indices() {
        match character {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every `impl From<_> for _` in a source file.
fn from_impls(source: &str) -> Vec<FromImpl> {
    let code = normalized(&code_only(source));
    let mut found = Vec::new();
    let mut cursor = 0usize;

    while let Some(offset) = code[cursor..].find("From<") {
        let start = cursor + offset;
        cursor = start + "From<".len();

        if !is_impl_position(&code, start) {
            continue;
        }
        let open = start + "From".len();
        let Some(close) = matching_angle(&code, open) else {
            continue;
        };
        let Some(rest) = code[close + 1..].strip_prefix(" for ") else {
            continue;
        };

        // The target runs to the first token that cannot be part of a path.
        let target: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':' || *c == '$')
            .collect();

        found.push(FromImpl {
            source: code[open + 1..close].trim().to_owned(),
            target,
        });
    }

    found
}

/// Every `.rs` file under `crates/*/src`, in a stable order.
fn workspace_sources() -> Vec<(Utf8PathBuf, String)> {
    let root = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Utf8Path::parent)
        .expect("the model crate sits two levels below the workspace root")
        .to_owned();

    // Library sources only. An impl written in a `tests/` directory exists in a
    // test binary and not in the crate anyone links against — and this file's own
    // samples, which spell out the forbidden impl in order to prove the scanner
    // sees it, would otherwise report themselves.
    let mut crate_dirs = Vec::new();
    collect_dirs(&root.join("crates"), &mut crate_dirs);
    crate_dirs.sort();

    let mut files = Vec::new();
    for crate_dir in &crate_dirs {
        collect_rs(&crate_dir.join("src"), &mut files);
    }
    files.sort();

    assert!(
        crate_dirs.len() >= 3 && files.len() > 10,
        "expected the workspace crates under {root}, found {} crates and {} source \
         files — if the layout moved, this test is scanning nothing and proving \
         nothing",
        crate_dirs.len(),
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

/// The immediate subdirectories of `dir`, i.e. the workspace's crates.
fn collect_dirs(dir: &Utf8Path, out: &mut Vec<Utf8PathBuf>) {
    let Ok(entries) = dir.read_dir_utf8() else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            out.push(entry.path().to_owned());
        }
    }
}

/// Recursively collect `.rs` paths under `dir`.
///
/// Via camino, so a source file whose path is not UTF-8 is skipped loudly by
/// the walk rather than lossily renamed — the same reason the workspace bans
/// `std::path` outright.
///
/// Directory order is filesystem-dependent, which is why `read_dir` is
/// disallowed elsewhere. It does not reach a verdict from here, but the caller
/// sorts anyway so that a failure names files in the same order on every
/// machine.
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
fn nothing_converts_into_evidence_or_an_artifact() {
    let mut offenders = Vec::new();

    for (path, source) in workspace_sources() {
        for found in from_impls(&source) {
            if FORBIDDEN_TARGETS.contains(&found.target.as_str()) {
                offenders.push(format!(
                    "{}: impl From<{}> for {}",
                    path, found.source, found.target
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "nothing may convert into `Evidence` or `Artifact`:\n{}\n\
         \n\
         `Evidence` has exactly one constructor, `from_parsed`, which takes a \
         `ParsedEvidence` that can only come from a parse that succeeded. \
         `Artifact` has exactly one constructor, which takes the bytes that were \
         actually downloaded. A `From` impl is a second constructor, and a second \
         constructor is a path by which something that was never measured becomes \
         indistinguishable from something that was.\n\
         \n\
         If you need to record that a capability could not be answered, that is \
         what `UnverifiedReason` is for — and it is the only thing a failure is \
         allowed to become.",
        offenders.join("\n")
    );
}

#[test]
fn nothing_converts_out_of_a_check_run() {
    let mut offenders = Vec::new();

    for (path, source) in workspace_sources() {
        for found in from_impls(&source) {
            if FORBIDDEN_SOURCES
                .iter()
                .any(|forbidden| found.source.contains(forbidden))
            {
                offenders.push(format!(
                    "{}: impl From<{}> for {}",
                    path, found.source, found.target
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a check run or workflow run may not be converted into anything:\n{}\n\
         \n\
         A `conclusion: success` means a job someone configured reported success. \
         It does not say which tests ran, whether the binary compiled, or whether \
         a path filter skipped the job entirely — and `Skipped` renders as a green \
         tick under most branch-protection settings. A `WorkflowRun` is the same \
         colour with provenance attached: it says which run produced bytes, never \
         that the run measured what its name suggests. Reading either for a \
         diagnostic, or filtering on one, is fine; converting one is a way to \
         launder a colour into a measurement.",
        offenders.join("\n")
    );
}

#[test]
fn the_scanner_actually_finds_impls() {
    // These guarantees are only as good as the parsing above, and a scanner that
    // silently matched nothing would pass both tests here forever.
    let sample = r"
        /// There is no `impl From<CheckRun> for Evidence` anywhere.
        impl From<ParseError> for UnverifiedReason {}
        impl<T: Debug> From<Vec<T>>
            for EvidenceFacts
        {}
        fn generic<T>(value: T) where T: From<u8> {}
    ";

    let found = from_impls(sample);

    assert_eq!(
        found,
        vec![
            FromImpl {
                source: "ParseError".to_owned(),
                target: "UnverifiedReason".to_owned(),
            },
            FromImpl {
                source: "Vec<T>".to_owned(),
                target: "EvidenceFacts".to_owned(),
            },
        ],
        "the scanner must see through rustfmt line wrapping and generic \
         parameters, must ignore a `From` bound in a `where` clause, and must \
         ignore the doc comment naming the very impl it forbids"
    );
}

#[test]
fn the_scanner_would_catch_the_impl_that_matters() {
    // The specific change this file exists to prevent, run through the scanner
    // to prove the assertion above is reachable rather than vacuous.
    let sample = "impl From<CheckRun> for Evidence {}";
    let found = from_impls(sample);

    assert_eq!(found.len(), 1, "the scanner sees the forbidden impl");
    assert!(FORBIDDEN_TARGETS.contains(&found[0].target.as_str()));
    assert!(FORBIDDEN_SOURCES.contains(&found[0].source.as_str()));
}
