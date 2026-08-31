//! Structural guards on the escalation accumulator.
//!
//! The claim that "verdicts only ever move up in scrutiny" rests on two things
//! that are true of the *source file*, not of any value:
//!
//! 1. `escalate` is the only method that can mutate an `Adjudicator`.
//! 2. `adjudicate::accumulator` has no child modules.
//!
//! The second one is easy to miss in review. Rust's field privacy is
//! module-scoped, so a private field is reachable from every descendant module —
//! adding `mod helpers;` inside `accumulator` would silently hand that module
//! write access to the tier, and nothing in the type system would object.
//!
//! Neither property is expressible as a type, so they are asserted against the
//! text. A test that reads source is unusual and worth the oddity here: the
//! alternative is a comment asking reviewers to notice something subtle forever.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

const ACCUMULATOR: &str = include_str!("../src/adjudicate/accumulator.rs");
const KNOWN: &str = include_str!("../src/known.rs");

/// Everything before `#[cfg(test)]`, so a module's own tests do not count.
fn non_test_source(source: &str) -> &str {
    source
        .split_once("#[cfg(test)]")
        .map_or(source, |(before, _)| before)
}

/// Collapse whitespace, so a signature rustfmt wrapped across several lines
/// reads the same as one written on a single line.
fn normalized(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Drop comment lines.
///
/// The documentation on `Adjudicator` lists the escape hatches it deliberately
/// does *not* provide — `DerefMut`, `set_tier`, and so on. Scanning raw text
/// would flag the sentence explaining the rule as a violation of it.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Names of functions whose parameter list takes `&mut self`.
fn mutating_method_names(source: &str) -> Vec<String> {
    normalized(source)
        .split("fn ")
        .skip(1)
        .filter_map(|rest| {
            let params_end = rest.find(')')?;
            let (head, params) = rest.split_at(params_end);
            let _ = params;
            let name = head.split('(').next()?.trim().to_owned();
            head.contains("&mut self").then_some(name)
        })
        .collect()
}

/// The body of a named struct declaration.
fn struct_body<'a>(source: &'a str, name: &str) -> &'a str {
    let needle = format!("pub struct {name} {{");
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("`{name}` must be declared in this file"))
        + needle.len();
    let rest = &source[start..];
    let end = rest.find('}').expect("struct declaration is closed");
    &rest[..end]
}

/// Module declarations in a source file.
fn submodules(source: &str) -> Vec<&str> {
    non_test_source(source)
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("mod ") || line.starts_with("pub mod "))
        .collect()
}

#[test]
fn escalate_is_the_only_mutator_on_the_adjudicator() {
    let mutators = mutating_method_names(non_test_source(ACCUMULATOR));

    assert_eq!(
        mutators,
        ["escalate"],
        "exactly one method may take `&mut self`\n\
         \n\
         Adding another is how \"scrutiny only rises\" stops being a property of \
         the API and becomes a rule someone has to keep enforcing. If new mutable \
         behaviour is genuinely needed, express it in terms of `escalate` rather \
         than alongside it."
    );
}

#[test]
fn the_accumulator_module_has_no_children() {
    // A child module would inherit access to the private `tier` field.
    let declarations = submodules(ACCUMULATOR);
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
fn the_known_module_has_no_children() {
    // Same reasoning: a child of `known` could match on the private `Inner` enum
    // and read an unresolved identifier without escalating, which is the one
    // thing `Known::get` exists to prevent.
    let declarations = submodules(KNOWN);
    assert!(
        declarations.is_empty(),
        "`known` must have no submodules, found: {declarations:#?}"
    );
}

#[test]
fn the_adjudicators_tier_field_is_private() {
    // Scoped to `Adjudicator` deliberately. `Adjudication` — the finished,
    // consumed-by-value result — *does* expose `tier` publicly, and that is
    // fine: it has no mutator at all, so a public field on it cannot be used to
    // lower anything.
    let body = struct_body(non_test_source(ACCUMULATOR), "Adjudicator");
    assert!(
        !body.contains("pub tier"),
        "`Adjudicator::tier` must stay private; a public field can be assigned, \
         and assignment is exactly the operation the lattice forbids"
    );
    assert!(
        body.contains("tier: Tier"),
        "sanity check: the field this test guards should exist"
    );
}

#[test]
fn the_adjudicator_exposes_no_escape_hatches() {
    let source = code_only(non_test_source(ACCUMULATOR));
    for forbidden in ["fn set_tier", "fn tier_mut", "DerefMut", "impl AsMut"] {
        assert!(
            !source.contains(forbidden),
            "`{forbidden}` would allow a tier to be assigned rather than joined, \
             which makes lowering a verdict expressible"
        );
    }
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
    let source = code_only(non_test_source(ACCUMULATOR));
    assert!(
        !source.contains("impl Default for Adjudicator"),
        "`Adjudicator` must not implement `Default`; the type's own documentation \
         explains why, and an impl that contradicts its documentation is worse \
         than either one alone"
    );
}

#[test]
fn the_signature_scanner_actually_works() {
    // This file's guarantees are only as good as the parsing above, and a
    // scanner that silently matches nothing would pass every test here.
    let sample = r"
        impl T {
            pub fn read(&self) -> u8 { 0 }
            pub fn write(
                &mut self,
                value: u8,
            ) {}
            fn helper(&mut self) {}
        }
    ";
    assert_eq!(mutating_method_names(sample), ["write", "helper"]);
}
