//! Structural guards on the escalation accumulator.
//!
//! The claim that "verdicts only ever move up in scrutiny" rests on things that
//! are true of the *source files*, not of any value:
//!
//! 1. Nothing but `escalate` writes `Adjudicator::tier`, and nothing but `new`
//!    produces an `Adjudicator`.
//! 2. `adjudicate::accumulator` has no child modules.
//! 3. `adjudicate::enforcement` has no child modules either, for the same
//!    reason: `Adjudicators` holds its two ledgers as private fields, and a
//!    child could swap them or hand out the advisory one under another name.
//! 4. `Adjudicators::route` is not `pub`, so the advisory ledger is not
//!    nameable from outside this crate.
//! 5. `AdvisoryAdjudication` yields no verdict.
//! 6. `CapabilityResolution::account` is not `pub`, so accounting a set of
//!    resolutions in an order of the caller's choosing is not expressible from
//!    outside the crate.
//!
//! The second one is easy to miss in review. Rust's field privacy is
//! module-scoped, so a private field is reachable from every descendant module —
//! adding `mod helpers;` inside `accumulator` would silently hand that module
//! write access to the tier, and nothing in the type system would object.
//!
//! None of these is expressible as a type, so they are asserted against the
//! source. A test that reads source is unusual and worth the oddity here: the
//! alternative is a comment asking reviewers to notice something subtle forever.
//!
//! # The invariant is stated over the syntax tree, not over text
//!
//! "Exactly one method takes `&mut self`" was only ever a *proxy* for the real
//! law, and it was a leaky one. A consuming builder passed every check the text
//! scanner made:
//!
//! ```ignore
//! pub fn with_tier(mut self, tier: Tier) -> Self
//! ```
//!
//! No `&mut self`, and not one of the four literal escape hatches the old guard
//! listed — yet it assigns the tier, which is exactly the operation the lattice
//! forbids. `LocalScheduler::with_concurrency` is already in the tree in that
//! shape, so it is the pattern someone copies by analogy rather than a
//! hypothetical. `impl IndexMut` and `impl BorrowMut` slipped past for the same
//! reason: the guard knew four strings, not the idea behind them.
//!
//! So these guards parse the file with `syn` and ask about receivers, return
//! types, trait paths and visibilities directly. See `tests/common/mod.rs` for
//! the readers. Parsing also retires the two scanning regimes this file used to
//! carry — one truncating at the first `#[cfg(test)]`, one excising by brace
//! matching. A test module is now skipped by its *attribute*, so a `mod
//! helpers;` written below one is seen like any other, which closes the gap #25
//! recorded here as owed to #35.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Receiver, Vis};

const ACCUMULATOR: &str = include_str!("../src/adjudicate/accumulator.rs");
const ENFORCEMENT: &str = include_str!("../src/adjudicate/enforcement.rs");
const KNOWN: &str = include_str!("../src/known.rs");
const RESOLUTION: &str = include_str!("../src/resolution.rs");

/// Traits whose whole purpose is to hand out a writable view of a value.
///
/// Matched by the trait's own path rather than by four literal strings, so
/// `core::ops::DerefMut` and `DerefMut` are the same prohibition and adding a
/// fifth trait to this list is the only way to extend it.
const WRITABLE_VIEWS: [&str; 4] = ["DerefMut", "AsMut", "BorrowMut", "IndexMut"];

/// `accumulator.rs`, parsed, with its own tests skipped.
fn accumulator() -> common::Source {
    common::read("accumulator.rs", ACCUMULATOR)
}

/// `enforcement.rs`, parsed, with its own tests skipped.
fn enforcement() -> common::Source {
    common::read("enforcement.rs", ENFORCEMENT)
}

/// `resolution.rs`, parsed, with its own tests skipped.
fn resolution() -> common::Source {
    common::read("resolution.rs", RESOLUTION)
}

#[test]
fn escalate_is_the_only_mutator_on_the_adjudicator() {
    let file = accumulator();
    let items = file.items();
    let mutators: Vec<String> = common::functions(items)
        .into_iter()
        .filter(|function| function.receiver == Receiver::RefMut)
        .map(|function| function.path())
        .collect();

    assert_eq!(
        mutators,
        ["Adjudicator::escalate"],
        "exactly one method in this file may take `&mut self`\n\
         \n\
         Adding another is how \"scrutiny only rises\" stops being a property of \
         the API and becomes a rule someone has to keep enforcing. If new mutable \
         behaviour is genuinely needed, express it in terms of `escalate` rather \
         than alongside it."
    );
}

#[test]
fn no_consuming_builder_returns_an_adjudicator() {
    // The hole the `&mut self` rule could never see. `fn with_tier(mut self,
    // tier: Tier) -> Self` takes `self` by value, assigns the tier, and hands
    // back an adjudicator — a *lowering* if `tier` is below the one it started
    // from, written in the shape of an ergonomic builder.
    //
    // `finish(self) -> Adjudication` is the reason this rule is about the return
    // type and not about the receiver: consuming an adjudicator to produce
    // something that is not one is exactly how a verdict is meant to be sealed.
    let file = accumulator();
    let items = file.items();
    let builders: Vec<String> = common::functions(items)
        .into_iter()
        .filter(|function| {
            function.owner.as_deref() == Some("Adjudicator")
                && function.receiver == Receiver::Value
                && function.returns.as_deref() == Some("Adjudicator")
        })
        .map(|function| function.path())
        .collect();

    assert!(
        builders.is_empty(),
        "no method may consume an `Adjudicator` and return another one: \
         {builders:#?}\n\
         \n\
         A consuming builder assigns what `escalate` may only join, and it does \
         so without taking `&mut self` — which is why the receiver rule alone \
         was never enough. Express the change as an `escalate` call."
    );
}

#[test]
fn only_new_produces_an_adjudicator() {
    // The other half of the real law. A second constructor — `from_tier`,
    // `restored`, `with_ledger` — is an adjudicator that did not start at
    // `Tier::BOTTOM`, and the lattice argument assumes it did.
    //
    // File-local on purpose. Workspace-wide this would flag
    // `Adjudicators::route` and `Adjudicators::integrity`, which return
    // `&mut Adjudicator` by design and are the supported way to reach one.
    let file = accumulator();
    let items = file.items();
    let producers: Vec<String> = common::functions(items)
        .into_iter()
        .filter(|function| function.returns.as_deref() == Some("Adjudicator"))
        .map(|function| function.path())
        .collect();

    assert_eq!(
        producers,
        ["Adjudicator::new"],
        "`Adjudicator::new` must be the only thing in this file that produces an \
         `Adjudicator`\n\
         \n\
         Every claim about the accumulator starts \"an adjudicator begins at \
         `Tier::BOTTOM`\". A second constructor is a value for which that \
         sentence is false, and nothing downstream can tell the two apart."
    );
}

#[test]
fn the_accumulator_module_has_no_children() {
    // A child module would inherit access to the private `tier` field.
    let file = accumulator();
    let declarations = common::module_declarations(file.items());
    assert!(
        declarations.is_empty(),
        "`adjudicate::accumulator` must have no submodules, found: {declarations:#?}\n\
         \n\
         Rust field privacy is module-scoped, so a child module can write `tier` \
         directly and bypass `escalate` entirely. Put new code in a sibling under \
         `adjudicate/`, where it has to go through the public API."
    );
}

#[test]
fn the_enforcement_module_has_no_children() {
    // `Adjudicators` holds `enforcing` and `advisory` as private fields of the
    // same type. A child module could swap them, or return `&mut self.advisory`
    // from something called `integrity`. Either is a downward operation wearing
    // a different word, and nothing in the type system would object.
    let file = enforcement();
    let declarations = common::module_declarations(file.items());
    assert!(
        declarations.is_empty(),
        "`adjudicate::enforcement` must have no submodules, found: {declarations:#?}"
    );
}

#[test]
fn the_known_module_has_no_children() {
    // Same reasoning: a child of `known` could match on the private `Inner` enum
    // and read an unresolved identifier without escalating, which is the one
    // thing `Known::get` exists to prevent.
    let file = common::read("known.rs", KNOWN);
    let declarations = common::module_declarations(file.items());
    assert!(
        declarations.is_empty(),
        "`known` must have no submodules, found: {declarations:#?}"
    );
}

#[test]
fn routing_is_not_public() {
    // `route` picks a ledger without applying the policy-integrity override that
    // `account` applies first. If it were `pub`, a downstream crate could write
    // `known.get(adjudicators.route(Enforcement::Advisory))` and resolve an
    // unknown capability against a ledger nothing enforces — a gate disabled by
    // typo, which is the exact failure `known.rs` exists to prevent.
    //
    // With it `pub(crate)`, the only `&mut Adjudicator` obtainable from outside
    // this crate is `integrity()`, which is the enforcing ledger. That is what
    // makes the behavioural tests exhaustive rather than illustrative.
    let file = enforcement();
    let items = file.items();
    let route: Vec<Vis> = common::functions(items)
        .into_iter()
        .filter(|function| {
            function.owner.as_deref() == Some("Adjudicators") && function.name == "route"
        })
        .map(|function| function.visibility)
        .collect();

    assert_eq!(
        route.len(),
        1,
        "sanity check: `Adjudicators::route` should exist, exactly once"
    );
    assert_eq!(
        route[0],
        Vis::Restricted,
        "`Adjudicators::route` must stay `pub(crate)`; routing belongs to \
         `CapabilityResolution::account`, which applies the policy-integrity \
         override before choosing a lane"
    );
}

#[test]
fn the_advisory_adjudication_yields_no_verdict() {
    // `Adjudicator::finish` always sets `verdict: tier.verdict()`. An advisory
    // ledger that could hand out its `Adjudication` would therefore hand out a
    // second, more permissive verdict, and a bundle carrying two verdicts is a
    // bundle whose readers will disagree about what happened.
    //
    // Asked of every function whose `impl` is for `AdvisoryAdjudication`, so a
    // second inherent impl is covered as well as the first, and of every
    // conversion *out of* the type — `impl From<AdvisoryAdjudication> for
    // Adjudication` hands out the same thing under a trait's name, and so do
    // `TryFrom` and `Into`, which the old text scan could not see at all.
    let file = enforcement();
    let items = file.items();

    let methods = common::functions(items);
    let advisory: Vec<_> = methods
        .iter()
        .filter(|function| function.owner.as_deref() == Some("AdvisoryAdjudication"))
        .collect();

    let leaking: Vec<String> = advisory
        .iter()
        .filter(|function| {
            function.name == "verdict" || function.returns.as_deref() == Some("Adjudication")
        })
        .map(|function| function.path())
        .collect();
    assert!(
        leaking.is_empty(),
        "`AdvisoryAdjudication` must expose neither a verdict nor an \
         `Adjudication`: {leaking:#?}\n\
         \n\
         Only the enforced ledger becomes a verdict, and a bundle carries \
         exactly one."
    );

    let converted: Vec<String> = common::conversions(items)
        .into_iter()
        .filter(|conversion| conversion.source == "AdvisoryAdjudication")
        .map(|conversion| conversion.rendered())
        .collect();
    assert!(
        converted.is_empty(),
        "nothing may convert an `AdvisoryAdjudication` into anything else: \
         {converted:#?}\n\
         \n\
         `Adjudication` always carries a verdict derived from its tier, and the \
         advisory tier is not allowed to become one. A conversion is that \
         promotion with a trait's name on it."
    );

    assert!(
        advisory.iter().any(|function| function.name == "tier"),
        "sanity check: the impl this test scans should have been found, and it \
         should be the one declaring `tier`"
    );
}

#[test]
fn the_adjudicators_tier_field_is_private() {
    // Scoped to `Adjudicator` deliberately. `Adjudication` — the finished,
    // consumed-by-value result — *does* expose `tier` publicly, and that is
    // fine: it has no mutator at all, so a public field on it cannot be used to
    // lower anything.
    let file = accumulator();
    let items = file.items();
    let fields = common::struct_fields(items, "Adjudicator");

    let tier = fields
        .iter()
        .find(|(name, _)| name == "tier")
        .expect("sanity check: the field this test guards should exist");
    assert_eq!(
        tier.1,
        Vis::Inherited,
        "`Adjudicator::tier` must stay private; a public field can be assigned, \
         and assignment is exactly the operation the lattice forbids"
    );
}

#[test]
fn the_adjudicator_exposes_no_escape_hatches() {
    let file = accumulator();
    let items = file.items();

    let views: Vec<String> = common::traits_implemented_for(items, "Adjudicator")
        .into_iter()
        .filter(|name| WRITABLE_VIEWS.contains(&name.as_str()))
        .collect();
    assert!(
        views.is_empty(),
        "`Adjudicator` must implement none of {WRITABLE_VIEWS:?}, found: \
         {views:#?}\n\
         \n\
         Each of them hands out a `&mut Tier` — or a `&mut Adjudicator` from \
         which one is reachable — and a tier that can be assigned is a verdict \
         that can be lowered."
    );

    // Unscoped by owner, deliberately. The obvious escape hatch is a method,
    // but `pub fn set_tier(adjudicator: &mut Adjudicator, tier: Tier)` written
    // beside the type does the same job with no receiver at all — field privacy
    // is module-scoped, so the assignment is legal from anywhere in this file,
    // and `escalate_is_the_only_mutator_on_the_adjudicator` cannot see it
    // because a `&mut Adjudicator` *parameter* is not a `&mut self` receiver.
    let setters: Vec<String> = common::functions(items)
        .into_iter()
        .filter(|function| matches!(function.name.as_str(), "set_tier" | "tier_mut"))
        .map(|function| function.path())
        .collect();
    assert!(
        setters.is_empty(),
        "`{setters:?}` would allow a tier to be assigned rather than joined, \
         which makes lowering a verdict expressible"
    );

    let borrowers: Vec<String> = common::functions(items)
        .into_iter()
        .filter(|function| {
            function
                .mutably_borrows
                .iter()
                .any(|parameter| parameter == "Adjudicator")
        })
        .map(|function| function.path())
        .collect();
    assert!(
        borrowers.is_empty(),
        "nothing in this file may take a `&mut Adjudicator` as a parameter: \
         {borrowers:#?}\n\
         \n\
         Inside this module a `&mut Adjudicator` is write access to a private \
         `tier`, and it arrives with no receiver for a rule about receivers to \
         catch. `escalate` is a method for exactly this reason: the mutation \
         has to be reachable only through the type's own API."
    );
}

#[test]
fn the_adjudicator_has_no_default() {
    // `Default` is the escape hatch that does not look like one. It reintroduces
    // exactly what `new` being the sole constructor is meant to prevent: an
    // adjudicator conjured in the middle of a function that should have threaded
    // the real one through, accumulating escalations nobody will ever read.
    //
    // Removing the impl trips `clippy::new_without_default`, which is allowed
    // with its reasoning next to the type. This test is what stops someone
    // silencing that lint the other way.
    let file = accumulator();
    let items = file.items();
    assert!(
        !common::traits_implemented_for(items, "Adjudicator").contains(&"Default".to_owned()),
        "`Adjudicator` must not implement `Default`; the type's own documentation \
         explains why, and an impl that contradicts its documentation is worse \
         than either one alone"
    );
}

#[test]
fn accounting_is_not_public() {
    // `CapabilityResolution::account` accounts *one* resolution. A caller that
    // could reach it could account a whole set in whatever order its scheduler
    // happened to produce, and both escalation ledgers are bundle fields — so
    // that order would be part of what a verdict digest covers, and two runs of
    // the same commit could disagree byte for byte.
    //
    // With it `pub(crate)`, the only way in from outside this crate is
    // `Resolutions::account_into`, which walks a `BTreeMap` ascending by
    // `RequirementId` and offers no second mode. Unordered accounting is not
    // discouraged, it is unnameable.
    //
    // This used to be a needle whose trailing `(` was load-bearing — without it
    // the same needle matched `pub fn account_into(` and the guard failed on the
    // API it exists to protect. Asked of the syntax tree, `account` and
    // `account_into` are simply two different functions, and the hazard is gone
    // rather than documented.
    let file = resolution();
    let items = file.items();
    let methods = common::functions(items);

    let account = methods
        .iter()
        .find(|function| {
            function.owner.as_deref() == Some("CapabilityResolution") && function.name == "account"
        })
        .expect("sanity check: the method this test guards should exist");
    assert_eq!(
        account.visibility,
        Vis::Restricted,
        "`CapabilityResolution::account` must stay `pub(crate)`; accounting in \
         an order of the caller's choosing is what `Resolutions::account_into` \
         exists to make inexpressible"
    );

    let account_into = methods
        .iter()
        .find(|function| {
            function.owner.as_deref() == Some("Resolutions") && function.name == "account_into"
        })
        .expect("sanity check: the ordered entry point must exist");
    assert_eq!(
        account_into.visibility,
        Vis::Public,
        "sanity check: the ordered entry point must be public, or demoting \
         `account` would have removed the ability to account at all"
    );
}

// --- anti-vacuity ----------------------------------------------------------
//
// Every guard above is only as good as the reader beneath it, and a reader that
// silently matched nothing would pass all of them forever. One meta-test per
// rule, each planting the violation that rule exists to catch.

#[test]
fn the_receiver_reader_tells_a_consuming_builder_from_a_getter() {
    let sample = common::read(
        "sample",
        r"
        impl Adjudicator {
            pub fn new() -> Self { Self {} }
            pub fn read(&self) -> u8 { 0 }
            pub fn write(
                &mut self,
                value: u8,
            ) {}
            pub fn with_tier(mut self, tier: Tier) -> Self { self }
            pub fn finish(self) -> Adjudication { Adjudication {} }
            fn helper(&mut self) {}
        }
        ",
    );
    let items = sample.items();
    let read = common::functions(items);

    let mutators: Vec<String> = read
        .iter()
        .filter(|function| function.receiver == Receiver::RefMut)
        .map(|function| function.name.clone())
        .collect();
    assert_eq!(
        mutators,
        ["write", "helper"],
        "the reader must see through rustfmt line wrapping, and must not count \
         `mut self` as a mutable borrow"
    );

    let builders: Vec<String> = read
        .iter()
        .filter(|function| {
            function.receiver == Receiver::Value
                && function.returns.as_deref() == Some("Adjudicator")
        })
        .map(|function| function.name.clone())
        .collect();
    assert_eq!(
        builders,
        ["with_tier"],
        "the consuming builder is the hole this file exists to close, and \
         `finish` — which consumes one and returns something else — is not it"
    );
}

#[test]
fn the_return_type_reader_resolves_self_to_the_impl() {
    let sample = common::read(
        "sample",
        r"
        impl Adjudicator {
            pub fn new() -> Self { Self {} }
            pub fn restored(ledger: Vec<Escalation>) -> Adjudicator { Adjudicator {} }
            pub fn maybe() -> Option<Box<Adjudicator>> { None }
            pub fn tier(&self) -> Tier { self.tier }
        }
        ",
    );
    let items = sample.items();
    let producers: Vec<String> = common::functions(items)
        .into_iter()
        .filter(|function| function.returns.as_deref() == Some("Adjudicator"))
        .map(|function| function.name)
        .collect();

    assert_eq!(
        producers,
        ["new", "restored", "maybe"],
        "`-> Self`, `-> Adjudicator` and `-> Option<Box<Adjudicator>>` are the \
         same production, and a second constructor written any of those ways \
         must be seen"
    );
}

#[test]
fn the_trait_reader_sees_a_writable_view_however_it_is_pathed() {
    let sample = common::read(
        "sample",
        r"
        impl core::ops::DerefMut for Adjudicator {}
        impl BorrowMut<Tier> for Adjudicator {}
        impl IndexMut<usize> for Adjudicator {}
        impl AsMut<Tier> for Adjudicator {}
        impl Deref for Adjudicator {}
        impl DerefMut for SomethingElse {}
        ",
    );
    let items = sample.items();
    let found: Vec<String> = common::traits_implemented_for(items, "Adjudicator")
        .into_iter()
        .filter(|name| WRITABLE_VIEWS.contains(&name.as_str()))
        .collect();

    assert_eq!(
        found,
        ["DerefMut", "BorrowMut", "IndexMut", "AsMut"],
        "all four writable views, matched by trait path rather than by literal \
         string — and `Deref`, which is read-only, left alone, as is another \
         type's `DerefMut`"
    );
}

#[test]
fn the_visibility_reader_tells_pub_from_pub_crate() {
    let sample = common::read(
        "sample",
        r"
        impl CapabilityResolution {
            pub(crate) fn account(&self) {}
        }
        impl Resolutions {
            pub fn account_into(&self) {}
        }
        ",
    );
    let items = sample.items();
    let read = common::functions(items);

    let account = read.iter().find(|f| f.name == "account").unwrap();
    let account_into = read.iter().find(|f| f.name == "account_into").unwrap();

    assert_eq!(account.visibility, Vis::Restricted);
    assert_eq!(
        account_into.visibility,
        Vis::Public,
        "the two are different functions to a parser, so the needle whose \
         trailing `(` used to be load-bearing has nothing left to get wrong"
    );
}

#[test]
fn the_field_reader_tells_a_private_field_from_a_public_one() {
    let sample = common::read(
        "sample",
        r"
        pub struct Adjudicator {
            tier: Tier,
            pub ledger: Vec<Escalation>,
        }
        ",
    );
    let items = sample.items();
    assert_eq!(
        common::struct_fields(items, "Adjudicator"),
        vec![
            ("tier".to_owned(), Vis::Inherited),
            ("ledger".to_owned(), Vis::Public),
        ]
    );
}

#[test]
fn the_reader_sees_a_module_declared_below_a_test_module() {
    // The reason this file no longer truncates at the first `#[cfg(test)]`. A
    // child module written below `mod tests` has exactly the same write access
    // to a private field as one written above it, and truncation could not see
    // it at all.
    let sample = common::read(
        "sample",
        r"
mod above;
#[cfg(test)]
mod tests {
    mod fixture;
}
mod below;
",
    );

    assert_eq!(
        common::module_declarations(sample.items()),
        ["above", "below"],
        "both real children, and neither the test module nor anything inside it"
    );
}

#[test]
fn the_conversion_reader_sees_every_spelling_out_of_the_advisory_ledger() {
    let sample = common::read(
        "sample",
        r"
        impl From<AdvisoryAdjudication> for Adjudication {}
        impl TryFrom<AdvisoryAdjudication> for Adjudication {}
        impl Into<Adjudication> for AdvisoryAdjudication {}
        impl From<EnforcedAdjudication> for Adjudication {}
        ",
    );
    let items = sample.items();
    let out: Vec<String> = common::conversions(items)
        .into_iter()
        .filter(|conversion| conversion.source == "AdvisoryAdjudication")
        .map(|conversion| conversion.via)
        .collect();

    assert_eq!(
        out,
        ["From", "TryFrom", "Into"],
        "`Into` reverses the two types and `TryFrom` was invisible to a scan for \
         `From<AdvisoryAdjudication>`; the enforced ledger's conversion is not \
         this rule's business"
    );
}

#[test]
fn the_reader_tells_a_test_gate_from_its_negation() {
    // "Does the `cfg` mention `test`" also matched `not(test)`, which is the
    // form that ships — so a `mod helpers;` or an `impl Default` behind it was
    // dropped from every guard in this file without a word.
    let sample = common::read(
        "sample",
        r#"
#[cfg(not(test))]
mod ships;
#[cfg(test)]
mod fixtures;
#[cfg(all(test, feature = "e2e"))]
mod slow;
#[cfg(any(test, unix))]
mod platform;
#[cfg(feature = "e2e")]
mod gated;
"#,
    );

    assert_eq!(
        common::module_declarations(sample.items()),
        ["ships", "platform", "gated"],
        "`not(test)` and `any(test, unix)` both ship; `test` and \
         `all(test, …)` do not"
    );
}

#[test]
fn a_free_function_taking_a_mutable_adjudicator_is_an_escape_hatch() {
    // The hole scoping the escape-hatch rule to methods opened. Field privacy
    // is module-scoped, so this assigns `tier` legally — and it has no
    // receiver, so the `&mut self` rule cannot see it either.
    let sample = common::read(
        "sample",
        r"
        pub fn set_tier(adjudicator: &mut Adjudicator, tier: Tier) {
            adjudicator.tier = tier;
        }
        pub fn inspect(adjudicator: &Adjudicator) -> Tier {
            adjudicator.tier
        }
        impl Adjudicator {
            pub fn escalate(&mut self, at_least: Tier) {}
        }
        ",
    );
    let read = common::functions(sample.items());

    assert_eq!(
        read.iter()
            .filter(|function| function.mutably_borrows.iter().any(|p| p == "Adjudicator"))
            .map(|function| function.name.clone())
            .collect::<Vec<_>>(),
        ["set_tier"],
        "the mutable borrow, and neither the shared one nor the receiver"
    );
    assert_eq!(
        read.iter()
            .filter(|function| function.receiver == Receiver::RefMut)
            .map(|function| function.name.clone())
            .collect::<Vec<_>>(),
        ["escalate"],
        "and the receiver rule genuinely cannot see the free function, which \
         is why both are asserted"
    );
}

#[test]
fn the_trait_reader_sees_a_derive() {
    // `#[derive(Default)]` and `impl Default for Adjudicator` produce the same
    // public `Adjudicator::default()`, and the derive is the more idiomatic of
    // the two. A rule that only knew the impl was a rule about spelling.
    let sample = common::read(
        "sample",
        r"
        #[derive(Debug, Default)]
        pub struct Adjudicator {
            tier: Tier,
        }
        #[derive(Clone)]
        pub struct Adjudication {}
        ",
    );
    let traits = common::traits_implemented_for(sample.items(), "Adjudicator");

    assert_eq!(traits, ["Debug", "Default"]);
    assert!(
        !common::traits_implemented_for(sample.items(), "Adjudication")
            .contains(&"Default".to_owned()),
        "and one type's derive list is not another's"
    );
}

#[test]
fn an_item_inside_a_const_block_is_read() {
    // `const _: () = { … };` registers an impl globally while sitting inside an
    // initialiser, where a walk over modules alone never looks.
    let sample = common::read(
        "sample",
        r"
        const _: () = {
            impl core::ops::DerefMut for Adjudicator {}
            mod helpers {}
        };
        ",
    );

    assert!(
        common::traits_implemented_for(sample.items(), "Adjudicator")
            .contains(&"DerefMut".to_owned())
    );
    assert_eq!(common::module_declarations(sample.items()), ["helpers"]);
}

#[test]
#[should_panic(expected = "cannot classify")]
fn a_cfg_this_reader_cannot_classify_is_a_loud_failure() {
    // The other half of the `cfg` fix, and the half that is easy to leave
    // decorative. Assuming an unreadable gate is test-only is how an item
    // vanishes from a guard in silence; assuming it ships would be a guard that
    // fails for reasons nobody can act on. Neither is a guess worth making, so
    // the reader stops instead — and this is what proves it actually does.
    let _ = common::read("sample", "#[cfg(1 + 1)]\nmod helpers {}\n");
}

#[test]
fn a_cfg_with_a_trailing_comma_is_read_rather_than_refused() {
    // `#[cfg(test,)]` compiles, and is configured out of an ordinary build
    // exactly like `#[cfg(test)]` — an attribute's argument list takes a
    // trailing comma the way every other list in the language does. The reader
    // consumed one predicate and left the comma behind, and the leftover made
    // `syn::parse2` fail, which turned a legal spelling into the loud panic
    // above.
    //
    // The nested `all(test,)` was already accepted, because an inner list is
    // read with `parse_terminated`. That asymmetry is what makes this a bug
    // rather than a rule: nobody could have derived which of the two forms was
    // allowed, and the failure blames the wrong thing when they guess wrong.
    let sample = common::read(
        "sample",
        r"
        #[cfg(test,)]
        mod fixture {}
        #[cfg(all(test,))]
        mod also_fixture {}
        #[cfg(not(test),)]
        mod ships {}
        ",
    );

    assert_eq!(
        common::module_declarations(sample.items()),
        ["ships"],
        "the trailing comma changes nothing about which build an item is in"
    );
}

#[test]
fn a_raw_identifier_predicate_is_the_predicate_it_spells() {
    // `#[cfg(r#test)]` is `#[cfg(test)]`: rustc compares the symbol, and the
    // `r#` is lexical syntax rather than part of the name. `Ident::to_string`
    // keeps the prefix, so the comparison against `"test"` failed and a
    // test-only item read as shipping.
    //
    // That direction over-reports rather than under-reports, so nothing was
    // hidden by it — but the guard's safety was resting on a spelling accident,
    // and the next person to compare an identifier the same way gets no warning
    // that they are on the other side of it.
    let sample = common::read(
        "sample",
        r"
        #[cfg(r#test)]
        mod fixture {}
        #[cfg(not(r#test))]
        mod ships {}
        ",
    );

    assert_eq!(
        common::module_declarations(sample.items()),
        ["ships"],
        "a raw identifier names the same predicate as the bare one"
    );
}

#[test]
fn an_unevaluable_predicate_survives_a_negation() {
    // The lattice fix, asserted on this guard's own reader as well as on the
    // conversion guard's. `Cfg::Other` used to be stored as the two-valued
    // `true`, so `not(…)` inverted it and an item behind
    // `#[cfg(not(feature = …))]` — the ordinary way to write a default — read
    // as configured out. Every rule in this file is over the items that ship,
    // so an item that vanishes from the walk vanishes from the rule.
    let sample = common::read(
        "sample",
        r#"
        #[cfg(not(feature = "e2e"))]
        mod ships {}
        #[cfg(all(not(feature = "e2e"), not(test)))]
        mod also_ships {}
        #[cfg(any(feature = "e2e", test))]
        mod ships_too {}
        #[cfg(all(feature = "e2e", test))]
        mod does_not {}
        "#,
    );

    assert_eq!(
        common::module_declarations(sample.items()),
        ["ships", "also_ships", "ships_too"],
        "unknown propagates through `not`, `all` and `any` instead of \
         collapsing into a value a negation can flip — and `all(…, test)` is \
         still decidedly false"
    );
}
