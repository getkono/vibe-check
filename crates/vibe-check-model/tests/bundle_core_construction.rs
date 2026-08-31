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
//! So the constraint is asserted against the source text of the whole
//! workspace, the way `accumulator_invariants` and `no_evidence_from_status`
//! assert the other properties that are not expressible as types.
//!
//! Separate from `accumulator_invariants` on purpose: that file is about the
//! adjudicator's shape, this one is about the bundle's, and they fail for
//! unrelated reasons.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use camino::{Utf8Path, Utf8PathBuf};

/// Drop comment lines. This file's own subject is spelled out in prose in
/// `bundle.rs`, which would otherwise report itself.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

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

/// Whether `Name {` at `start` opens a struct literal.
///
/// The same three tokens appear in three other roles, and none of them
/// constructs anything: `struct Name {` declares it, `impl Name {` opens an impl
/// block, and `-> Name {` is a return type followed by the function's own
/// opening brace. Missing that last one would make every function returning a
/// `BundleCore` report itself.
fn is_literal_position(code: &str, start: usize) -> bool {
    let before = code[..start].trim_end();
    !(before.ends_with("struct") || before.ends_with("impl") || before.ends_with("->"))
}

/// Offsets of every `BundleCore {` that opens a struct literal.
fn literal_offsets(code: &str) -> Vec<usize> {
    let needle = "BundleCore {";
    let mut found = Vec::new();
    let mut cursor = 0usize;

    while let Some(offset) = code[cursor..].find(needle) {
        let start = cursor + offset;
        cursor = start + needle.len();

        if is_literal_position(code, start) {
            found.push(start);
        }
    }

    found
}

/// Offsets of every `Self {` that opens a struct literal, by the same rules.
fn self_literal_offsets(code: &str) -> Vec<usize> {
    let needle = "Self {";
    let mut found = Vec::new();
    let mut cursor = 0usize;

    while let Some(offset) = code[cursor..].find(needle) {
        let start = cursor + offset;
        cursor = start + needle.len();

        if is_literal_position(code, start) {
            found.push(start);
        }
    }

    found
}

/// The name of the function containing the byte at `offset`.
fn enclosing_fn(code: &str, offset: usize) -> Option<&str> {
    let start = code[..offset].rfind("fn ")? + "fn ".len();
    let rest = &code[start..];
    let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_')?;
    Some(&rest[..end])
}

/// Every `.rs` file under `crates/*/src`, in a stable order.
///
/// The same walk as `no_evidence_from_status`: library sources only, sorted, and
/// with a sanity assertion so that a moved layout fails loudly instead of
/// scanning nothing and passing.
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
/// `no_evidence_from_status` does: a non-UTF-8 path is skipped loudly rather
/// than lossily renamed.
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
fn a_bundle_core_is_constructed_in_exactly_one_place() {
    let mut sites = Vec::new();

    for (path, source) in workspace_sources() {
        let code = without_test_modules(&code_only(&source));
        for offset in literal_offsets(&code) {
            sites.push((
                path.clone(),
                enclosing_fn(&code, offset).unwrap_or("<none>").to_owned(),
            ));
        }
    }

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

    let (path, function) = &sites[0];
    assert!(
        path.ends_with("vibe-check-model/src/bundle.rs"),
        "the sole construction site must be in `bundle.rs`, found {path}"
    );
    assert_eq!(
        function, "new",
        "the sole construction site must be `BundleCore::new`"
    );
}

#[test]
fn nothing_else_can_construct_one_under_another_name() {
    // `Self { .. }` inside any `impl` *for* `BundleCore` is the same
    // construction written in a way the scan above cannot see, because it
    // mentions neither `BundleCore {` nor `impl BundleCore {`.
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
    let mut offenders = Vec::new();
    let mut scanned = 0usize;

    for (path, source) in workspace_sources() {
        let code = without_test_modules(&code_only(&source));
        for block in impl_blocks(&code, "BundleCore") {
            scanned += 1;
            if !self_literal_offsets(block).is_empty() {
                offenders.push(path.clone());
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "an `impl` for `BundleCore` must not contain a `Self {{` literal: \
         {offenders:#?}\n\
         \n\
         Write the type out by name so the single-construction-site guard can \
         see it — or, better, call `BundleCore::new`, which is the only place \
         the enforced and advisory tiers are known to be the right way round."
    );
    assert_eq!(
        scanned, 1,
        "sanity check: exactly the one inherent `impl BundleCore` should have \
         been found and scanned"
    );
}

#[test]
fn the_scanner_actually_finds_literals() {
    // These guarantees are only as good as the parsing above, and a scanner that
    // silently matched nothing would pass every test here forever.
    let sample = "\
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
";
    let code = without_test_modules(&code_only(sample));
    let offsets = literal_offsets(&code);

    assert_eq!(
        offsets.len(),
        2,
        "the scanner must see both literals, and none of the declaration, the \
         impl header, or `elsewhere`'s return type"
    );
    assert_eq!(enclosing_fn(&code, offsets[0]), Some("new"));
    assert_eq!(enclosing_fn(&code, offsets[1]), Some("elsewhere"));
    assert_eq!(
        self_literal_offsets(&code).len(),
        0,
        "`-> Self {{` is a return type, not a second constructor"
    );
}

#[test]
fn the_brace_matcher_is_not_fooled_by_literals() {
    // `code_only` strips comments, not literals, and the workspace is full of
    // braces inside strings: `write!(f, "flag:{flag}")`, `find('{')`, and
    // `r#"{"path":"a.rs"}"#`. Counting any one of them would unbalance the scan,
    // and an unbalanced scan silently discards the rest of the file.
    let sample = "\
fn noisy() {
    let a = \"{ unclosed in a string\";
    let b = '{';
    let c = r#\"{\"path\":\"a.rs\"}\"#;
    let d = \"escaped quote \\\" and a brace {\";
}
";
    let open = sample.find('{').expect("the fn body opens");
    let close = matching_brace(sample, open);

    assert_eq!(
        &sample[close..=close],
        "}",
        "the matcher must land on the brace closing `noisy`"
    );
    assert!(
        sample[close..].trim() == "}",
        "and it must be the last one in the sample"
    );
}

#[test]
fn the_impl_scanner_sees_trait_impls() {
    let sample = "\
impl BundleCore {
    fn new() -> Self { BundleCore { tier } }
}
impl Default for BundleCore {
    fn default() -> Self { Self { tier } }
}
impl Default for SomethingElse {
    fn default() -> Self { Self { tier } }
}
";
    let blocks = impl_blocks(sample, "BundleCore");

    assert_eq!(blocks.len(), 2, "the inherent impl and the trait impl");
    assert!(blocks[1].contains("Self { tier }"));
    assert_eq!(
        impl_blocks(sample, "SomethingElse").len(),
        1,
        "and it must not confuse one type's impls for another's"
    );
}

#[test]
fn a_fixture_in_a_modules_own_tests_is_not_a_construction_site() {
    // And — the reason this excises rather than truncates — a construction site
    // written *below* a test module is still found. Truncating at the first
    // `#[cfg(test)]` would leave `sneaky` unscanned, which is precisely where a
    // second constructor would end up if someone were avoiding this test.
    let sample = "\
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
";
    let code = without_test_modules(&code_only(sample));
    let offsets = literal_offsets(&code);

    assert_eq!(offsets.len(), 2, "the two non-test sites, and only those");
    assert_eq!(enclosing_fn(&code, offsets[0]), Some("real"));
    assert_eq!(enclosing_fn(&code, offsets[1]), Some("sneaky"));
}
