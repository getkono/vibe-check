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

/// The source with every `#[cfg(test)]` block excised.
///
/// A fixture that builds a bundle in a module's own tests is not a second
/// construction site in anything anyone links against, and forbidding them
/// would only push fixtures into shapes that prove less.
///
/// Excised by brace matching rather than by truncating at the first
/// `#[cfg(test)]`, which is what `accumulator_invariants` does. That file scans
/// one hand-chosen source; this one scans every crate, and truncation would
/// leave everything *after* a test module unscanned — so a second construction
/// site written below one would be invisible to the guard that exists to find
/// it. Run after `code_only`, so a brace inside a comment cannot confuse the
/// matching.
fn without_test_modules(source: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0usize;

    while let Some(offset) = source[cursor..].find("#[cfg(test)]") {
        let start = cursor + offset;
        out.push_str(&source[cursor..start]);

        let Some(open) = source[start..].find('{').map(|index| start + index) else {
            return out;
        };
        let Some(close) = matching_brace(source, open) else {
            return out;
        };
        cursor = close + 1;
    }

    out.push_str(&source[cursor..]);
    out
}

/// The `}` closing the block that opens at `open`.
fn matching_brace(source: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in source[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
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

/// Drop comment lines. This file's own subject is spelled out in prose in
/// `bundle.rs`, which would otherwise report itself.
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
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
    // `Self { .. }` inside an `impl BundleCore` block is the same construction
    // written in a way the scan above cannot see. There is one such block, in
    // `bundle.rs`, and `new` writes the type out by name — so a `Self` literal
    // anywhere in it is a second constructor.
    let mut blocks = Vec::new();

    for (path, source) in workspace_sources() {
        let code = without_test_modules(&code_only(&source));
        let mut cursor = 0usize;
        while let Some(offset) = code[cursor..].find("impl BundleCore {") {
            let start = cursor + offset;
            cursor = start + 1;
            let rest = &code[start..];
            let end = rest.find("\n}").unwrap_or(rest.len());
            blocks.push((path.clone(), rest[..end].to_owned()));
        }
    }

    assert_eq!(
        blocks.len(),
        1,
        "there must be exactly one `impl BundleCore` block, found: {:#?}",
        blocks.iter().map(|(path, _)| path).collect::<Vec<_>>()
    );

    let (path, block) = &blocks[0];
    let disguised = self_literal_offsets(block);
    assert!(
        disguised.is_empty(),
        "{path}: `impl BundleCore` must not contain a `Self {{` literal; write \
         the type out by name so the guard above can see it"
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
