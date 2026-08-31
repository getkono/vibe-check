//! The model crate may not read a clock, and the absence is asserted here.
//!
//! `DecisionTime` is built so that the only instant a decision can depend on is
//! a commit's committer date: no `now()`, no `Default`, no `From<Timestamp>`.
//! That argument holds only while nothing *else* in this crate reads the
//! current moment — a single `Timestamp::now()` somewhere in `resolution.rs`
//! would make the type a formality, because a caller could then get a fresh
//! instant without ever naming one.
//!
//! # What this catches, and what the lint catches
//!
//! `clippy.toml` is the first line of defence and the more complete one. It
//! bans the *types* `std::time::SystemTime` and `std::time::Instant`, which is
//! what closes the spellings that never say `now`: `UNIX_EPOCH.elapsed()`,
//! `duration_since(UNIX_EPOCH)`, and whatever the next one turns out to be. It
//! bans `Timestamp::now`, `Zoned::now`, and `TimeZone::system`/`try_system`,
//! the `TZ` read that can move a civil date by a day.
//!
//! This test is the second line, and it is deliberately not a superset. It
//! reaches exactly one thing the lint does not: **a name that resolves to no
//! listed path.** A `fn now` this crate defines itself hands every caller the
//! shortcut `DecisionTime` refuses to provide, and `clippy.toml` cannot ban a
//! path that does not exist yet. Both a declaration and a call count.
//!
//! That is the whole of the residual, and two tempting additions to it are not
//! real. Declarations are not a reason to scan text rather than parse it: an
//! AST walk sees a `fn` item at least as well, and sees `now ()` and
//! `now::<T>()` without being taught to. Nor are macro bodies: an AST walk over
//! expressions would indeed lose a call inside one, but clippy resolves paths
//! after expansion and was measured firing on `format!("{}", Timestamp::now())`
//! at the call site's own column, so the lint already covers it.
//!
//! It is vacuous today, deliberately. The model crate contains no such call and
//! none of its six dependencies would tempt one. It ships as a prospective
//! guard, in the style of `no_evidence_from_status`: the change it exists to
//! stop is one that nothing else fails on, that makes the type checker no
//! unhappier, and that reads in review as a small convenience.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use camino::{Utf8Path, Utf8PathBuf};

/// Type names that may not appear in this crate at all.
///
/// Both are banned as *types* by `clippy.toml` for the same reason, and are
/// repeated here so that this test's name is true on its own: a guard called
/// `the_model_crate_never_reads_a_clock` that only knows the word `now` goes
/// green while `SystemTime::UNIX_EPOCH.elapsed()` sits in the file, and a green
/// assertion that a thing cannot happen while it is happening is worse than no
/// assertion, because the green is what a reviewer trusts.
const FORBIDDEN_IDENTIFIERS: [&str; 2] = ["SystemTime", "Instant"];

/// The source with comment and string-literal *contents* blanked out.
///
/// Blanked rather than deleted: every byte is replaced by a space and every
/// newline is preserved, so offsets and line numbers still refer to the file as
/// written and a failure can name the line a human will find.
///
/// Masking both is required, not cosmetic. This crate argues about clocks in
/// prose and in error messages — `time.rs` names `now()` several times
/// explaining why it is absent, and the expiry work #32 defers will ship user-
/// facing text like "decisions read the committer date, not now()". A scanner
/// over raw text would report the sentences that state the rule as violations
/// of it, and the fix a hurried author reaches for is to reword the message.
/// That is how a guard earns a reputation for being worked around rather than
/// obeyed.
///
/// Handles line comments, ordinary and raw string literals, and character
/// literals — the last only so that `'"'` cannot open a string that runs to the
/// end of the file. A `'` that is not a complete character literal is a
/// lifetime and is left alone. Block comments are masked too, nesting-aware:
/// the workspace contains none, but nothing enforces that — neither
/// `clippy.toml` nor `rustfmt.toml` bans them, and review alone is what keeps
/// it true.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut index = 0usize;

    while index < chars.len() {
        let character = chars[index];

        if character == '/' && chars.get(index + 1) == Some(&'/') {
            while index < chars.len() && chars[index] != '\n' {
                out.push(' ');
                index += 1;
            }
            continue;
        }

        if character == '/' && chars.get(index + 1) == Some(&'*') {
            index = block_comment(&chars, index, &mut out);
            continue;
        }

        if let Some(after) = string_literal(&chars, index, &mut out) {
            index = after;
            continue;
        }

        if character == '\''
            && let Some(after) = character_literal(&chars, index, &mut out)
        {
            index = after;
            continue;
        }

        out.push(character);
        index += 1;
    }

    out
}

/// Whether `character` can appear inside an identifier.
fn identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Whether the character before `index` can be part of an identifier.
fn preceded_by_identifier(chars: &[char], index: usize) -> bool {
    index > 0 && identifier_char(chars[index - 1])
}

/// Whether the name at `start..end` in `line` stands alone as an identifier.
///
/// So `snow(` and `nowhere(` are not `now`, and `SystemTimeish` is not
/// `SystemTime`.
fn stands_alone(line: &str, start: usize, end: usize) -> bool {
    !line[..start]
        .chars()
        .next_back()
        .is_some_and(identifier_char)
        && !line[end..].chars().next().is_some_and(identifier_char)
}

/// Blank a string literal of any prefix, returning the index just past it.
///
/// Rust spells strings six ways — `"…"`, `b"…"`, `c"…"`, `r"…"`, `br"…"`,
/// `cr"…"` — and the byte and C prefixes matter here for a reason that is not
/// tidiness. `b` and `c` are identifier characters, so a scanner that only
/// recognises a bare `r` treats `br"…"` as an identifier followed by an
/// *ordinary* string, and then applies escape semantics that a raw string does
/// not have. In `br"C:\path\"` the trailing backslash eats the closing quote,
/// the mask runs on to the next `"` or to the end of the file, and every line
/// it swallows is a line this guard no longer reads. That is the one failure
/// direction that matters: it makes the test go green over a real clock read.
fn string_literal(chars: &[char], index: usize, out: &mut String) -> Option<usize> {
    let character = *chars.get(index)?;

    if character == '"' {
        return Some(ordinary_string(chars, index, out));
    }
    if !matches!(character, 'b' | 'c' | 'r') || preceded_by_identifier(chars, index) {
        return None;
    }

    let after_prefix = if character == 'r' { index } else { index + 1 };
    if chars.get(after_prefix) == Some(&'r') {
        return raw_string(chars, index, after_prefix, out);
    }
    if chars.get(after_prefix) != Some(&'"') {
        return None;
    }

    for _ in index..after_prefix {
        out.push(' ');
    }
    Some(ordinary_string(chars, after_prefix, out))
}

/// Blank a nesting-aware `/* … */` comment, returning the index just past it.
///
/// The workspace contains none today. It is handled anyway because the
/// alternative is a false positive on `/* never call now() here */`, and a
/// false positive teaches an author to reword the prose rather than obey the
/// rule — the exact incentive the masking above exists to remove.
fn block_comment(chars: &[char], index: usize, out: &mut String) -> usize {
    let mut depth = 0usize;
    let mut cursor = index;

    while cursor < chars.len() {
        let pair = (chars[cursor], chars.get(cursor + 1).copied());
        match pair {
            ('/', Some('*')) => depth += 1,
            ('*', Some('/')) => depth = depth.saturating_sub(1),
            _ => {
                out.push(if chars[cursor] == '\n' { '\n' } else { ' ' });
                cursor += 1;
                continue;
            }
        }
        out.push(' ');
        out.push(' ');
        cursor += 2;
        if depth == 0 {
            return cursor;
        }
    }

    cursor
}

/// Blank the raw-string body of a literal whose prefix begins at `start` and
/// whose `r` sits at `r_at`, returning the index just past it.
///
/// `None` when what follows the `r` is not a literal after all, which is how an
/// identifier such as `br` or `render` falls through to be scanned as code.
fn raw_string(chars: &[char], start: usize, r_at: usize, out: &mut String) -> Option<usize> {
    let mut hashes = 0usize;
    let mut cursor = r_at + 1;
    while chars.get(cursor) == Some(&'#') {
        hashes += 1;
        cursor += 1;
    }
    if chars.get(cursor) != Some(&'"') {
        return None;
    }

    for _ in start..=cursor {
        out.push(' ');
    }
    cursor += 1;

    while cursor < chars.len() {
        if chars[cursor] == '"' && (1..=hashes).all(|n| chars.get(cursor + n) == Some(&'#')) {
            for _ in 0..=hashes {
                out.push(' ');
            }
            return Some(cursor + hashes + 1);
        }
        out.push(if chars[cursor] == '\n' { '\n' } else { ' ' });
        cursor += 1;
    }

    Some(cursor)
}

/// Blank a `"…"` literal, returning the index just past it.
///
/// Newlines survive, including the one after a `\` line continuation, because
/// the assertion messages in this workspace are written that way and losing
/// them would shift every line number reported after the first long message.
fn ordinary_string(chars: &[char], index: usize, out: &mut String) -> usize {
    out.push(' ');
    let mut cursor = index + 1;

    while cursor < chars.len() {
        match chars[cursor] {
            '\\' => {
                out.push(' ');
                if let Some(escaped) = chars.get(cursor + 1) {
                    out.push(if *escaped == '\n' { '\n' } else { ' ' });
                }
                cursor += 2;
            }
            '"' => {
                out.push(' ');
                return cursor + 1;
            }
            '\n' => {
                out.push('\n');
                cursor += 1;
            }
            _ => {
                out.push(' ');
                cursor += 1;
            }
        }
    }

    cursor
}

/// Blank a `'x'` or `'\n'` literal, returning the index just past it.
///
/// `None` for a lifetime, which is every other use of `'` in Rust and must be
/// left in place.
fn character_literal(chars: &[char], index: usize, out: &mut String) -> Option<usize> {
    let end = if chars.get(index + 1) == Some(&'\\') {
        (index + 3..=index + 8).find(|at| chars.get(*at) == Some(&'\''))?
    } else if chars.get(index + 2) == Some(&'\'') {
        index + 2
    } else {
        return None;
    };

    for _ in index..=end {
        out.push(' ');
    }
    Some(end + 1)
}

/// The 1-based line numbers on which this crate reads a clock.
fn wall_clock_lines(source: &str) -> Vec<usize> {
    code_only(source)
        .lines()
        .enumerate()
        .filter(|(_, line)| calls_or_declares_now(line) || names_a_clock_type(line))
        .map(|(index, _)| index + 1)
        .collect()
}

/// Whether a line calls or declares something named `now`.
///
/// Tolerant of the spellings rustfmt and generics produce: `now()`, `now ()`,
/// and `now::<Utc>()` all count, as does `fn now(`. The name must stand alone,
/// so `snow(` and `nowhere(` do not.
fn calls_or_declares_now(line: &str) -> bool {
    let mut cursor = 0usize;

    while let Some(offset) = line[cursor..].find("now") {
        let start = cursor + offset;
        let end = start + "now".len();
        cursor = end;

        if line[..start]
            .chars()
            .next_back()
            .is_some_and(identifier_char)
        {
            continue;
        }

        let mut rest = line[end..].trim_start();
        if let Some(turbofish) = rest.strip_prefix("::<") {
            let Some(close) = turbofish.find('>') else {
                continue;
            };
            rest = turbofish[close + 1..].trim_start();
        }
        if rest.starts_with('(') {
            return true;
        }
    }

    false
}

/// Whether a line names a type that is a clock.
fn names_a_clock_type(line: &str) -> bool {
    FORBIDDEN_IDENTIFIERS.iter().any(|identifier| {
        let mut cursor = 0usize;
        while let Some(offset) = line[cursor..].find(identifier) {
            let start = cursor + offset;
            let end = start + identifier.len();
            cursor = end;

            if stands_alone(line, start, end) {
                return true;
            }
        }
        false
    })
}

/// Every `.rs` file under this crate's `src`, in a stable order.
///
/// Library sources only. This file lives in `tests/`, so it is outside its own
/// scan by construction — which is what lets the samples below spell out the
/// calls they forbid in order to prove the scanner sees them.
fn model_sources() -> Vec<(Utf8PathBuf, String)> {
    let src = Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    let mut files = Vec::new();
    collect_rs(&src, &mut files);
    files.sort();

    assert!(
        files.len() > 5,
        "expected the model crate's sources under {src}, found {} — if the \
         layout moved, this test is scanning nothing and proving nothing",
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

/// Recursively collect `.rs` paths under `dir`.
///
/// Via camino, so a source file whose path is not UTF-8 is skipped loudly by
/// the walk rather than lossily renamed — the same reason the workspace bans
/// `std::path` outright. Directory order is filesystem-dependent, which is why
/// `read_dir` is disallowed elsewhere; the caller sorts so that a failure names
/// files in the same order on every machine.
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
fn the_model_crate_never_reads_a_clock() {
    let mut offenders = Vec::new();

    for (path, source) in model_sources() {
        for line in wall_clock_lines(&source) {
            offenders.push(format!("{path}:{line}"));
        }
    }

    assert!(
        offenders.is_empty(),
        "the model crate may not read the current moment:\n{}\n\
         \n\
         Time-dependent decisions compare against the head commit's committer \
         date, carried as `DecisionTime`, so that re-evaluating last month's \
         pull request gives the verdict it had. A verdict that changes because \
         of when it was asked cannot be replayed, cannot be audited, and cannot \
         be appealed.\n\
         \n\
         The wall clock is read in exactly one place in this workspace: \
         `vibe-check-host`'s `clock` module, whose every output is display-only \
         and on the digest's exclusion list. If a decision here needs a time, it \
         needs a `DecisionTime` threaded in from the caller.",
        offenders.join("\n")
    );
}

#[test]
fn the_scanner_sees_the_calls_that_matter() {
    // The specific changes this file exists to prevent, run through the scanner
    // to prove the assertion above is reachable rather than vacuous.
    //
    // The macro line is kept for a narrower reason than it once claimed: the
    // lint already catches it, but it is the spelling a *future* AST guard
    // would lose, since a macro body reaches such a walk as an unparsed token
    // stream. The `br"…"` line below it is the one that swallowed a real clock
    // read before `string_literal` learned the byte and C prefixes.
    let sample = r##"
let at = jiff::Timestamp::now();
let zoned = jiff::Zoned::now();
let since = std::time::SystemTime::UNIX_EPOCH.elapsed();
let measured = Instant::now();
tracing::info!("generated at {}", Timestamp::now());
let path = br"C:\path\";
let t = Timestamp::now();
"##;

    assert_eq!(wall_clock_lines(sample), vec![2, 3, 4, 5, 6, 8]);
}

#[test]
fn the_scanner_sees_the_spellings_rustfmt_and_generics_produce() {
    let sample = r##"
let spaced = Timestamp::now ();
let turbofished = Clock::now::<Utc>();
fn now() -> Self {}
"##;

    assert_eq!(wall_clock_lines(sample), vec![2, 3, 4]);
}

#[test]
fn the_zone_read_is_the_lints_job_and_not_this_scanners() {
    // Recorded rather than fixed. `TimeZone::system()` reads the runner's `TZ`
    // and can move a civil date by a day, which is why `clippy.toml` bans it
    // and `try_system` beside it. This scanner does not see it, and should not
    // try: `system` is too common a word to match on text, and the lint
    // resolves the path exactly. Asserted so that a later reader learns the
    // division of labour here rather than assuming a hole.
    let sample = r##"
let local = at.to_zoned(jiff::tz::TimeZone::system()).date();
"##;

    assert!(
        wall_clock_lines(sample).is_empty(),
        "if this ever starts failing, the scanner grew a `system` rule and this \
         test should become an assertion that it fires"
    );
}

#[test]
fn the_scanner_ignores_prose_and_lookalike_identifiers() {
    // Every line here is a false positive the scanner used to produce, or would
    // produce without the masking above. The trailing comment and the strings
    // are the ones that matter: rewording an error message must never be the
    // way to make this test pass.
    let sample = r####"
/// There is deliberately no `now()` on this type.
// let evaded = Timestamp::now();
let a = 1; // there is no now() here
panic!("never call now() in the model");
let wrapped = format!(
    "decisions read the committer date, \
     not now(), and never SystemTime"
);
let raw = r#"now() and SystemTime in a raw string"#;
let nested = r###"now() inside a nested raw string"###;
let bytes = b"now() in a byte string";
let cstr = c"now() in a C string";
/* never call now() here, and never SystemTime */
/* nested /* now() */ still a comment */
fn snow(depth: u8) {}
let known = knowns.get(key);
struct Now(Timestamp);
let quote = '"';
fn borrow<'a>(now: &'a str) {}
"####;

    assert!(
        wall_clock_lines(sample).is_empty(),
        "prose, error messages, raw and byte and C strings, block comments \
         including nested ones, a char literal holding a quote, and \
         identifiers that merely contain those letters are not clock reads; \
         found lines {:?}",
        wall_clock_lines(sample)
    );
}

#[test]
fn masking_preserves_line_numbers() {
    // A failure that names the wrong line sends a reader to the wrong place,
    // and multi-line strings are how the numbering slips.
    let sample = r##"
let message = "a long assertion \
     wrapped across lines";
let at = Timestamp::now();
"##;

    assert_eq!(wall_clock_lines(sample), vec![4]);
    assert_eq!(code_only(sample).lines().count(), sample.lines().count());
}
