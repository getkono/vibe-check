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
//! trusting a green tick. So the absence is asserted here, against the parsed
//! source of every crate, the same way `accumulator_invariants` asserts the
//! properties of the adjudicator that are likewise not expressible as types.
//!
//! This test lives in `vibe-check-model` because that is where `Evidence`'s
//! constructor monopoly is defined, but it deliberately reads the whole
//! workspace: the impl it is looking for would most naturally be written in
//! whichever crate learns about check runs, which is not this one.
//!
//! # Why `From` was never the rule
//!
//! The prohibition used to be spelled as a search for the eleven characters
//! `impl From<`, and every near-miss passed:
//!
//! ```ignore
//! impl TryFrom<CheckRun> for Evidence   // a different trait
//! impl Into<Evidence> for CheckRun      // the same conversion, reversed
//! impl From<CheckRun> for &Evidence     // a target the scan read as empty
//! fn evidence_from_check_run(…) -> Evidence  // no trait at all
//! ```
//!
//! Each of those is what someone writes when the obvious spelling does not
//! compile, so the guard asks the syntax tree instead. `From`, `TryFrom` and
//! `Into` are normalized into one direction; `&T`, `Box<T>`, `Option<T>` and
//! `Vec<T>` are reduced to the type they carry; and the rule the file always
//! *meant* — that nothing but the two sanctioned constructors produces an
//! `Evidence` or an `Artifact` — is finally stated outright.
//!
//! Types are matched by exact base identifier, never by substring. An
//! `EvidenceBundle` is not an `Evidence`, and a guard that could not tell them
//! apart would fail on a legitimate function that had never gone near a check
//! run.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::Conversion;

/// Types that must never be produced except by their sanctioned constructor.
///
/// `Evidence` because it may only be built by `Evidence::from_parsed`, which
/// takes a `ParsedEvidence`, which can only come from a parse that succeeded.
/// `Artifact` because it may only be built by `Artifact::new`, from bytes that
/// were actually downloaded and hashed. A second producer — a `From` impl, or
/// a free function — is a second constructor, and a second constructor is the
/// whole hole.
const FORBIDDEN_TARGETS: [&str; 2] = ["Evidence", "Artifact"];

/// Whether `function` is the one thing allowed to produce `produced`.
///
/// Matched on the function's *context*, not on its name alone, so that a free
/// `fn from_parsed() -> Evidence` written somewhere else is a failure rather
/// than a coincidence of naming.
///
/// `ForgeRead::download` is the third entry and the only one that is not an
/// associated function of the type it builds. It is the sole honest source of
/// an `Artifact`: it downloads the bytes, hashes them, and calls
/// `Artifact::new`. Sanctioned by *trait*, so the declaration and every
/// implementation of that one method are covered and nothing else is — an
/// inherent `download` on some other type inherits no exemption. That is what
/// lets the return-type reader see through `Result` everywhere else.
fn is_sanctioned(function: &common::Function, produced: &str) -> bool {
    match produced {
        "Evidence" => {
            function.owner.as_deref() == Some("Evidence") && function.name == "from_parsed"
        }
        "Artifact" => {
            (function.owner.as_deref() == Some("Artifact") && function.name == "new")
                || (function.owner_trait.as_deref() == Some("ForgeRead")
                    && function.name == "download")
        }
        _ => false,
    }
}

/// Types that must never appear as the *source* of a conversion.
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

/// Every conversion in the workspace's library sources, with its file.
fn workspace_conversions() -> Vec<(String, Conversion)> {
    let mut found = Vec::new();
    for (path, file) in common::workspace_sources() {
        for conversion in common::conversions(file.items()) {
            found.push((path.to_string(), conversion));
        }
    }
    found
}

#[test]
fn nothing_converts_into_evidence_or_an_artifact() {
    let offenders: Vec<String> = workspace_conversions()
        .into_iter()
        .filter(|(_, conversion)| FORBIDDEN_TARGETS.contains(&conversion.target.as_str()))
        .map(|(path, conversion)| format!("{path}: {}", conversion.rendered()))
        .collect();

    assert!(
        offenders.is_empty(),
        "nothing may convert into `Evidence` or `Artifact`:\n{}\n\
         \n\
         `Evidence` has exactly one constructor, `from_parsed`, which takes a \
         `ParsedEvidence` that can only come from a parse that succeeded. \
         `Artifact` has exactly one constructor, which takes the bytes that were \
         actually downloaded. A conversion is a second constructor, and a second \
         constructor is a path by which something that was never measured becomes \
         indistinguishable from something that was. `TryFrom` and `Into` are the \
         same conversion under other names, and `&Evidence` is the same type \
         behind a reference.\n\
         \n\
         If you need to record that a capability could not be answered, that is \
         what `UnverifiedReason` is for — and it is the only thing a failure is \
         allowed to become.",
        offenders.join("\n")
    );
}

#[test]
fn nothing_converts_out_of_a_check_run() {
    let offenders: Vec<String> = workspace_conversions()
        .into_iter()
        .filter(|(_, conversion)| FORBIDDEN_SOURCES.contains(&conversion.source.as_str()))
        .map(|(path, conversion)| format!("{path}: {}", conversion.rendered()))
        .collect();

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
fn nothing_but_the_sanctioned_constructor_produces_one() {
    // The rule the two tests above were always a proxy for. `From` was only ever
    // the most *likely* spelling of the violation; a plain
    //
    //     fn evidence_from_check_run(run: &CheckRun) -> Evidence
    //
    // is the identical laundering with no trait involved, and no scan for `impl
    // From<` would ever have seen it.
    //
    // `Result<_, _>` and `ForgeResult<_>` are unwrapped along with `Box`,
    // `Option` and the rest. Leaving them opaque was an earlier decision made
    // to protect `ForgeRead::download`, and it was wrong twice over: it protected
    // that one function by handing every other function a bypass, and the
    // bypass was the motivating example's own signature made fallible —
    //
    //     fn evidence_from_check_run(..) -> Result<Evidence, Infallible>
    //
    // — which is a laundering with an extra `Ok` around it. Nothing in the
    // workspace returns `Result<Evidence, _>`, so unwrapping costs nothing;
    // `download` is sanctioned by name and trait instead.
    let mut offenders = Vec::new();

    for (path, file) in common::workspace_sources() {
        for function in common::functions(file.items()) {
            let Some(produced) = function.returns.as_deref() else {
                continue;
            };
            if !FORBIDDEN_TARGETS.contains(&produced) {
                continue;
            }
            if !is_sanctioned(&function, produced) {
                offenders.push(format!("{path}: fn {} -> {produced}", function.path()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "only `Evidence::from_parsed`, `Artifact::new` and `ForgeRead::download` may produce one:\n{}\n\
         \n\
         Both constructors take the thing that makes the value honest — a \
         `ParsedEvidence` that could only come from a parse that succeeded, and \
         the bytes that were actually downloaded. Any other function returning \
         one is a place where that argument is not made, whether or not it wears \
         a trait's name.",
        offenders.join("\n")
    );
}

// --- anti-vacuity ----------------------------------------------------------
//
// The guarantees above are only as good as the reader beneath them, and a
// reader that silently matched nothing would pass all three forever. One
// meta-test per rule, each planting the violation that rule exists to catch.

#[test]
fn the_reader_normalizes_every_spelling_of_the_impl_that_matters() {
    // The four bypasses the text scanner had, planted together. Each must come
    // back as the same conversion: out of a check run, into evidence.
    let sample = common::read(
        "sample",
        r"
        /// There is no `impl From<CheckRun> for Evidence` anywhere.
        impl TryFrom<CheckRun> for Evidence {}
        impl Into<Evidence> for CheckRun {}
        impl From<CheckRun> for &Evidence {}
        impl<T: Debug> From<Vec<CheckRun>>
            for Box<Evidence>
        {}
        fn generic<T>(value: T) where T: From<u8> {}
        ",
    );
    let items = sample.items();
    let found = common::conversions(items);

    assert_eq!(
        found,
        vec![
            Conversion {
                via: "TryFrom".to_owned(),
                source: "CheckRun".to_owned(),
                target: "Evidence".to_owned(),
            },
            Conversion {
                via: "Into".to_owned(),
                source: "CheckRun".to_owned(),
                target: "Evidence".to_owned(),
            },
            Conversion {
                via: "From".to_owned(),
                source: "CheckRun".to_owned(),
                target: "Evidence".to_owned(),
            },
            Conversion {
                via: "From".to_owned(),
                source: "CheckRun".to_owned(),
                target: "Evidence".to_owned(),
            },
        ],
        "a different trait, a reversed direction, a reference target and a \
         wrapped pair are all the same conversion — and a `From` bound in a \
         `where` clause is not a conversion at all"
    );

    for conversion in &found {
        assert!(FORBIDDEN_TARGETS.contains(&conversion.target.as_str()));
        assert!(FORBIDDEN_SOURCES.contains(&conversion.source.as_str()));
    }
}

#[test]
fn the_reader_does_not_confuse_an_evidence_bundle_for_evidence() {
    // Exact base identifiers, never substrings. `EvidenceBundle` is a legitimate
    // public type and a conversion into one says nothing about check runs; a
    // guard that matched `Evidence` as a substring would fail on it, which is
    // the false positive that makes people delete guards.
    let sample = common::read(
        "sample",
        r"
        impl From<Parts> for EvidenceBundle {}
        impl From<Parts> for EvidenceFacts {}
        fn assemble(parts: Parts) -> EvidenceBundle { EvidenceBundle {} }
        ",
    );
    let items = sample.items();

    assert!(
        common::conversions(items)
            .iter()
            .all(|conversion| !FORBIDDEN_TARGETS.contains(&conversion.target.as_str())),
        "`EvidenceBundle` and `EvidenceFacts` are not `Evidence`"
    );
    assert!(
        common::functions(items)
            .iter()
            .all(|function| function.returns.as_deref() != Some("Evidence")),
        "and neither is a function that returns one"
    );
}

#[test]
fn the_reader_sees_a_free_function_that_launders_a_check_run() {
    // No trait, no impl, and every text scan in the tree's history blind to it.
    let sample = common::read(
        "sample",
        r"
        fn evidence_from_check_run(run: &CheckRun) -> Evidence { todo!() }
        impl Adoption {
            pub fn artifact_for(&self, run: &WorkflowRun) -> Option<Artifact> { None }
        }
        ",
    );
    let items = sample.items();
    let producers: Vec<String> = common::functions(items)
        .into_iter()
        .filter(|function| {
            function
                .returns
                .as_deref()
                .is_some_and(|name| FORBIDDEN_TARGETS.contains(&name))
        })
        .map(|function| function.path())
        .collect();

    assert_eq!(
        producers,
        ["evidence_from_check_run", "Adoption::artifact_for"],
        "a free function and a method behind an `Option` both produce one, and \
         neither is `Evidence::from_parsed` or `Artifact::new`"
    );
}

#[test]
fn a_fallible_producer_is_still_a_producer() {
    // The bypass that unwrapping `Result` closes: the motivating example's own
    // signature, made fallible. An `Ok(evidence)` is an evidence.
    let sample = common::read(
        "sample",
        r"
        pub fn evidence_from_check_run(
            _conclusion: &str,
        ) -> Result<Evidence, core::convert::Infallible> {
            unimplemented!()
        }
        ",
    );
    let read = common::functions(sample.items());

    assert_eq!(
        read.iter()
            .filter(|function| function.returns.as_deref() == Some("Evidence"))
            .map(|function| function.name.clone())
            .collect::<Vec<_>>(),
        ["evidence_from_check_run"],
        "`Result<Evidence, _>` produces an `Evidence`"
    );
    assert!(
        !is_sanctioned(&read[0], "Evidence"),
        "and nothing about being fallible sanctions it"
    );
}

#[test]
fn only_the_forge_read_trait_may_download_an_artifact() {
    // The one function that legitimately returns an `Artifact` behind a
    // `Result`, and the reason the sanction is written against the trait rather
    // than against the name: an inherent `download` on some other type is not
    // `ForgeRead::download` and must not inherit its exemption.
    let sample = common::read(
        "sample",
        r"
        trait ForgeRead {
            async fn download(&self, meta: &ArtifactMeta) -> ForgeResult<Artifact>;
        }
        impl ForgeRead for NullForge {
            async fn download(&self, _meta: &ArtifactMeta) -> ForgeResult<Artifact> {
                unimplemented!()
            }
        }
        impl Cache {
            pub fn download(&self) -> ForgeResult<Artifact> {
                unimplemented!()
            }
        }
        ",
    );
    let read = common::functions(sample.items());

    assert_eq!(
        read.iter()
            .filter(|function| function.returns.as_deref() == Some("Artifact"))
            .count(),
        3,
        "all three produce an `Artifact` once the `Result` is seen through"
    );
    assert!(is_sanctioned(&read[0], "Artifact"), "the declaration");
    assert!(
        is_sanctioned(&read[1], "Artifact"),
        "and its implementation"
    );
    assert!(
        !is_sanctioned(&read[2], "Artifact"),
        "but an inherent `Cache::download` is not `ForgeRead::download`"
    );
}

#[test]
fn the_reader_covers_more_than_the_top_level_of_a_file() {
    // A conversion written inside an inline module is in the crate anyone links
    // against, and a reader that only looked at a file's top-level items would
    // miss it — while a `#[cfg(test)]` module is not linked, and must be skipped
    // however deep it sits.
    let sample = common::read(
        "sample",
        r"
        mod inner {
            impl From<CheckRun> for Evidence {}
        }
        #[cfg(test)]
        mod tests {
            impl From<CheckRun> for Evidence {}
        }
        ",
    );
    let items = sample.items();

    assert_eq!(
        common::conversions(items).len(),
        1,
        "the one in `inner`, and not the fixture in `tests`"
    );
}

#[test]
fn a_cfg_not_test_item_is_read_and_a_cfg_test_item_is_not() {
    // The blind spot a naive "does the `cfg` mention `test`" check had. The
    // negation ships in every build anyone links against, and skipping it
    // hid a whole `impl` from every guard in this file.
    let sample = common::read(
        "sample",
        r#"
        #[cfg(not(test))]
        impl From<CheckRun> for Evidence {}
        #[cfg(test)]
        impl From<CheckRun> for Artifact {}
        #[cfg(all(test, feature = "e2e"))]
        impl From<CheckRun> for Adoption {}
        #[cfg(feature = "e2e")]
        impl From<CheckRun> for Coverage {}
        #[cfg(any(test, unix))]
        impl From<CheckRun> for Report {}
        "#,
    );
    let targets: Vec<String> = common::conversions(sample.items())
        .into_iter()
        .map(|conversion| conversion.target)
        .collect();

    assert_eq!(
        targets,
        ["Evidence", "Coverage", "Report"],
        "`not(test)` ships, `test` and `all(test, …)` do not, and a predicate \
         this reader cannot evaluate is assumed to ship rather than assumed \
         away"
    );
}

#[test]
fn an_impl_inside_a_const_block_is_still_registered() {
    // `const _: () = { … };` is the idiom for a scoped impl. The compiler
    // registers it globally; a walk that only descended into modules never saw
    // it. The `fn` form is here too, even though rustc's
    // `non_local_definitions` lint discourages it — a lint is not a guard.
    let sample = common::read(
        "sample",
        r"
        const _: () = {
            impl From<CheckRun> for Evidence {}
        };
        static _WIRE: () = {
            impl From<WorkflowRun> for Evidence {}
        };
        fn wire_up() {
            impl From<CheckRequest> for Evidence {}
        }
        ",
    );
    let sources: Vec<String> = common::conversions(sample.items())
        .into_iter()
        .map(|conversion| conversion.source)
        .collect();

    assert_eq!(
        sources,
        ["CheckRun", "WorkflowRun", "CheckRequest"],
        "an item body is a place items live, not only a place expressions do"
    );
}

#[test]
fn an_impl_written_inside_a_macro_is_read() {
    // `syn` hands a `macro_rules!` body back as opaque tokens, so a naive parse
    // is a *regression* here: the text scan this file replaced could see the
    // impl, because the impl is written in the source even though it is not
    // an item yet. One `impl From<$name> for Evidence` in a macro invoked
    // eleven times is eleven forbidden impls.
    let sample = common::read(
        "sample",
        r"
        macro_rules! id_newtype {
            ($(#[$meta:meta])* $name:ident) => {
                $(#[$meta])*
                pub struct $name(SmolStr);

                impl From<$name> for Evidence {
                    fn from(_value: $name) -> Self {
                        unimplemented!()
                    }
                }
            };
        }
        ",
    );
    let found = common::conversions(sample.items());

    assert_eq!(found.len(), 1, "the impl inside the macro body");
    assert_eq!(found[0].target, "Evidence");
    assert_eq!(
        found[0].source, "metavar_name",
        "the metavariable stands in for every type the macro is invoked for"
    );
}

#[test]
#[should_panic(expected = "could not be re-parsed")]
fn a_macro_body_this_reader_cannot_parse_is_a_loud_failure() {
    // Re-parsing macro expansions is only worth anything if a body it cannot
    // handle stops the guard rather than slipping past it — a silent skip is
    // exactly the regression the macro fix exists to undo, reintroduced through
    // the fix's own escape hatch. This is what proves the panic is reachable.
    let _ = common::read("sample", "macro_rules! partial { ($t:ty) => { impl }; }\n");
}
