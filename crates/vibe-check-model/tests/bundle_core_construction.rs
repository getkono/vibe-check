//! The workspace-wide guard on where a `BundleCore` comes from.
//!
//! `BundleCore` is the one part of the bundle that can never change. Its
//! `tier` must come from the enforced ledger and its `advisory_tier` from the
//! advisory one, and `BundleCore::new` is what makes that true: it takes the two
//! as distinct types, so they cannot be transposed.
//!
//! That argument holds only while `new` is the *only* way a `BundleCore` is
//! built. A second construction site — a struct literal in the crate that
//! assembles the bundle, most likely, where both tiers are in scope at once — is
//! a place where the wrong one can be written into `tier`, and no test of `new`
//! would notice. Nothing in the type system objects, because every field is
//! `pub` and has to be: readers need them.
//!
//! So the constraint is asserted against the parsed source of the whole
//! workspace, the way `accumulator_invariants` and `no_evidence_from_status`
//! assert the other properties that are not expressible as types.
//!
//! Separate from `accumulator_invariants` on purpose: that file is about the
//! adjudicator's shape, this one is about the bundle's, and they fail for
//! unrelated reasons.
//!
//! # Parsed, not scanned
//!
//! This guard used to carry its own copy of a brace matcher, a string-literal
//! skipper and a `#[cfg(test)]` exciser — some seven helpers, duplicated
//! verbatim into `accumulator_invariants.rs` because integration tests are
//! separate binaries. All of it existed to answer one question the compiler
//! answers for free: is this `Name {` a struct literal, a struct declaration, an
//! `impl` header, or a return type followed by a function body? `syn` is asked
//! instead, and the shared readers now live in `tests/common/mod.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::Literal;

/// Every `BundleCore` literal in the workspace's library sources, with its file.
fn construction_sites() -> Vec<(String, Literal)> {
    let mut sites = Vec::new();
    for (path, file) in common::workspace_sources() {
        for literal in common::struct_literals(&file) {
            if literal.type_name == "BundleCore" {
                sites.push((path.to_string(), literal));
            }
        }
    }
    sites
}

#[test]
fn a_bundle_core_is_constructed_in_exactly_one_place() {
    let sites = construction_sites();

    assert_eq!(
        sites.len(),
        1,
        "a `BundleCore` may be built in exactly one place, found: {sites:#?}\n\
         \n\
         `BundleCore::new` takes the enforced and advisory ledgers as distinct \
         types so that `tier` and `advisory_tier` cannot be transposed. A second \
         struct literal — most likely in the crate that assembles the bundle, \
         where both tiers are in scope at once — is a place where the wrong one \
         can be written into the one field that can never be corrected."
    );

    let (path, literal) = &sites[0];
    assert!(
        path.ends_with("vibe-check-model/src/bundle.rs"),
        "the sole construction site must be in `bundle.rs`, found {path}"
    );
    assert_eq!(
        literal.function, "new",
        "the sole construction site must be `BundleCore::new`"
    );
}

#[test]
fn nothing_else_can_construct_one_under_another_name() {
    // `Self { .. }` inside any `impl` *for* `BundleCore` is the same
    // construction written in a way a search for `BundleCore {` cannot see,
    // because it mentions neither the type nor `impl BundleCore {`.
    //
    // Trait impls are the shape that matters most here:
    //
    //     impl Default for BundleCore {
    //         fn default() -> Self { Self { tier: Tier::T0, .. } }
    //     }
    //
    // is a second construction site, and a `T0`-shaped one — the fail-open the
    // lattice exists to prevent, arriving through the one type whose meaning can
    // never be corrected afterwards.
    //
    // The parser resolves `Self` to the enclosing `impl`'s type, so such a site
    // would already have failed the count above. This test says *why* it failed,
    // which is the difference between a fix and a workaround.
    let disguised: Vec<String> = construction_sites()
        .into_iter()
        .filter(|(_, literal)| literal.written_as_self)
        .map(|(path, literal)| format!("{path}: fn {}", literal.function))
        .collect();

    assert!(
        disguised.is_empty(),
        "an `impl` for `BundleCore` must not build one with a `Self {{` literal: \
         {disguised:#?}\n\
         \n\
         Write the type out by name so the construction site reads as one — or, \
         better, call `BundleCore::new`, which is the only place the enforced \
         and advisory tiers are known to be the right way round."
    );

    let impls: usize = common::workspace_sources()
        .iter()
        .map(|(_, file)| common::impls_for(file.items(), "BundleCore").len())
        .sum();
    assert_eq!(
        impls, 1,
        "sanity check: exactly the one inherent `impl BundleCore` should exist"
    );
}

// --- anti-vacuity ----------------------------------------------------------

#[test]
fn the_reader_tells_a_literal_from_a_declaration_or_a_return_type() {
    // The three false positives a text scan for `BundleCore {` had to special-
    // case by hand, and the one true positive it had to keep. `-> BundleCore {`
    // is a return type followed by the function's own opening brace; missing it
    // would make every function returning one report itself.
    let sample = common::read(
        "sample",
        r"
pub struct BundleCore {
    pub tier: Tier,
}
impl BundleCore {
    pub fn new() -> Self {
        BundleCore { tier }
    }
}
fn elsewhere() -> BundleCore {
    BundleCore { tier }
}
",
    );
    let found: Vec<Literal> = common::struct_literals(&sample)
        .into_iter()
        .filter(|literal| literal.type_name == "BundleCore")
        .collect();

    assert_eq!(
        found,
        vec![
            Literal {
                type_name: "BundleCore".to_owned(),
                function: "new".to_owned(),
                written_as_self: false,
            },
            Literal {
                type_name: "BundleCore".to_owned(),
                function: "elsewhere".to_owned(),
                written_as_self: false,
            },
        ],
        "both literals with the function each sits in, and none of the \
         declaration, the impl header, or `elsewhere`'s return type"
    );
}

#[test]
fn the_reader_resolves_a_self_literal_to_the_impl_it_sits_in() {
    // The construction that names the type nowhere. `impl Default for
    // BundleCore` is the shape that matters, and it is also the shape a scan for
    // `Self {` alone could not attribute to any particular type — the third impl
    // here builds a `SomethingElse` with identical text.
    let sample = common::read(
        "sample",
        r"
impl BundleCore {
    fn new() -> Self { BundleCore { tier } }
}
impl Default for BundleCore {
    fn default() -> Self { Self { tier } }
}
impl Default for SomethingElse {
    fn default() -> Self { Self { tier } }
}
",
    );
    let found: Vec<Literal> = common::struct_literals(&sample)
        .into_iter()
        .filter(|literal| literal.type_name == "BundleCore")
        .collect();

    assert_eq!(found.len(), 2, "the named literal and the disguised one");
    assert!(!found[0].written_as_self);
    assert!(
        found[1].written_as_self,
        "`impl Default for BundleCore` builds one under a trait's name, and the \
         identically written `Self {{ tier }}` in `SomethingElse`'s impl is not \
         confused for it"
    );

    let items = sample.items();
    assert_eq!(
        common::impls_for(items, "BundleCore").len(),
        2,
        "the inherent impl and the trait impl"
    );
    assert_eq!(
        common::impls_for(items, "SomethingElse").len(),
        1,
        "and one type's impls are not another's"
    );
}

#[test]
fn a_fixture_in_a_modules_own_tests_is_not_a_construction_site() {
    // And — the reason a `#[cfg(test)]` module is skipped by its attribute
    // rather than by truncating the file at it — a construction site written
    // *below* a test module is still found. Truncation left `sneaky` unscanned,
    // which is precisely where a second constructor would end up if someone were
    // avoiding this test.
    let sample = common::read(
        "sample",
        r"
fn real() -> BundleCore {
    BundleCore { tier }
}
#[cfg(test)]
mod tests {
    fn fixture() -> BundleCore {
        BundleCore { tier }
    }
    fn nested() {
        if true {
            let _ = BundleCore { tier };
        }
    }
}
fn sneaky() -> BundleCore {
    BundleCore { tier }
}
",
    );
    let found: Vec<String> = common::struct_literals(&sample)
        .into_iter()
        .filter(|literal| literal.type_name == "BundleCore")
        .map(|literal| literal.function)
        .collect();

    assert_eq!(
        found,
        ["real", "sneaky"],
        "the two non-test sites, and only those"
    );
}

#[test]
fn a_literal_nested_inside_an_expression_is_still_a_construction_site() {
    // Not a hypothetical bypass so much as a completeness check: a literal
    // inside a closure, a `match` arm, or another struct's field is the same
    // construction, and the reader walks the whole expression tree rather than
    // the item headers.
    let sample = common::read(
        "sample",
        r"
fn assemble() -> EvidenceBundle {
    EvidenceBundle {
        core: match ready {
            true => BundleCore { tier },
            false => (|| BundleCore { tier })(),
        },
    }
}
",
    );
    let found: Vec<String> = common::struct_literals(&sample)
        .into_iter()
        .filter(|literal| literal.type_name == "BundleCore")
        .map(|literal| literal.function)
        .collect();

    assert_eq!(
        found,
        ["assemble", "assemble"],
        "a literal in a `match` arm and one inside a closure are both sites"
    );
}

#[test]
fn a_literal_written_inside_a_macro_is_a_construction_site() {
    // The regression a naive parse introduced: `syn` treats a `macro_rules!`
    // body as opaque tokens, so a literal written there is invisible to the
    // tree while the text scan this file replaced could still see it. A macro
    // is a construction site in every expansion of itself, and a `..core`
    // functional update is the shape that overwrites exactly one field — the
    // one this whole file exists to say can never be corrected.
    let sample = common::read(
        "sample",
        r"
macro_rules! retier {
    ($core:expr, $tier:expr) => {
        BundleCore { tier: $tier, ..$core }
    };
}

/// A public re-tiering entry point.
pub fn retiered(core: BundleCore, tier: Tier) -> BundleCore {
    retier!(core, tier)
}
",
    );
    let found: Vec<Literal> = common::struct_literals(&sample)
        .into_iter()
        .filter(|literal| literal.type_name == "BundleCore")
        .collect();

    assert_eq!(found.len(), 1, "the literal inside the macro body");
    assert_eq!(
        found[0].function, "<none>",
        "a macro body sits in no function, which is itself enough to fail the \
         `must be `BundleCore::new`` assertion above"
    );
}

#[test]
fn a_literal_inside_a_const_block_is_a_construction_site() {
    // `const _: () = { … };` and a function body are both places a literal can
    // be written that a walk over top-level items and modules never reaches.
    let sample = common::read(
        "sample",
        r"
const SEED: BundleCore = { BundleCore { tier } };
fn outer() {
    fn inner() -> BundleCore {
        BundleCore { tier }
    }
}
",
    );
    let found: Vec<String> = common::struct_literals(&sample)
        .into_iter()
        .filter(|literal| literal.type_name == "BundleCore")
        .map(|literal| literal.function)
        .collect();

    assert_eq!(found, ["<none>", "inner"], "both, and each attributed");
}

#[test]
fn a_cfg_not_test_construction_site_is_a_construction_site() {
    let sample = common::read(
        "sample",
        r"
#[cfg(not(test))]
fn ships() -> BundleCore {
    BundleCore { tier }
}
#[cfg(test)]
fn fixture() -> BundleCore {
    BundleCore { tier }
}
",
    );
    let found: Vec<String> = common::struct_literals(&sample)
        .into_iter()
        .filter(|literal| literal.type_name == "BundleCore")
        .map(|literal| literal.function)
        .collect();

    assert_eq!(
        found,
        ["ships"],
        "`not(test)` is in the artifact people link against; `test` is not"
    );
}

#[test]
fn a_macro_used_in_type_position_does_not_stop_this_guard() {
    // A macro may legally expand into far more than an item list, an expression
    // or a block, and the reader used to try only those three. This is ordinary
    // stable Rust:
    //
    //     macro_rules! list { ($item:ty) => { ::std::vec::Vec<$item> }; }
    //     pub type Facts = list!(EvidenceFacts);
    //
    // and it made the reader panic with a message about an `impl` hiding inside
    // a macro body — on *this* guard, which has nothing to do with the change,
    // because this guard reads every source file in the workspace. Someone
    // adding a type alias would see two frozen-model tests explode for reasons
    // they cannot act on, and that is the shape of failure that gets a guard
    // deleted rather than fixed.
    //
    // The expansion is still visited, and there is still nothing in it: a type
    // cannot carry a construction site.
    let sample = common::read(
        "sample",
        r"
macro_rules! list {
    ($item:ty) => { ::std::vec::Vec<$item> };
}
pub type Facts = list!(BundleCore);

macro_rules! fields {
    ($name:ident) => { $name: Tier, digest: Digest };
}

macro_rules! guard {
    ($t:ty) => { $t: Clone + Send };
}

macro_rules! branch {
    ($pattern:pat) => { $pattern => BundleCore { tier } };
}

pub fn real() -> BundleCore {
    BundleCore { tier }
}
",
    );
    let found: Vec<String> = common::struct_literals(&sample)
        .into_iter()
        .filter(|literal| literal.type_name == "BundleCore")
        .map(|literal| literal.function)
        .collect();

    assert_eq!(
        found,
        ["real", "<none>"],
        "the one in the function and the one in the match-arm macro body — a \
         type, a field list and a where-clause predicate carry none, and the \
         arm is still read rather than merely recognised and dropped"
    );
}

#[test]
#[should_panic(expected = "could not be re-parsed")]
fn an_unreadable_expansion_that_could_hold_an_item_is_still_a_loud_failure() {
    // The half of the previous test that must not be lost. Widening the reader
    // to accept the positions that declare nothing is only safe while an
    // expansion it can place *nowhere* still stops the guard — otherwise the
    // fix for a spurious panic becomes a silent skip, which is the regression
    // the macro reader exists to undo, reintroduced through its own escape
    // hatch.
    let _ = common::read(
        "sample",
        "macro_rules! partial { ($t:ty) => { impl $t for }; }\n",
    );
}

#[test]
fn a_type_position_macro_whose_expansion_says_impl_is_not_a_hidden_item() {
    // The falsifier the previous round was missing. Recognising the
    // declaration-free expansion positions and the keyword backstop are two
    // separate layers, and the keyword backstop alone covers almost
    // everything — `::std::vec::Vec<$item>` contains no `impl`, no `fn` and no
    // `struct`, so it is passed over whether or not anything parsed it.
    //
    // This is the shape where they come apart:
    //
    //     macro_rules! iter { ($t:ty) => { impl Iterator<Item = $t> }; }
    //
    // `impl Trait` is a *type*, and it is the one type whose spelling starts
    // with the keyword that means "an item is hiding here". Without the
    // position parse, the backstop reads the `impl` and the reader panics —
    // five tests across two guard binaries, on a return type that declares
    // nothing at all.
    let sample = common::read(
        "sample",
        r"
macro_rules! iter {
    ($t:ty) => { impl Iterator<Item = $t> };
}
macro_rules! boxed {
    ($t:ty) => { Box<dyn Fn() -> $t + Send> };
}
pub fn stream() -> iter!(u8) {
    ::core::iter::empty()
}
pub fn real() -> BundleCore {
    BundleCore { tier }
}
",
    );
    let found: Vec<String> = common::struct_literals(&sample)
        .into_iter()
        .filter(|literal| literal.type_name == "BundleCore")
        .map(|literal| literal.function)
        .collect();

    assert_eq!(
        found,
        ["real"],
        "an `impl Trait` expansion is a type, and a type holds no construction \
         site — but the reader has to be able to *say* it is a type, because \
         the word `impl` is otherwise the signal that it cannot"
    );
}
