//! The workspace-wide guard on comparing two `ResolutionState`s.
//!
//! `core.capability_states` is keyed by a bare capability while resolution
//! happens per *(capability × scope)* requirement, so the writer of that map
//! must reduce several states to one. There is exactly one correct reduction —
//! `ResolutionState::collapse`, the `confidence_rank` minimum — and exactly one
//! plausible wrong one:
//!
//! ```text
//! let state = group.iter().copied().min().unwrap();
//! ```
//!
//! That line is shorter, reads like the same idea, and is a fail-open. A
//! derived ordering is *declaration* order — `Adopt`, `Run`, `Skip`,
//! `Unverified` — so its minimum is `Adopt`, and a capability adopted for one
//! crate and unverified for another would be written into a permanently frozen
//! bundle field as `adopt`: answered, when a scope went unanswered.
//!
//! So `ResolutionState` derives no `PartialOrd`/`Ord`. The wrong line does not
//! compile, in any crate, and `collapse` is the only way to combine two states.
//! That is a stronger guarantee than any scan of source text can give — but it
//! is one attribute wide. Restoring the derive is a one-word diff that makes a
//! compiler error go away, looks like an oversight being corrected, and silently
//! re-arms the fail-open. This file is what makes that word fail a test that
//! explains itself, the same way `no_evidence_from_status` guards an absence
//! that nothing else would notice.
//!
//! # Why not a ban on `min`/`max`/`sort` in the source text
//!
//! Because it cannot be written accurately. The bundle writer this guard
//! protects must sort `flag_ids` — `core.flag_ids` is documented "sorted" — in
//! the same file in which it builds `capability_states`, so a file-level ban on
//! ordering calls near `ResolutionState` reports the very file it exists to
//! protect, and the fix for that false positive is an allowlist entry that
//! disarms it. Meanwhile `group.iter().copied().min()` names no type at all, so
//! a textual scan cannot see the case that matters. The type checker can, and
//! does, as long as the derive stays off.
//!
//! Written as a text scan rather than over `syn`, because this crate has no
//! `syn` dependency and `vibe-check-model`'s dependency list is deliberately
//! tiny. If #80 lands a `syn`-based guard harness, this belongs in it: the
//! property is "the derive list of one item excludes two traits", which is an
//! easier question to ask of an AST than of a string.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use camino::{Utf8Path, Utf8PathBuf};

/// The traits that must never order a [`ResolutionState`].
const FORBIDDEN_TRAITS: [&str; 2] = ["Ord", "PartialOrd"];

/// The item whose derive list is the subject.
const GUARDED_ITEM: &str = "pub enum ResolutionState";

/// Drop comment lines, so this file's own prose — and `resolution.rs`'s, which
/// spells out the derive it refuses — is not read as code.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Collapse whitespace, so an attribute rustfmt wrapped across lines reads as
/// one.
fn normalized(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The traits derived on `item` in `source`, or `None` if the item is absent.
///
/// Takes the `#[derive(...)]` nearest *above* the item, which is where rustfmt
/// puts it and where the compiler requires it. Returning `None` for a missing
/// item rather than an empty list matters: an empty list would let this guard
/// pass by finding nothing, which is the failure mode every source scan in this
/// workspace is written to avoid.
fn derived_traits(source: &str, item: &str) -> Option<Vec<String>> {
    let code = normalized(&code_only(source));
    let item_at = code.find(item)?;
    let derive_at = code[..item_at].rfind("#[derive(")?;
    let open = derive_at + "#[derive(".len();
    let close = open + code[open..].find(')')?;
    Some(
        code[open..close]
            .split(',')
            .map(|trait_name| trait_name.trim().to_owned())
            .filter(|trait_name| !trait_name.is_empty())
            .collect(),
    )
}

/// Any hand-written ordering impl for `ResolutionState` in `source`.
///
/// The derive is not the only way back to a comparison: `impl Ord for
/// ResolutionState` reaches the same fail-open with more typing.
fn hand_written_ordering_impls(source: &str) -> Vec<String> {
    let code = normalized(&code_only(source));
    FORBIDDEN_TRAITS
        .iter()
        .flat_map(|trait_name| {
            [
                format!("impl {trait_name} for ResolutionState"),
                format!("impl {trait_name}<ResolutionState> for ResolutionState"),
            ]
        })
        .filter(|needle| code.contains(needle.as_str()))
        .collect()
}

/// Every library source in the workspace, as `(path, text)`.
///
/// The same walk as `no_evidence_from_status`: `crates/*/src` only. An impl in a
/// `tests/` directory is in a test binary and not in the crate anyone links
/// against, and this file's own samples would otherwise report themselves.
fn workspace_sources() -> Vec<(Utf8PathBuf, String)> {
    let root = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Utf8Path::parent)
        .expect("the model crate sits two levels below the workspace root")
        .to_owned();

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

/// Recursively collect `.rs` paths under `dir`, via camino for the same reason
/// the rest of the workspace uses it: a non-UTF-8 path is skipped by the walk
/// rather than lossily renamed.
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
fn a_resolution_state_derives_no_ordering() {
    let sources = workspace_sources();
    let (path, source) = sources
        .iter()
        .find(|(_, source)| source.contains(GUARDED_ITEM))
        .expect("`ResolutionState` is declared in a library source");

    let derived = derived_traits(source, GUARDED_ITEM).expect("the item has a derive list");

    for forbidden in FORBIDDEN_TRAITS {
        assert!(
            !derived.contains(&forbidden.to_owned()),
            "{path} derives `{forbidden}` on `ResolutionState`. A derived order is \
             declaration order, whose minimum is `Adopt` — so `group.iter().min()` \
             would write `adopt` into `core.capability_states` for a capability \
             whose other scope went unanswered, in the one part of the bundle that \
             can never be corrected. Combine states with `ResolutionState::collapse`, \
             which takes the `confidence_rank` minimum, and leave the derive off. \
             Derived: {derived:?}"
        );
    }
}

#[test]
fn nothing_hand_writes_an_ordering_for_a_resolution_state() {
    let offenders: Vec<String> = workspace_sources()
        .iter()
        .flat_map(|(path, source)| {
            hand_written_ordering_impls(source)
                .into_iter()
                .map(move |found| format!("{path}: {found}"))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "`ResolutionState` must have no ordering at all — the derive is refused \
         for a reason that a hand-written impl does not escape. Use \
         `ResolutionState::collapse`. Found: {offenders:?}"
    );
}

#[test]
fn the_scanner_actually_reads_the_derive_list() {
    // A positive control. Without it, a rename of the item or a change in how
    // rustfmt lays out attributes would turn both guards above into tests that
    // find nothing and pass.
    let sources = workspace_sources();
    let (_, source) = sources
        .iter()
        .find(|(_, source)| source.contains(GUARDED_ITEM))
        .expect("`ResolutionState` is declared in a library source");

    let derived = derived_traits(source, GUARDED_ITEM).expect("the item has a derive list");
    for expected in [
        "Clone",
        "Copy",
        "PartialEq",
        "Eq",
        "Serialize",
        "Deserialize",
    ] {
        assert!(
            derived.contains(&expected.to_owned()),
            "the scanner should see `{expected}` on `ResolutionState`; it read {derived:?}"
        );
    }
}

#[test]
fn the_scanner_would_catch_a_restored_derive() {
    let restored = "\
/// Which of the four states a requirement landed in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[serde(rename_all = \"kebab-case\")]
pub enum ResolutionState {
    Adopt,
}
";
    let derived = derived_traits(restored, GUARDED_ITEM).expect("the sample has a derive list");
    for forbidden in FORBIDDEN_TRAITS {
        assert!(
            derived.contains(&forbidden.to_owned()),
            "the scanner must see `{forbidden}` when it is restored; it read {derived:?}"
        );
    }
}

#[test]
fn the_scanner_would_catch_a_hand_written_impl() {
    let hand_written = "\
impl Ord for ResolutionState {
    fn cmp(&self, other: &Self) -> Ordering {
        self.confidence_rank().cmp(&other.confidence_rank())
    }
}
";
    assert_eq!(
        hand_written_ordering_impls(hand_written),
        vec!["impl Ord for ResolutionState".to_owned()]
    );
}

#[test]
fn the_scanner_reads_code_and_not_prose() {
    // `resolution.rs` and this file both spell out the forbidden derive in
    // order to explain it. A scanner that read comments would report the
    // sentences describing the rule as violations of it.
    let commented = "\
// #[derive(Ord)] would be a fail-open here.
#[derive(Clone, Copy)]
pub enum ResolutionState {
    Adopt,
}
";
    assert_eq!(
        derived_traits(commented, GUARDED_ITEM),
        Some(vec!["Clone".to_owned(), "Copy".to_owned()])
    );
    assert!(hand_written_ordering_impls("// impl Ord for ResolutionState").is_empty());
}

#[test]
fn a_missing_item_is_not_a_pass() {
    assert_eq!(derived_traits("pub struct Something;", GUARDED_ITEM), None);
}
