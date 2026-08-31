//! Structural guards on the escalation accumulator.
//!
//! The claim that "verdicts only ever move up in scrutiny" rests on things that
//! are true of the *source files*, not of any value:
//!
//! 1. `escalate` is the only method that can mutate an `Adjudicator`.
//! 2. `adjudicate::accumulator` has no child modules.
//! 3. `adjudicate::enforcement` has no child modules either, for the same
//!    reason: `Adjudicators` holds its two ledgers as private fields, and a
//!    child could swap them or hand out the advisory one under another name.
//! 4. `Adjudicators::route` is not `pub`, so the advisory ledger is not
//!    nameable from outside this crate.
//! 5. `AdvisoryAdjudication` yields no verdict.
//!
//! The second one is easy to miss in review. Rust's field privacy is
//! module-scoped, so a private field is reachable from every descendant module —
//! adding `mod helpers;` inside `accumulator` would silently hand that module
//! write access to the tier, and nothing in the type system would object.
//!
//! None of these is expressible as a type, so they are asserted against the
//! text. A test that reads source is unusual and worth the oddity here: the
//! alternative is a comment asking reviewers to notice something subtle forever.
//!
//! # Two scanning regimes
//!
//! The guards on `enforcement.rs` prepare their source with
//! [`without_test_modules`], which excises `#[cfg(test)]` blocks by brace
//! matching. The older guards on `accumulator.rs` and `known.rs` still use
//! `non_test_source`, which *truncates* at the first `#[cfg(test)]` and so
//! cannot see anything written below it. That is a real gap and it belongs to
//! #35, which owns those two files; widening it here would put this lane's
//! changes in a file it was not scoped to rewrite. The new guards do not
//! inherit the gap, because the attack they defend against — a `mod helpers;`
//! with write access to the private `advisory` field — is written in exactly
//! the place truncation stops looking.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

const ACCUMULATOR: &str = include_str!("../src/adjudicate/accumulator.rs");
const ENFORCEMENT: &str = include_str!("../src/adjudicate/enforcement.rs");
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

// The scanning helpers below are duplicated from
// `tests/bundle_core_construction.rs`. Integration tests are separate binaries
// and cannot share code without a third file, which is outside this change's
// scope; #35 already owns both files and can lift them into a shared module
// then. Duplicated and sound beats shared and truncating.

/// The byte index just past a string, raw-string, or character literal
/// beginning at `at`, or `None` if one does not begin there.
///
/// `code_only` removes comments but not literals, and literals carry braces:
/// `write!(f, "flag:{flag}")`, `find('{')`, and `r#"{"path":"a.rs"}"#` all put a
/// brace in the source that opens or closes nothing. Counting those would
/// unbalance the scan.
fn end_of_literal(source: &str, at: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    match bytes.get(at)? {
        // A raw string: `r"…"`, `r#"…"#`, and so on. No escape processing, so
        // the terminator is the quote followed by the same number of hashes.
        b'r' => {
            let mut hashes = 0usize;
            let mut index = at + 1;
            while bytes.get(index) == Some(&b'#') {
                hashes += 1;
                index += 1;
            }
            if bytes.get(index) != Some(&b'"') {
                return None;
            }
            let terminator = format!("\"{}", "#".repeat(hashes));
            let body = index + 1;
            let end = source[body..]
                .find(&terminator)
                .unwrap_or_else(|| panic!("unterminated raw string at byte {at}"));
            Some(body + end + terminator.len())
        }
        b'"' => {
            let mut index = at + 1;
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' => index += 1,
                    b'"' => return Some(index + 1),
                    _ => {}
                }
                index += 1;
            }
            panic!("unterminated string literal at byte {at}")
        }
        // `'a` is a lifetime and `'{'` is a character. Tell them apart by
        // looking for the closing quote where a character literal would put it.
        b'\'' => [3usize, 4]
            .into_iter()
            .find(|width| bytes.get(at + width - 1) == Some(&b'\''))
            .map(|width| at + width),
        _ => None,
    }
}

/// The byte index of the `}` closing the block that opens at `open`.
///
/// Panics on an imbalance rather than reporting one. A scanner that gives up
/// quietly discards the remainder of the file, and a guard that scanned nothing
/// passes — which is the silent pass these tests exist to refuse. Loud and
/// wrong is recoverable; quiet and green is not.
fn matching_brace(source: &str, open: usize) -> usize {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut index = open;

    while index < bytes.len() {
        if let Some(after) = end_of_literal(source, index) {
            index = after;
            continue;
        }
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                assert!(depth > 0, "unbalanced closing brace at byte {index}");
                depth -= 1;
                if depth == 0 {
                    return index;
                }
            }
            _ => {}
        }
        index += 1;
    }

    panic!("unbalanced braces from byte {open}; this scan cannot be trusted")
}

/// The source with every `#[cfg(test)]` block excised.
///
/// A fixture in a module's own tests is not part of anything anyone links
/// against, and forbidding them would only push fixtures into shapes that prove
/// less.
///
/// Excised by brace matching rather than by truncating at the first
/// `#[cfg(test)]`. Truncation leaves everything *below* a test module unscanned,
/// which is precisely where a second construction site — or a `mod helpers;`, or
/// a `pub fn route` — would be written by someone working around one of these
/// guards. Run after `code_only`, so a brace in a comment cannot mislead it.
fn without_test_modules(source: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0usize;

    while let Some(offset) = source[cursor..].find("#[cfg(test)]") {
        let start = cursor + offset;
        out.push_str(&source[cursor..start]);

        let open = source[start..]
            .find('{')
            .map(|index| start + index)
            .unwrap_or_else(|| panic!("`#[cfg(test)]` at byte {start} opens no block"));
        assert!(
            source[start..open].contains("mod "),
            "only `#[cfg(test)] mod` blocks are excised; the attribute at byte \
             {start} guards something else, and excising to the next brace would \
             silently remove unrelated code"
        );
        cursor = matching_brace(source, open) + 1;
    }

    out.push_str(&source[cursor..]);
    out
}

/// Whether the text ending just before a `for` opens an `impl` header.
fn is_impl_header(before: &str) -> bool {
    before
        .rfind("impl")
        .is_some_and(|at| !before[at..].contains(['{', '}', ';']))
}

/// The bodies of every `impl` block for `name` — inherent *and* trait.
///
/// Trait impls matter as much as inherent ones: `impl Default for BundleCore`
/// and `impl Default for AdvisoryAdjudication` are both written
/// `fn default() -> Self { Self { … } }`, which mentions neither the type's name
/// nor `impl <name> {`.
fn impl_blocks<'a>(code: &'a str, name: &str) -> Vec<&'a str> {
    let needle = format!("{name} {{");
    let mut blocks = Vec::new();
    let mut cursor = 0usize;

    while let Some(offset) = code[cursor..].find(&needle) {
        let start = cursor + offset;
        cursor = start + 1;

        let before = code[..start].trim_end();
        let inherent = before.ends_with("impl");
        let of_a_trait = before.strip_suffix("for").is_some_and(is_impl_header);
        if !inherent && !of_a_trait {
            continue;
        }

        let open = start + needle.len() - 1;
        blocks.push(&code[open + 1..matching_brace(code, open)]);
    }

    blocks
}

/// `enforcement.rs`, comments gone and its own tests excised.
fn enforcement_code() -> String {
    without_test_modules(&code_only(ENFORCEMENT))
}

/// Module declarations in already-prepared code.
fn module_declarations(code: &str) -> Vec<&str> {
    code.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("mod ") || line.starts_with("pub mod "))
        .collect()
}

/// Module declarations in a source file, via the truncating helper.
///
/// Retained for the `accumulator.rs` and `known.rs` guards only — see the note
/// on scanning regimes above.
fn submodules(source: &str) -> Vec<&str> {
    module_declarations(non_test_source(source))
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
fn the_enforcement_module_has_no_children() {
    // `Adjudicators` holds `enforcing` and `advisory` as private fields of the
    // same type. A child module could swap them, or return `&mut self.advisory`
    // from something called `integrity`. Either is a downward operation wearing
    // a different word, and nothing in the type system would object.
    //
    // Scanned with the excising helper, not the truncating one: a `mod helpers;`
    // written *below* `mod tests` would be invisible to truncation, and it would
    // still have write access to `advisory`.
    let code = enforcement_code();
    let declarations = module_declarations(&code);
    assert!(
        declarations.is_empty(),
        "`adjudicate::enforcement` must have no submodules, found: {declarations:#?}"
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
    let source = enforcement_code();
    assert!(
        !source.contains("pub fn route"),
        "`Adjudicators::route` must stay `pub(crate)`; routing belongs to \
         `CapabilityResolution::account`, which applies the policy-integrity \
         override before choosing a lane"
    );
    assert!(
        source.contains("pub(crate) fn route"),
        "sanity check: the method this test guards should exist"
    );
}

#[test]
fn the_advisory_adjudication_yields_no_verdict() {
    // `Adjudicator::finish` always sets `verdict: tier.verdict()`. An advisory
    // ledger that could hand out its `Adjudication` would therefore hand out a
    // second, more permissive verdict, and a bundle carrying two verdicts is a
    // bundle whose readers will disagree about what happened.
    //
    // Every impl block, not the first one found: a second inherent impl adding
    // `pub fn verdict` is as good as the first, and `impl From<AdvisoryAdjudication>
    // for Adjudication` hands out the same thing under a trait's name.
    let source = enforcement_code();
    let blocks = impl_blocks(&source, "AdvisoryAdjudication");

    for block in &blocks {
        for forbidden in ["fn verdict", "-> Adjudication", "Adjudication {"] {
            assert!(
                !block.contains(forbidden),
                "`AdvisoryAdjudication` must not expose `{forbidden}`; only the \
                 enforced ledger becomes a verdict, and a bundle carries exactly \
                 one"
            );
        }
    }

    // The other direction, which no `impl … for AdvisoryAdjudication` block
    // contains: a conversion *out of* an advisory ledger into an `Adjudication`
    // hands a caller the verdict just as effectively, under a trait's name.
    assert!(
        !source.contains("From<AdvisoryAdjudication>"),
        "nothing may convert an `AdvisoryAdjudication` into an `Adjudication`; \
         that type always carries a verdict derived from its tier, and the \
         advisory tier is not allowed to become one"
    );

    assert_eq!(
        blocks.len(),
        1,
        "sanity check: the one impl block this test scans should have been found"
    );
    assert!(
        blocks[0].contains("fn tier"),
        "sanity check: and it should be the one declaring `tier`"
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
fn the_excising_scanner_sees_below_a_test_module() {
    // The reason the enforcement guards do not use `non_test_source`. A child
    // module written below `mod tests` has exactly the same write access to the
    // private `advisory` field as one written above it, and truncation cannot
    // see it at all. This test is what stops the enforcement guards quietly
    // regressing onto the truncating helper.
    let sample = r"
mod above;
#[cfg(test)]
mod tests {
    fn fixture() {}
}
mod below;
";

    assert_eq!(
        submodules(sample),
        ["mod above;"],
        "truncation stops at the test module — this is the gap, recorded"
    );
    assert_eq!(
        module_declarations(&without_test_modules(&code_only(sample))),
        ["mod above;", "mod below;"],
        "excision sees both, which is why the enforcement guards use it"
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
