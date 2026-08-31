//! The workspace-wide guard on the karet feature surface.
//!
//! `Cargo.toml` states the invariant in prose: `default-features = false` on
//! `karet-vcs` is "load-bearing, not tidiness", because `karet-vcs` defaults to
//! `["signature"]`, which enables `dep:ssh-key` — and *"a dependency that parses
//! key material has no business in a process that runs untrusted pull-request
//! code."* The same sentence carries a second prohibition: never enable the
//! `view` feature on any karet crate, because it pulls `ratatui`, a terminal UI
//! into a process that has no terminal.
//!
//! Like `no_evidence_from_status.rs`, the invariant is implemented as an
//! **absence**, and an absence is invisible. Nothing fails when someone deletes
//! `default-features = false` to get at `karet_vcs::signature`, or adds
//! `features = ["view"]` to reuse a diff renderer. The build still succeeds, the
//! diff reads as ergonomics, and the dependency arrives silently. So the absence
//! is asserted here rather than verified by hand once, which is all that had
//! ever happened to it.
//!
//! ## Why `Cargo.lock`, and not `cargo metadata`
//!
//! `Cargo.lock` is **feature-accurate**, which is the whole reason this works.
//! Cargo performs feature resolution when it writes the lock, so an optional
//! dependency that no activated feature turns on never enters it. The proof is
//! in the file itself: `karet-vcs`'s entry lists exactly `gix`, `imara-diff`,
//! `karet-core` and `thiserror` — and *not* `ssh-key`, which is precisely the
//! `dep:ssh-key` that `default-features = false` is switching off. If the lock
//! were a feature-independent union of every optional dependency, `ssh-key`
//! would be sitting in it today.
//!
//! The lock is also guaranteed current at the moment this test runs: `cargo
//! test` resolves dependencies and updates `Cargo.lock` during the build phase,
//! before any test binary is executed. A stale lock cannot be read from here,
//! because a lock that disagreed with `Cargo.toml` would have been rewritten
//! before this code existed as a binary.
//!
//! Reading a file also keeps this test hermetic and network-free, which
//! `mise run test` requires — it runs on the `hk` pre-push hook.
//!
//! ## Why the whole workspace
//!
//! `Cargo.toml`'s wording names `cargo tree -p vibe-check-diff`, but the reason
//! it gives is about "a process that runs untrusted pull-request code", and that
//! process is the `vibe-check` binary — of which `vibe-check-diff` is only one
//! link-time input. A key parser reaching the binary through any other member is
//! the same supply-chain fact. So the scope here is the workspace, matching
//! `no_evidence_from_status.rs`, which likewise lives in one crate and
//! deliberately scans all of them.
//!
//! ## The one accepted blind spot
//!
//! A `[workspace.dependencies]` entry carrying `features = ["view"]` that **no
//! crate actually consumes** does not enter the lock, and is not caught here.
//! That is correct rather than a gap: an unused declaration links nothing and
//! ships nothing. The moment a crate adds it to its own `[dependencies]`, the
//! transitive package enters the lock and this test fails.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use camino::{Utf8Path, Utf8PathBuf};

/// Package names banned by an exact match.
///
/// `ssh-key` is the crate `karet-vcs`'s default `signature` feature pulls in via
/// `dep:ssh-key`. Exact, because the name is the crate: there is no family of
/// sibling crates to miss.
const BANNED_EXACT: [&str; 1] = ["ssh-key"];

/// Package names banned by a **prefix** match.
///
/// The prefix is deliberate and is the detail that would silently rot under an
/// exact-name ban. ratatui 0.30 is split across `ratatui`, `ratatui-core`,
/// `ratatui-widgets` and `ratatui-macros`; a crate can depend on a sibling
/// directly, so banning only the literal name `ratatui` would let
/// `ratatui-widgets` through while the terminal UI it exists to draw arrives
/// anyway.
///
/// The cost of the prefix is a hypothetical unrelated crate whose name starts
/// with `ratatui`, which would be a false positive. That trade is right: a false
/// positive is a five-minute conversation, and a false negative is a terminal UI
/// linked into a headless CI process.
const BANNED_PREFIX: [&str; 1] = ["ratatui"];

/// The smallest believable number of packages in a resolved lock.
///
/// The workspace resolves 236 today, through `gix` alone. A lock that parsed to
/// fewer than this is not a lock this test read correctly, and the ban below
/// would be passing on an empty set rather than on evidence.
const MINIMUM_PACKAGES: usize = 100;

/// Whether a resolved package name is one this workspace must never link.
fn is_banned(name: &str) -> bool {
    BANNED_EXACT.contains(&name) || BANNED_PREFIX.iter().any(|banned| name.starts_with(banned))
}

/// Every package name in a `Cargo.lock`, in the order the lock lists them.
///
/// The lock is `version = 4` and contains nothing but `[[package]]` tables, so
/// splitting on the table header and taking the first `name = "…"` of each block
/// is sufficient. Taking the *first* matters: a block's `dependencies` array
/// also holds quoted crate names, and those are not package declarations.
fn package_names(lock: &str) -> Vec<String> {
    lock.split("[[package]]")
        .skip(1)
        .filter_map(|block| {
            block.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("name = \"")
                    .and_then(|rest| rest.strip_suffix('"'))
                    .map(str::to_owned)
            })
        })
        .collect()
}

/// The workspace root, two levels above this crate's manifest.
fn workspace_root() -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Utf8Path::parent)
        .expect("the model crate sits two levels below the workspace root")
        .to_owned()
}

/// The resolved lockfile, guaranteed current by `cargo test`'s build phase.
fn lockfile() -> String {
    let path = workspace_root().join("Cargo.lock");
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "the workspace lockfile at {path} must be readable: {error}\n\
             \n\
             Without it this test asserts nothing at all. If the workspace layout \
             moved, fix the path — do not delete the test."
        )
    })
}

/// The name of every directory under `crates/`, i.e. every workspace member.
///
/// Derived from the filesystem rather than hardcoded, so that adding a crate
/// strengthens the floor assertion below instead of going unnoticed.
///
/// Via camino's `read_dir_utf8`, because `clippy.toml` disallows
/// `std::fs::read_dir`: directory order is filesystem-dependent. The caller
/// sorts, so a failure names members in the same order on every machine.
fn member_names() -> Vec<String> {
    let crates = workspace_root().join("crates");
    let entries = crates.read_dir_utf8().unwrap_or_else(|error| {
        panic!("the workspace has a crates directory at {crates}: {error}")
    });

    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn the_lockfile_is_actually_being_read() {
    // The floor. A lockfile-scanning ban rots into a no-op the instant the
    // parser returns an empty set — every name it looks for is absent from
    // nothing — so the ban below is only worth as much as this assertion.
    let resolved = package_names(&lockfile());
    let members = member_names();

    assert!(
        resolved.len() >= MINIMUM_PACKAGES,
        "parsed only {} packages out of the workspace lockfile, expected at least \
         {MINIMUM_PACKAGES}\n\
         \n\
         The ban in this file is an absence check. If the parser sees nothing, \
         the ban passes for the wrong reason and keeps passing forever. Fix the \
         parser or the path before trusting any other test in this file.",
        resolved.len()
    );

    assert!(
        !members.is_empty(),
        "found no directories under crates/ — the workspace layout moved and this \
         test is comparing two empty sets"
    );

    let missing: Vec<&String> = members
        .iter()
        .filter(|member| !resolved.iter().any(|package| package == *member))
        .collect();

    assert!(
        missing.is_empty(),
        "these workspace members are missing from the parsed lockfile: {missing:?}\n\
         \n\
         Every directory under crates/ is a workspace member and must therefore \
         appear as a package in Cargo.lock. If one does not, this file is reading \
         the wrong artefact or parsing it wrongly, and its ban proves nothing."
    );
}

#[test]
fn no_key_parser_or_terminal_ui_is_linked() {
    let mut offenders: Vec<String> = package_names(&lockfile())
        .into_iter()
        .filter(|name| is_banned(name))
        .collect();
    offenders.sort();
    offenders.dedup();

    assert!(
        offenders.is_empty(),
        "these packages must never be linked into this workspace: {offenders:?}\n\
         \n\
         `karet-vcs` defaults to `[\"signature\"]`, which enables `dep:ssh-key`. \
         We never verify a commit signature, and a dependency that parses key \
         material has no business in a process that runs untrusted pull-request \
         code. The other vector is `karet-diff`'s `view` feature, which pulls \
         ratatui — a terminal UI, in a headless CI process that has no terminal.\n\
         \n\
         Neither arrives by accident: something enabled a feature. The two \
         remedies are to restore `default-features = false` on `karet-vcs` in the \
         workspace `Cargo.toml`, and to remove any `features = [\"view\"]` from a \
         karet dependency. Do not widen this list to make the build pass."
    );
}

#[test]
fn the_parser_actually_finds_packages() {
    // A literal fragment in the shape the real lock uses, so that a parser which
    // silently returned nothing could not pass the ban above.
    let fragment = r#"
version = 4

[[package]]
name = "gix"
version = "0.74.0"
dependencies = [
 "gix-actor",
 "thiserror",
]

[[package]]
name = "karet-vcs"
version = "0.6.0"
dependencies = [
 "gix",
 "imara-diff",
]
"#;

    assert_eq!(
        package_names(fragment),
        vec!["gix".to_owned(), "karet-vcs".to_owned()],
        "the parser must take one name per [[package]] block and must not mistake \
         a quoted entry in a `dependencies` array for a package declaration"
    );
}

#[test]
fn the_parser_would_catch_the_dependency_that_matters() {
    // The two changes this file exists to prevent, run end to end through the
    // parser and the ban predicate, so the assertion above is demonstrably
    // reachable rather than vacuous. Committed, network-free, and always run.
    let signature_feature = "[[package]]\nname = \"ssh-key\"\nversion = \"0.6.7\"\n";
    let view_feature = "[[package]]\nname = \"ratatui-core\"\nversion = \"0.1.0\"\n";

    for fragment in [signature_feature, view_feature] {
        let names = package_names(fragment);
        assert_eq!(names.len(), 1, "the parser sees the forbidden package");
        assert!(
            is_banned(&names[0]),
            "{} must be banned — it is the package the invariant is about",
            names[0]
        );
    }

    assert!(
        is_banned("ratatui-widgets"),
        "the ratatui ban matches by prefix, so a direct dependency on a 0.30 \
         sibling crate cannot slip past an exact-name comparison"
    );
    assert!(
        !is_banned("karet-vcs"),
        "the ban must not fire on the dependency it is protecting"
    );
}
