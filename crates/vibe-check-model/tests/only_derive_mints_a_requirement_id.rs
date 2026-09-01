//! The guard behind `RequirementId`'s construction monopoly.
//!
//! `RequirementId` is the key of [`Resolutions`], and a second identifier for
//! one requirement — or one identifier for two — is a resolution displaced out
//! of the map, which reads from outside as a question that was answered. So
//! `derive` is the only way to *mint* one and `from_wire` the only way to read
//! one back, and `ids.rs` withholds `new`, `From<&str>` and `From<String>` by
//! invoking `id_newtype!`'s `@derived_only` arm.
//!
//! # Why prose was not enough
//!
//! The monopoly is implemented as an *absence*, and an absence is invisible.
//! Three edits undo it, each of which compiles, and none of which looks like a
//! security change while it is being written:
//!
//! 1. Dropping `@derived_only` from the invocation in `ids.rs`. The ordinary
//!    arm hands `RequirementId` an infallible `new` and two `From` impls, the
//!    build goes green, and the diff is four deleted tokens.
//! 2. `impl From<&str> for RequirementId` in some other file, falling back to
//!    a default scope or an `expect` on the error path.
//! 3. A plain `fn requirement_id_for(name: &str) -> RequirementId`, which wears
//!    no trait at all and which no scan for `impl From<` would ever see.
//!
//! This file asserts all three, the same way `no_evidence_from_status.rs`
//! asserts the absence of `From<CheckRun> for Evidence`.
//!
//! # What it cannot see, stated plainly
//!
//! Rule 1 is the load-bearing one, and it is checked by reading the
//! *invocation* rather than the expansion. It has to be: the structural reader
//! substitutes `$name` with `metavar_name` before re-parsing a macro body, so
//! the `impl From<&str>` the ordinary arm emits is attributed to `metavar_name`
//! and not to any of the eleven types the macro is invoked for. A guard reading
//! only expansions could not tell an identifier that gets `new` from one that
//! does not. Reading the invocation is the one place that distinction exists.
//!
//! [`Resolutions`]: vibe_check_model::Resolutions

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use proc_macro2::TokenTree;

/// The type whose construction is being guarded.
const GUARDED: &str = "RequirementId";

/// The macro arm that withholds the infallible constructors.
const DERIVED_ONLY: &str = "derived_only";

/// Functions permitted to hand back a `RequirementId`.
///
/// `derive` mints one and `from_wire` reads one back. `try_from` and
/// `deserialize` are the two trait impls `ids.rs` writes over `from_wire`, and
/// they are sanctioned by *trait and owner* rather than by name so that a
/// `try_from` on some other type does not inherit the exemption.
fn is_sanctioned(function: &common::Function) -> bool {
    if function.owner.as_deref() != Some(GUARDED) {
        return false;
    }
    match function.owner_trait.as_deref() {
        Some("TryFrom" | "Deserialize") => true,
        Some(_) => false,
        None => matches!(function.name.as_str(), "derive" | "from_wire"),
    }
}

/// Every `id_newtype!` invocation in a source, as `(names, uses_derived_only)`.
///
/// The invocation is an `Item::Macro` whose body `syn` hands back as opaque
/// tokens, so this reads the token stream directly: which identifiers it names,
/// and whether an `@`-prefixed `derived_only` appears among them.
fn id_newtype_invocations(items: &[syn::Item]) -> Vec<(Vec<String>, bool)> {
    let mut out = Vec::new();

    for item in items {
        let syn::Item::Macro(invocation) = item else {
            continue;
        };
        // A `macro_rules!` *definition* carries an ident; an invocation does not.
        if invocation.ident.is_some() || !invocation.mac.path.is_ident("id_newtype") {
            continue;
        }

        let trees: Vec<TokenTree> = invocation.mac.tokens.clone().into_iter().collect();
        let mut names = Vec::new();
        let mut derived_only = false;
        for (index, tree) in trees.iter().enumerate() {
            let TokenTree::Ident(ident) = tree else {
                continue;
            };
            let name = ident.to_string();
            if name == DERIVED_ONLY
                && index > 0
                && matches!(&trees[index - 1], TokenTree::Punct(punct) if punct.as_char() == '@')
            {
                derived_only = true;
                continue;
            }
            names.push(name);
        }
        out.push((names, derived_only));
    }

    out
}

/// Every `id_newtype!` invocation in the workspace, with the file it is in.
fn invocations() -> Vec<(String, Vec<String>, bool)> {
    let mut out = Vec::new();
    for (path, file) in common::workspace_sources() {
        for (names, derived_only) in id_newtype_invocations(file.items()) {
            out.push((path.to_string(), names, derived_only));
        }
    }
    out
}

// --- the three rules -------------------------------------------------------

#[test]
fn the_guarded_identifier_is_registered_without_the_infallible_constructors() {
    let mut registrations = 0usize;
    for (path, names, derived_only) in invocations() {
        if !names.iter().any(|name| name == GUARDED) {
            continue;
        }
        registrations += 1;
        assert!(
            derived_only,
            "{path}: `{GUARDED}` is registered through `id_newtype!`'s ordinary arm, \
             which hands it `new`, `From<&str>` and `From<String>`.\n\
             \n\
             Those three are the bypass this type exists to remove: a requirement \
             identifier is *computed* from a capability and a scope, and an \
             infallible constructor one keystroke shorter than the derivation is \
             the one that gets used. Registering it as `@{DERIVED_ONLY}` is what \
             withholds them."
        );
    }

    assert_eq!(
        registrations, 1,
        "expected exactly one `id_newtype!` invocation naming `{GUARDED}` — if it \
         moved or was renamed, this guard is scanning nothing and proving nothing"
    );
}

#[test]
fn nothing_converts_infallibly_into_one() {
    let mut offenders = Vec::new();

    for (path, file) in common::workspace_sources() {
        for conversion in common::conversions(file.items()) {
            if conversion.target != GUARDED {
                continue;
            }
            // `TryFrom` is fine: `ids.rs`'s two route through `from_wire`, and a
            // fallible conversion has somewhere to put a malformed identifier.
            // `From` and `Into` do not, so they must either invent a value or
            // panic, and both are the monopoly gone.
            if conversion.via == "TryFrom" {
                continue;
            }
            offenders.push(format!(
                "{path}: impl {}<{}> for {}",
                conversion.via, conversion.source, conversion.target
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "nothing may convert infallibly into `{GUARDED}`:\n{}\n\
         \n\
         An infallible conversion has nowhere to put a string that is not a \
         derived identifier, so it either invents one or panics. `derive` \
         computes an identifier from what it identifies; `from_wire` reads one \
         back and says so in its name. A `From<&str>` beside them is the third \
         path, and it is the one that gets used.",
        offenders.join("\n")
    );
}

#[test]
fn nothing_but_derive_and_from_wire_produces_one() {
    let mut offenders = Vec::new();

    for (path, file) in common::workspace_sources() {
        for function in common::functions(file.items()) {
            if function.returns.as_deref() != Some(GUARDED) {
                continue;
            }
            if !is_sanctioned(&function) {
                offenders.push(format!("{path}: fn {} -> {GUARDED}", function.path()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "only `{GUARDED}::derive` and `{GUARDED}::from_wire` may produce one:\n{}\n\
         \n\
         `From` was only ever the most likely spelling of the violation. A plain \
         `fn requirement_id_for(name: &str) -> RequirementId` is the identical \
         bypass with no trait involved, and it is what somebody writes when the \
         obvious spelling does not compile.",
        offenders.join("\n")
    );
}

// --- anti-vacuity ----------------------------------------------------------
//
// Each rule above is only as good as the reader beneath it, and a reader that
// silently matched nothing would pass forever. One meta-test per rule, planting
// the violation that rule exists to catch.

#[test]
fn the_reader_finds_the_ordinary_arm_too() {
    // Without this, `the_guarded_identifier_is_registered_…` would pass just as
    // happily if `id_newtype_invocations` reported *every* invocation as
    // `@derived_only`. The workspace registers ten identifiers the ordinary
    // way, and the reader has to be able to see the difference.
    let ordinary: Vec<String> = invocations()
        .into_iter()
        .filter(|(_, _, derived_only)| !derived_only)
        .flat_map(|(_, names, _)| names)
        .collect();

    assert!(
        ordinary.iter().any(|name| name == "CapabilityId")
            && ordinary.iter().any(|name| name == "RiskFlagId"),
        "the reader must distinguish the two arms; it reported these as ordinary: \
         {ordinary:?}"
    );
    assert!(
        !ordinary.iter().any(|name| name == GUARDED),
        "and `{GUARDED}` must not be among them"
    );
}

#[test]
fn the_reader_sees_a_planted_ordinary_registration() {
    let sample = common::read(
        "sample",
        r"
        id_newtype! {
            /// A doc comment, which is an attribute in the token stream.
            RequirementId
        }
        ",
    );
    assert_eq!(
        id_newtype_invocations(sample.items()),
        vec![(vec!["RequirementId".to_owned()], false)],
        "an invocation without `@derived_only` must read as the ordinary arm"
    );
}

#[test]
fn the_reader_sees_a_planted_conversion_and_a_planted_function() {
    let sample = common::read(
        "sample",
        r"
        impl From<&str> for RequirementId {}
        impl Into<RequirementId> for String {}
        impl TryFrom<u8> for RequirementId {}
        fn requirement_id_for(_name: &str) -> RequirementId { todo!() }
        ",
    );

    let conversions: Vec<String> = common::conversions(sample.items())
        .into_iter()
        .filter(|conversion| conversion.target == GUARDED)
        .map(|conversion| conversion.via)
        .collect();
    assert_eq!(
        conversions,
        ["From", "Into", "TryFrom"],
        "all three spellings reach the same target, and only `TryFrom` is exempt"
    );

    let producers: Vec<String> = common::functions(sample.items())
        .into_iter()
        .filter(|function| function.returns.as_deref() == Some(GUARDED))
        .filter(|function| !is_sanctioned(function))
        .map(|function| function.path())
        .collect();
    assert!(
        producers.contains(&"requirement_id_for".to_owned()),
        "a free function returning one wears no trait and must still be caught, \
         found {producers:?}"
    );
}
