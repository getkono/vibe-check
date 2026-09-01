//! Source-reading helpers shared by the structural guard tests.
//!
//! Several of this workspace's load-bearing claims are not expressible as
//! types — "`escalate` is the only mutator", "nothing converts a check run into
//! evidence", "a `BundleCore` is built in exactly one place". They are asserted
//! against the workspace's own sources instead, which makes the *reader* of
//! those sources part of the guarantee.
//!
//! # Why this parses instead of scanning
//!
//! Every one of those guards used to match substrings, and a substring match is
//! bypassed by rewording rather than by argument:
//!
//! - `fn with_tier(mut self, t: Tier) -> Self` is a consuming builder that
//!   writes the tier, and it contains no `&mut self`.
//! - `impl TryFrom<CheckRun> for Evidence` and `impl Into<Evidence> for CheckRun`
//!   are the forbidden conversion, and neither is spelled `impl From<… for
//!   Evidence`.
//! - `impl From<CheckRun> for &Evidence` extracts an empty target under a scan
//!   that stops at the first non-path character.
//!
//! None of those is exotic; each is what someone reaches for when the obvious
//! spelling does not compile. So the guards read the same syntax tree the
//! compiler does.
//!
//! # And what a syntax tree does not give for free
//!
//! Parsing has blind spots a text scan did not have, and every one of them is a
//! way to write a shipped item that a naive tree walk never visits. This module
//! closes them deliberately, because each was a real bypass:
//!
//! 1. **`#[cfg(not(test))]`.** "Skip anything whose `cfg` mentions `test`" also
//!    skips the negation — which is the form that ships. [`ships`] evaluates the
//!    predicate instead, with `test` off, so `cfg(test)` is dropped and
//!    `cfg(not(test))` is kept.
//! 2. **`#[cfg(not(<anything else>))]`.** Evaluating the predicate is only
//!    enough while the evaluation is honest about what it does not know.
//!    Recording an unevaluable leaf as "conservatively satisfied" made
//!    `not(target_os = "windows")` — which is how portable code is written —
//!    come out false, and every item behind one left the walk. [`Cfg::holds`]
//!    is three-valued so that unknown survives a negation, and the
//!    conservative reading happens once, in [`ships`].
//! 3. **Item bodies.** `const _: () = { impl From<String> for Evidence {…} };`
//!    registers that impl globally, and it is nested inside a `const`
//!    initialiser rather than at the top level of a file. The walk descends into
//!    every body, not only into modules.
//! 4. **Macro bodies.** `syn` hands a `macro_rules!` definition back as opaque
//!    tokens, so an `impl` written inside one is invisible — while the old text
//!    scan could still see it, which made a naive parse a *regression*. The
//!    expansions are re-parsed here, with metavariables substituted for plain
//!    identifiers, and an expansion that cannot be parsed at all is a loud
//!    failure rather than a silent skip.
//! 5. **Macro invocations.** Substituting a placeholder answers the wrong
//!    question when the metavariable stands where the rule keys:
//!    `impl From<$src> for $dst` re-parses to a target no rule names, and the
//!    real name is written only at the invocation — which `syn` also hands
//!    back as opaque tokens. So the invocations' arguments are collected,
//!    pooled across the workspace, and substituted in as well. See
//!    [`substitute`].
//! 6. **A written name is not a type.** Every rule here keys on an identifier,
//!    and a type alias, a tuple and an associated type are three ways to hand
//!    back the forbidden value while writing a different one. The first two are
//!    resolved — see [`resolve_alias`] for exactly how far, and for what is
//!    deliberately left alone.
//!
//! A guard that stops catching something silently is worse than no guard: the
//! green is what a reviewer trusts. The corollary is that a guard which fails
//! loudly for a reason nobody can act on gets deleted, which is the same
//! outcome by a slower route — so the loud failures are kept narrow: the reader
//! stops on what it cannot place *and* that could hold an item, and passes over
//! the legal shapes that hold nothing.
//!
//! # Integration tests cannot share code without a directory
//!
//! Each file in `tests/` is its own binary. `tests/common/mod.rs` is the one
//! shape Cargo does not compile as a fourth test target, so this is where the
//! shared readers live; each guard declares `mod common;`.

// Each guard binary uses a different subset of these helpers, and an unused
// helper in one binary is not dead code — it is load-bearing in another.
#![allow(dead_code)]
// A guard that cannot read its subject must fail loudly. A reader that gives up
// quietly returns nothing, and a guard over nothing passes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use camino::{Utf8Path, Utf8PathBuf};
use proc_macro2::{Delimiter, Group, Ident, Span, TokenStream, TokenTree};
use syn::parse::{Parse, ParseStream};

/// Type constructors that do not change what a value *is*.
///
/// `Box<Evidence>`, `Option<Evidence>`, `&Evidence` and `Result<Evidence, _>`
/// are all ways of handing someone an `Evidence`, so a guard on `Evidence` has
/// to see through them.
///
/// `Result` and its aliases are here because the one place unwrapping them
/// would have been a false positive is sanctioned by name instead:
/// `ForgeRead::download` is the only honest source of an `Artifact`, and it is
/// listed as such. Nothing in the workspace returns `Result<Evidence, _>`, so leaving
/// `Result` opaque bought nothing and cost the guard the most obvious bypass
/// of all — the sanctioned function's own signature, made fallible.
const TRANSPARENT: [&str; 7] = ["Box", "Arc", "Rc", "Option", "Vec", "Result", "ForgeResult"];

/// How a function takes `self`, if it does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Receiver {
    /// No receiver: a free function, or an associated function like `new`.
    None,
    /// `&self`.
    Ref,
    /// `&mut self`.
    RefMut,
    /// `self` or `mut self` — taken by value, and therefore able to return a
    /// modified copy. This is the consuming-builder shape.
    Value,
}

/// Visibility as written, reduced to the three cases the guards ask about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vis {
    /// `pub` — nameable from outside the crate.
    Public,
    /// `pub(crate)`, `pub(super)`, `pub(in …)`.
    Restricted,
    /// No visibility keyword.
    Inherited,
}

/// A function or method, with the `impl` or `trait` it was found in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Function {
    /// Its name.
    pub name: String,
    /// Base identifier of the type the enclosing `impl` is for, if any.
    pub owner: Option<String>,
    /// Base identifier of the trait it belongs to — whether as one of that
    /// trait's `impl`s or as the declaration in the trait itself.
    pub owner_trait: Option<String>,
    /// Visibility as written.
    pub visibility: Vis,
    /// How it takes `self`.
    pub receiver: Receiver,
    /// Base identifiers of the parameters taken as `&mut T`, receiver excluded.
    ///
    /// A free function is not a method, and `fn set_tier(a: &mut Adjudicator,
    /// t: Tier)` therefore has no receiver at all — yet module-scoped field
    /// privacy lets it assign a private field just as effectively. A rule about
    /// receivers cannot see it; this is what does.
    pub mutably_borrows: Vec<String>,
    /// Base identifier of the return type, with `Self` resolved to [`owner`]
    /// and any same-file type alias followed to the name it stands for.
    ///
    /// The first of [`produces`], and the same thing whenever the return type
    /// names one component. A rule that asks "does this return an `Evidence`"
    /// wants [`produces`]; this is for the messages and for the cases where a
    /// single answer is the honest one.
    ///
    /// [`owner`]: Function::owner
    /// [`produces`]: Function::produces
    pub returns: Option<String>,
    /// Every base identifier the return type could hand back.
    ///
    /// A tuple returns each of its elements, so `-> (Evidence, u8)` produces an
    /// `Evidence` — and pairing the forbidden value with a throwaway is
    /// otherwise a one-character escape from any rule that keys on the return
    /// type.
    pub produces: Vec<String>,
}

impl Function {
    /// A readable path for a failure message: `Type::name` or `name`.
    #[must_use]
    pub fn path(&self) -> String {
        match &self.owner {
            Some(owner) => format!("{owner}::{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// One conversion `impl` between two types.
///
/// `From`, `TryFrom` and `Into` are normalized into the same direction, so a
/// guard states its rule once instead of three times: `impl Into<Evidence> for
/// CheckRun` and `impl From<CheckRun> for Evidence` produce the same value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Conversion {
    /// Which trait spelled it: `From`, `TryFrom`, or `Into`.
    pub via: String,
    /// Base identifier of the type converted *from*.
    pub source: String,
    /// Base identifier of the type converted *to*.
    pub target: String,
}

impl Conversion {
    /// The impl as it would read in source, in the trait's own direction.
    #[must_use]
    pub fn rendered(&self) -> String {
        if self.via == "Into" {
            format!("impl Into<{}> for {}", self.target, self.source)
        } else {
            format!("impl {}<{}> for {}", self.via, self.source, self.target)
        }
    }
}

/// One struct literal, with the function that contains it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Literal {
    /// Base identifier of the type constructed, with `Self` resolved to the
    /// enclosing `impl`'s type.
    pub type_name: String,
    /// The name of the innermost enclosing function, or `<none>`.
    pub function: String,
    /// Whether it was written `Self { … }` rather than by name.
    pub written_as_self: bool,
}

// --- `cfg` evaluation ------------------------------------------------------

/// A `cfg` predicate, reduced to what these guards need to decide.
enum Cfg {
    /// The bare `test` predicate.
    Test,
    /// `not(…)`.
    Not(Box<Cfg>),
    /// `all(…)`.
    All(Vec<Cfg>),
    /// `any(…)`.
    Any(Vec<Cfg>),
    /// Anything else — `feature = "e2e"`, `unix`, `debug_assertions`.
    ///
    /// *Unknown*, and deliberately not the same thing as `false`. An item
    /// behind one ships in *some* configuration, so a guard that skipped it
    /// would be a guard someone could hide behind a feature flag — but
    /// recording that as the two-valued `true` is worse than the hole it
    /// closes, because `not(…)` then inverts it into `false` and the item
    /// vanishes anyway. See [`Cfg::holds`].
    Other,
}

impl Parse for Cfg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let path: syn::Path = input.parse()?;
        let name = path.get_ident().map(unraw);

        if input.peek(syn::token::Paren) {
            let inner;
            syn::parenthesized!(inner in input);
            let list = inner.parse_terminated(Cfg::parse, syn::Token![,])?;
            let mut list: Vec<Cfg> = list.into_iter().collect();
            return match name.as_deref() {
                Some("not") if list.len() == 1 => Ok(Cfg::Not(Box::new(
                    list.pop().expect("length was just checked"),
                ))),
                Some("not") => Err(input.error("`not` takes exactly one predicate")),
                Some("all") => Ok(Cfg::All(list)),
                Some("any") => Ok(Cfg::Any(list)),
                _ => Ok(Cfg::Other),
            };
        }

        if input.peek(syn::Token![=]) {
            let _: syn::Token![=] = input.parse()?;
            let _: syn::Expr = input.parse()?;
            return Ok(Cfg::Other);
        }

        Ok(if name.as_deref() == Some("test") {
            Cfg::Test
        } else {
            Cfg::Other
        })
    }
}

/// The whole token stream inside one `#[cfg(…)]`.
///
/// An attribute's argument list takes a trailing comma the way every other
/// Rust list does: `#[cfg(test,)]` compiles, and is configured out of an
/// ordinary build exactly like `#[cfg(test)]`. Parsing a single predicate and
/// nothing else left the comma unconsumed, `syn::parse2` rejected the
/// leftovers, and [`ships`] panicked on legal source — while the nested
/// `#[cfg(all(test,))]` was accepted, because the inner list *is* read with
/// `parse_terminated`. That asymmetry reads as an accident rather than a rule,
/// and a guard that dies on legal code is a guard someone deletes.
struct CfgAttr(Cfg);

impl Parse for CfgAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let list = syn::punctuated::Punctuated::<Cfg, syn::Token![,]>::parse_terminated(input)?;
        let mut list: Vec<Cfg> = list.into_iter().collect();
        if list.len() != 1 {
            return Err(input.error("`cfg` takes exactly one predicate"));
        }
        Ok(Self(list.pop().expect("length was just checked")))
    }
}

impl Cfg {
    /// Whether the predicate holds in an ordinary build — the one that produces
    /// the artifact people link against, in which `test` is off.
    ///
    /// Three-valued, and that is the whole point. `Some(true)` and
    /// `Some(false)` are decided; `None` is *unknown*, which is what a
    /// predicate this reader cannot evaluate actually is.
    ///
    /// Storing unknown as `true` — "conservatively satisfied" — is only
    /// conservative until something negates it. `#[cfg(not(target_os =
    /// "windows"))]` ships on every machine this workspace is built on, and
    /// under two-valued evaluation it read as `!true`, so every item behind one
    /// disappeared from every guard in this directory. The same held for
    /// `#[cfg(not(feature = "e2e"))]` and for `all(not(feature = "x"), unix)`.
    /// That is a bypass an attacker does not even have to be clever to find:
    /// it is the ordinary way to write portable code.
    ///
    /// So unknown propagates instead of collapsing. `not` of unknown is
    /// unknown. `all` is `Some(false)` as soon as one conjunct is decidedly
    /// false — that item really is configured out — and unknown otherwise if
    /// anything in it was unknown. `any` is the mirror image. [`ships`] then
    /// reads unknown as shipping, which is the conservative direction *stated
    /// once, at the end*, rather than smuggled into a leaf where a negation can
    /// reach it.
    fn holds(&self) -> Option<bool> {
        match self {
            Cfg::Test => Some(false),
            Cfg::Not(inner) => inner.holds().map(|value| !value),
            Cfg::All(list) => {
                let mut unknown = false;
                for predicate in list {
                    match predicate.holds() {
                        Some(false) => return Some(false),
                        Some(true) => {}
                        None => unknown = true,
                    }
                }
                (!unknown).then_some(true)
            }
            Cfg::Any(list) => {
                let mut unknown = false;
                for predicate in list {
                    match predicate.holds() {
                        Some(true) => return Some(true),
                        Some(false) => {}
                        None => unknown = true,
                    }
                }
                (!unknown).then_some(false)
            }
            Cfg::Other => None,
        }
    }
}

/// Whether an item carrying these attributes is compiled into an ordinary
/// build.
///
/// This is *not* "does the `cfg` mention `test`". `#[cfg(not(test))]` mentions
/// it and ships in every non-test build; treating it as test-only was a hole
/// wide enough to hide a `From<String> for Evidence` in.
///
/// A predicate this reader cannot evaluate is unknown rather than false, and an
/// item behind an unknown gate is read as shipping — it ships in *some*
/// configuration, and the guards are about what can reach a build, not about
/// what reaches this one. The conservative choice is made here, once, on the
/// three-valued answer [`Cfg::holds`] returns, because making it inside the
/// predicate put it somewhere `not(…)` could invert.
///
/// # Panics
///
/// If a `cfg` predicate cannot be parsed. A guard that does not understand a
/// gate must not guess which way it falls — guessing "test-only" is how an item
/// disappears from a guard without anyone noticing.
#[must_use]
pub fn ships(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().all(|attr| {
        let syn::Meta::List(list) = &attr.meta else {
            return true;
        };
        if !list.path.is_ident("cfg") {
            return true;
        }
        let predicate: CfgAttr = syn::parse2(list.tokens.clone()).unwrap_or_else(|error| {
            panic!(
                "a `#[cfg(…)]` this guard cannot classify is a gate it cannot \
                 reason about, and guessing is how an item vanishes from a \
                 guard silently: {error}"
            )
        });
        predicate.0.holds() != Some(false)
    })
}

/// An identifier's name with any raw-identifier prefix removed.
///
/// `#[cfg(r#test)]` is `#[cfg(test)]`: rustc compares the symbol, and the `r#`
/// is lexical syntax rather than part of the name. `Ident::to_string` keeps the
/// prefix, so a comparison against `"test"` failed and the item read as
/// shipping. That over-reports rather than under-reports — the safe direction —
/// but a guard whose safety rests on a spelling accident is one refactor away
/// from resting on nothing.
fn unraw(ident: &Ident) -> String {
    ident.to_string().trim_start_matches("r#").to_owned()
}

// --- reading a source file -------------------------------------------------

/// One source file, reduced to what actually ships.
///
/// Built by [`read`]. Holds the items in the file — inline modules flattened,
/// item bodies descended into, `#[cfg]`-excluded items dropped, and
/// `macro_rules!` expansions parsed and folded in — plus the expansions that
/// are expressions rather than item lists, which declare nothing but can still
/// construct something.
pub struct Source {
    label: String,
    file: syn::File,
    arguments: Arguments,
    macro_files: Vec<syn::File>,
    macro_exprs: Vec<syn::Expr>,
    items: Vec<syn::Item>,
}

/// The identifiers each macro is invoked with, keyed by the macro's name.
///
/// `BTreeMap`/`BTreeSet` rather than the hashed pair: iteration order reaches a
/// failure message, and the workspace bans the hashed containers outright for
/// the same reason.
pub type Arguments = std::collections::BTreeMap<String, std::collections::BTreeSet<String>>;

impl Source {
    /// Every item that ships, flattened.
    #[must_use]
    pub fn items(&self) -> &[syn::Item] {
        &self.items
    }

    /// The identifiers this file passes to each macro it invokes.
    #[must_use]
    pub fn arguments(&self) -> &Arguments {
        &self.arguments
    }

    /// Re-read the file, expanding its `macro_rules!` bodies against arguments
    /// gathered from somewhere other than this file too.
    ///
    /// A `macro_rules!` definition and the invocation that binds its
    /// metavariables need not share a file: `#[macro_use]` and
    /// `#[macro_export]` both carry one across. Reading each file alone would
    /// therefore see `impl From<$src> for $dst` as a conversion into
    /// `metavar_dst` — a type nothing forbids — while the crate really contains
    /// whatever the invocation named.
    pub fn expand_against(&mut self, arguments: &Arguments) {
        let mut merged = self.arguments.clone();
        for (macro_name, idents) in arguments {
            merged
                .entry(macro_name.clone())
                .or_default()
                .extend(idents.iter().cloned());
        }
        if merged == self.arguments {
            return;
        }
        let rebuilt = build(self.label.clone(), self.file.clone(), merged);
        self.arguments = rebuilt.arguments;
        self.macro_files = rebuilt.macro_files;
        self.macro_exprs = rebuilt.macro_exprs;
        self.items = rebuilt.items;
    }
}

/// Parse Rust source, naming `label` if it does not parse.
///
/// # Panics
///
/// If `source` is not valid Rust. A guard that could not read its subject must
/// fail; the alternative is a green test over an empty parse.
#[must_use]
pub fn parse(label: &str, source: &str) -> syn::File {
    syn::parse_file(source)
        .unwrap_or_else(|error| panic!("`{label}` must parse as Rust, but did not: {error}"))
}

/// Parse Rust source and reduce it to what ships.
#[must_use]
pub fn read(label: &str, source: &str) -> Source {
    let file = parse(label, source);
    // The invocations have to be in hand before the definitions are expanded,
    // because they are what the metavariables stand for. Two passes over one
    // parse, not two parses.
    let arguments = invocation_arguments(&file);
    build(label.to_owned(), file, arguments)
}

/// Walk `file`, expanding each `macro_rules!` body against `arguments`.
fn build(label: String, file: syn::File, arguments: Arguments) -> Source {
    let mut walk = ItemWalk {
        label: label.clone(),
        arguments,
        items: Vec::new(),
        macro_files: Vec::new(),
        macro_exprs: Vec::new(),
    };
    syn::visit::Visit::visit_file(&mut walk, &file);

    // A macro expansion is source too, and it can define macros of its own.
    let mut index = 0usize;
    while index < walk.macro_files.len() {
        let expansion = walk.macro_files[index].clone();
        syn::visit::Visit::visit_file(&mut walk, &expansion);
        index += 1;
    }

    Source {
        label,
        file,
        arguments: walk.arguments,
        macro_files: walk.macro_files,
        macro_exprs: walk.macro_exprs,
        items: walk.items,
    }
}

/// The workspace root, derived from this crate's manifest directory.
#[must_use]
pub fn workspace_root() -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Utf8Path::parent)
        .expect("the model crate sits two levels below the workspace root")
        .to_owned()
}

/// Every `.rs` file under `crates/*/src`, read, in a stable order.
///
/// Library sources only. An `impl` written under `tests/` exists in a test
/// binary and not in the crate anyone links against — and the guards' own
/// samples, which spell out the forbidden shapes in order to prove the readers
/// see them, would otherwise report themselves.
///
/// # Panics
///
/// If the layout moved. A walk that finds nothing would make every guard over
/// it pass while proving nothing, so the floor is asserted rather than assumed.
#[must_use]
pub fn workspace_sources() -> Vec<(Utf8PathBuf, Source)> {
    let root = workspace_root();

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

    let mut sources: Vec<(Utf8PathBuf, Source)> = files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("a listed source file is readable");
            let source = read(path.as_str(), &text);
            (path, source)
        })
        .collect();

    // A `macro_rules!` definition and the invocation that binds its
    // metavariables are not obliged to share a file — `#[macro_use]` and
    // `#[macro_export]` are exactly the features that separate them. Reading
    // each file in isolation would leave `impl From<$src> for $dst` looking
    // like a conversion into `metavar_dst`, which nothing forbids, so the
    // arguments are pooled across the workspace and every definition is
    // expanded again against all of them.
    let mut pooled = Arguments::new();
    for (_, source) in &sources {
        for (macro_name, idents) in source.arguments() {
            pooled
                .entry(macro_name.clone())
                .or_default()
                .extend(idents.iter().cloned());
        }
    }
    for (_, source) in &mut sources {
        source.expand_against(&pooled);
    }

    sources
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

/// Recursively collect `.rs` paths under `dir`.
///
/// Via camino, so a source file whose path is not UTF-8 is skipped loudly by
/// the walk rather than lossily renamed — the same reason the workspace bans
/// `std::path` outright.
///
/// Directory order is filesystem-dependent, which is why `read_dir` is
/// disallowed elsewhere. It does not reach a verdict from here, but the caller
/// sorts anyway so that a failure names files in the same order on every
/// machine.
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

// --- macro bodies ----------------------------------------------------------

/// One rule of a `macro_rules!` body.
struct Rule {
    /// The token group that follows the `=>`.
    expansion: TokenStream,
    /// The metavariables this rule matches that could stand for a *type name*:
    /// the ones declared `:ty`, `:ident` or `:path`.
    ///
    /// Restricted to those three on purpose. Every rule in this directory keys
    /// on a written type — the `for` target of a conversion, a return type, the
    /// name in a struct literal — and no other fragment specifier can supply
    /// one. `$core:expr` cannot be the target of a `From` impl, so binding it
    /// to each identifier its invocations pass would only manufacture copies of
    /// the same expansion for a guard to count twice.
    names: std::collections::BTreeSet<String>,
}

/// Each rule of a `macro_rules!` body: the group after each `=>`, and the
/// naming metavariables the matcher before it declares.
fn rules(body: &TokenStream) -> Vec<Rule> {
    let trees: Vec<TokenTree> = body.clone().into_iter().collect();
    let mut out = Vec::new();

    for index in 0..trees.len() {
        let (TokenTree::Punct(first), Some(TokenTree::Punct(second))) =
            (&trees[index], trees.get(index + 1))
        else {
            continue;
        };
        if first.as_char() != '=' || second.as_char() != '>' {
            continue;
        }
        let Some(TokenTree::Group(expansion)) = trees.get(index + 2) else {
            continue;
        };
        let names = match index.checked_sub(1).and_then(|before| trees.get(before)) {
            Some(TokenTree::Group(matcher)) => naming_metavariables(&matcher.stream()),
            _ => std::collections::BTreeSet::new(),
        };
        out.push(Rule {
            expansion: expansion.stream(),
            names,
        });
    }

    out
}

/// The `$name:ty`, `$name:ident` and `$name:path` metavariables a matcher
/// declares, by name.
fn naming_metavariables(matcher: &TokenStream) -> std::collections::BTreeSet<String> {
    let trees: Vec<TokenTree> = matcher.clone().into_iter().collect();
    let mut out = std::collections::BTreeSet::new();

    for index in 0..trees.len() {
        if let TokenTree::Group(group) = &trees[index] {
            out.extend(naming_metavariables(&group.stream()));
            continue;
        }
        let TokenTree::Punct(dollar) = &trees[index] else {
            continue;
        };
        if dollar.as_char() != '$' {
            continue;
        }
        let (Some(TokenTree::Ident(name)), Some(TokenTree::Punct(colon))) =
            (trees.get(index + 1), trees.get(index + 2))
        else {
            continue;
        };
        if colon.as_char() != ':' {
            continue;
        }
        let Some(TokenTree::Ident(fragment)) = trees.get(index + 3) else {
            continue;
        };
        if matches!(unraw(fragment).as_str(), "ty" | "ident" | "path") {
            out.insert(unraw(name));
        }
    }

    out
}

/// Whether any of `names` is actually written as a metavariable in `stream`.
///
/// A rule that declares `$t:ty` and never uses it expands identically however
/// `$t` is bound, so binding it would only duplicate the expansion.
fn mentions_any(stream: &TokenStream, names: &std::collections::BTreeSet<String>) -> bool {
    let trees: Vec<TokenTree> = stream.clone().into_iter().collect();
    for index in 0..trees.len() {
        match &trees[index] {
            TokenTree::Group(group) => {
                if mentions_any(&group.stream(), names) {
                    return true;
                }
            }
            TokenTree::Punct(punct) if punct.as_char() == '$' => {
                if let Some(TokenTree::Ident(name)) = trees.get(index + 1)
                    && names.contains(&unraw(name))
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Replace macro metavariables so an expansion parses.
///
/// With `binding` absent, `$name` becomes `metavar_name` and the result is a
/// *representative* of every expansion: `impl From<$name> for Evidence` is the
/// forbidden impl in each of the eleven types that macro is invoked for,
/// whatever those types are, because the rule keys on the target and the target
/// is written out.
///
/// With `binding` present, every metavariable becomes that identifier instead.
/// That is what the representative cannot do, and it is not a nicety: when the
/// metavariable sits *where the rule keys*, the placeholder answers the wrong
/// question.
///
/// ```ignore
/// macro_rules! conv { ($src:ty => $dst:ty) => { impl From<$src> for $dst {…} }; }
/// conv!(String => Evidence);
/// ```
///
/// The representative reads as `impl From<metavar_src> for metavar_dst`, whose
/// target is not `Evidence`, so nothing fires — while the crate really does
/// contain `impl From<String> for Evidence`. The invocation is the only place
/// the real name appears, and `syn` hands the invocation back as opaque tokens
/// too, so the guard never saw it anywhere.
///
/// So every identifier any invocation of that macro passes is substituted in
/// turn, one expansion each. It over-approximates — it does not track *which*
/// metavariable an argument binds, and an argument that only ever appears in
/// one position is tried in all of them — and over-approximating is the
/// direction to be wrong in: the cost is a spurious `impl From<CapabilityId>
/// for CapabilityId` that no rule mentions, and the alternative is a real
/// `impl From<String> for Evidence` that no rule sees.
///
/// Only the metavariables named in [`Binding::names`] take the identifier;
/// everything else still becomes a placeholder, so an `$expr` keeps standing
/// for an expression rather than being rewritten into a type name.
fn substitute(stream: TokenStream, binding: Option<&Binding<'_>>) -> TokenStream {
    let trees: Vec<TokenTree> = stream.into_iter().collect();
    let mut out: Vec<TokenTree> = Vec::new();
    let mut index = 0usize;

    while index < trees.len() {
        match &trees[index] {
            TokenTree::Punct(punct) if punct.as_char() == '$' => {
                match trees.get(index + 1) {
                    Some(TokenTree::Ident(name)) => {
                        let replacement = match binding {
                            Some(bound) if bound.names.contains(&unraw(name)) => {
                                bound.ident.to_owned()
                            }
                            _ => format!("metavar_{name}"),
                        };
                        out.push(TokenTree::Ident(Ident::new(
                            &replacement,
                            Span::call_site(),
                        )));
                        index += 2;
                    }
                    Some(TokenTree::Group(group)) => {
                        out.extend(substitute(group.stream(), binding));
                        index += 2;
                        // Drop an optional separator and the repetition
                        // operator that follow: `$( … ),*` and `$( … )*` alike.
                        if let Some(TokenTree::Punct(next)) = trees.get(index)
                            && !matches!(next.as_char(), '*' | '+' | '?')
                            && matches!(
                                trees.get(index + 1),
                                Some(TokenTree::Punct(after))
                                    if matches!(after.as_char(), '*' | '+' | '?')
                            )
                        {
                            index += 1;
                        }
                        if let Some(TokenTree::Punct(next)) = trees.get(index)
                            && matches!(next.as_char(), '*' | '+' | '?')
                        {
                            index += 1;
                        }
                    }
                    _ => index += 1,
                }
            }
            TokenTree::Group(group) => {
                out.push(TokenTree::Group(Group::new(
                    group.delimiter(),
                    substitute(group.stream(), binding),
                )));
                index += 1;
            }
            other => {
                out.push(other.clone());
                index += 1;
            }
        }
    }

    out.into_iter().collect()
}

/// One identifier an invocation passed, and the metavariables it may stand for.
struct Binding<'a> {
    names: &'a std::collections::BTreeSet<String>,
    ident: &'a str,
}

/// Every identifier in a token stream, however deeply nested.
fn identifiers(stream: &TokenStream, out: &mut std::collections::BTreeSet<String>) {
    for tree in stream.clone() {
        match tree {
            TokenTree::Ident(ident) => {
                out.insert(unraw(&ident));
            }
            TokenTree::Group(group) => identifiers(&group.stream(), out),
            _ => {}
        }
    }
}

/// The identifiers passed to each macro `file` invokes.
///
/// Invocations only — the `macro_rules!` definitions themselves are skipped,
/// and so is anything a `#[cfg]` keeps out of an ordinary build, since an
/// argument passed only under `#[cfg(test)]` binds nothing in the artifact
/// people link against.
fn invocation_arguments(file: &syn::File) -> Arguments {
    let mut walk = InvocationWalk {
        found: Arguments::new(),
    };
    syn::visit::Visit::visit_file(&mut walk, file);
    walk.found
}

/// Collects macro invocation arguments, honouring `#[cfg]`.
struct InvocationWalk {
    found: Arguments,
}

impl<'ast> syn::visit::Visit<'ast> for InvocationWalk {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !ships(attrs_of(item)) {
            return;
        }
        syn::visit::visit_item(self, item);
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        if !ships(&function.attrs) {
            return;
        }
        syn::visit::visit_impl_item_fn(self, function);
    }

    fn visit_trait_item_fn(&mut self, function: &'ast syn::TraitItemFn) {
        if !ships(&function.attrs) {
            return;
        }
        syn::visit::visit_trait_item_fn(self, function);
    }

    // One hook covers every position a macro can be invoked from — item,
    // expression, type, statement, pattern, impl member. Naming them
    // individually is how one gets forgotten.
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let Some(name) = last_ident(&mac.path) else {
            return;
        };
        if name == "macro_rules" {
            return;
        }
        let entry = self.found.entry(name).or_default();
        identifiers(&mac.tokens, entry);
    }
}

// --- the walk --------------------------------------------------------------

/// Collects every shipped item, descending into bodies and macro definitions.
struct ItemWalk {
    label: String,
    arguments: Arguments,
    items: Vec<syn::Item>,
    macro_files: Vec<syn::File>,
    macro_exprs: Vec<syn::Expr>,
}

impl ItemWalk {
    /// Re-parse a `macro_rules!` definition's expansions.
    ///
    /// Each rule is re-parsed once with its metavariables replaced by
    /// placeholders — which is what catches a forbidden name written out in the
    /// body — and once per identifier any invocation of that macro passes,
    /// which is what catches a forbidden name the body only refers to through a
    /// metavariable. See [`substitute`] for why both are needed.
    ///
    /// # Panics
    ///
    /// If an expansion parses in none of the positions a macro may legally
    /// expand into *and* contains a token that could introduce an item. `syn`
    /// treats a macro body as opaque, so the alternative is to skip it — and
    /// skipping is what let `impl From<$name> for Evidence` hide inside
    /// `id_newtype!`.
    fn expand(&mut self, definition: &syn::ItemMacro) {
        if definition.ident.is_none() || !definition.mac.path.is_ident("macro_rules") {
            return;
        }
        let name = definition
            .ident
            .as_ref()
            .map_or_else(|| "<anonymous>".to_owned(), ToString::to_string);

        let arguments: Vec<String> = self
            .arguments
            .get(&name)
            .map(|idents| idents.iter().cloned().collect())
            .unwrap_or_default();

        for rule in rules(&definition.mac.tokens) {
            let bound: Vec<Binding<'_>> = if mentions_any(&rule.expansion, &rule.names) {
                arguments
                    .iter()
                    .map(|ident| Binding {
                        names: &rule.names,
                        ident,
                    })
                    .collect()
            } else {
                Vec::new()
            };

            for binding in std::iter::once(None).chain(bound.iter().map(Some)) {
                let tokens = substitute(rule.expansion.clone(), binding);
                if self.absorb(&tokens) {
                    continue;
                }
                // The placeholder pass is the one that has to be readable: it
                // is the shape written in the source. A binding pass can fail
                // to parse because the argument was never valid in that
                // position — `conv!(String => Evidence)` substituted into a
                // pattern, say — and that is an artefact of the
                // over-approximation, not a hole.
                if binding.is_some() {
                    continue;
                }
                if !declares_anything(&tokens) {
                    // A macro that expands to a type, a field list, a match
                    // arm or a where-clause predicate declares nothing these
                    // guards ask about, and the triad of file/expression/block
                    // reaches none of those positions. `pub type Facts =
                    // list!(EvidenceFacts);` is ordinary stable Rust, and
                    // panicking on it made two *unrelated* guards explode with
                    // a message about hidden `impl`s — which is the shape of
                    // failure that gets a guard deleted rather than fixed.
                    continue;
                }

                panic!(
                    "`{}`: the expansion of `{name}!` could not be re-parsed, so \
                     anything written inside it is invisible to these guards. A \
                     macro body is where an `impl` hides from a syntax tree; teach \
                     `substitute` the shape it uses rather than letting the guard \
                     pass over it.",
                    self.label
                );
            }
        }
    }

    /// Take an expansion in whichever position it parses, or report failure.
    fn absorb(&mut self, tokens: &TokenStream) -> bool {
        if let Ok(file) = syn::parse2::<syn::File>(tokens.clone()) {
            self.macro_files.push(file);
            return true;
        }
        if let Ok(expr) = syn::parse2::<syn::Expr>(tokens.clone()) {
            self.macro_exprs.push(expr);
            return true;
        }
        let braced: TokenStream =
            TokenTree::Group(Group::new(Delimiter::Brace, tokens.clone())).into();
        if let Ok(block) = syn::parse2::<syn::Block>(braced) {
            self.macro_exprs.push(syn::Expr::Block(syn::ExprBlock {
                attrs: Vec::new(),
                label: None,
                block,
            }));
            return true;
        }
        // A match arm is the one declaration-free position with an expression
        // inside it, and an expression is where a struct literal lives. Keeping
        // the bodies is what stops `($p:pat) => { $p => BundleCore { … } }`
        // from being recognised and then thrown away.
        if let Ok(arms) = syn::parse::Parser::parse2(
            syn::punctuated::Punctuated::<syn::Arm, syn::Token![,]>::parse_terminated,
            tokens.clone(),
        ) {
            self.macro_exprs
                .extend(arms.into_iter().map(|arm| *arm.body));
            return true;
        }
        parses_as_a_position_that_declares_nothing(tokens)
    }
}

/// Whether a token stream parses in one of the macro-expansion positions that
/// cannot declare an item.
///
/// A macro may legally expand into far more than items, expressions and blocks:
/// a type, a struct's fields, an enum's variants, a where-clause predicate, a
/// pattern, a bound. None of those can carry an `impl`, a `fn` or a `struct`,
/// so there is nothing in them for these guards to find — match arms are
/// handled by [`ItemWalk::absorb`] itself, because an arm's body is an
/// expression and an expression can construct something. But a
/// reader that could not parse them at all panicked on legal source, and the
/// panic surfaced on whichever guard binary happened to read the file. Being
/// able to *name* the position is what turns "I cannot read this" into "there
/// is nothing here to read".
fn parses_as_a_position_that_declares_nothing(tokens: &TokenStream) -> bool {
    use syn::punctuated::Punctuated;

    syn::parse2::<syn::Type>(tokens.clone()).is_ok()
        || syn::parse2::<syn::FieldsNamed>(tokens.clone()).is_ok()
        || syn::parse2::<syn::FieldsUnnamed>(tokens.clone()).is_ok()
        || syn::parse2::<syn::Visibility>(tokens.clone()).is_ok()
        || syn::parse::Parser::parse2(
            |input: ParseStream| {
                Punctuated::<syn::Field, syn::Token![,]>::parse_terminated_with(
                    input,
                    syn::Field::parse_named,
                )
            },
            tokens.clone(),
        )
        .is_ok()
        || syn::parse::Parser::parse2(
            Punctuated::<syn::Variant, syn::Token![,]>::parse_terminated,
            tokens.clone(),
        )
        .is_ok()
        || syn::parse::Parser::parse2(
            Punctuated::<syn::WherePredicate, syn::Token![,]>::parse_terminated,
            tokens.clone(),
        )
        .is_ok()
        || syn::parse::Parser::parse2(
            Punctuated::<syn::TypeParamBound, syn::Token![+]>::parse_terminated,
            tokens.clone(),
        )
        .is_ok()
        || syn::parse::Parser::parse2(syn::Pat::parse_single, tokens.clone()).is_ok()
}

/// Whether a token stream contains a keyword that could introduce something a
/// guard in this directory asks about.
///
/// The backstop under [`parses_as_a_position_that_declares_nothing`]. An
/// expansion this reader can place in no position at all is only a *silence*
/// worth panicking over if there is something in it to be silent about; these
/// are the words under which an `impl`, a producer or a construction site can
/// live.
fn declares_anything(stream: &TokenStream) -> bool {
    stream.clone().into_iter().any(|tree| match tree {
        TokenTree::Ident(ident) => matches!(
            unraw(&ident).as_str(),
            "impl" | "fn" | "struct" | "enum" | "union" | "trait" | "macro_rules"
        ),
        TokenTree::Group(group) => declares_anything(&group.stream()),
        _ => false,
    })
}

impl<'ast> syn::visit::Visit<'ast> for ItemWalk {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !ships(attrs_of(item)) {
            return;
        }
        self.items.push(item.clone());
        if let syn::Item::Macro(definition) = item {
            self.expand(definition);
        }
        syn::visit::visit_item(self, item);
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        if !ships(&function.attrs) {
            return;
        }
        syn::visit::visit_impl_item_fn(self, function);
    }

    fn visit_trait_item_fn(&mut self, function: &'ast syn::TraitItemFn) {
        if !ships(&function.attrs) {
            return;
        }
        syn::visit::visit_trait_item_fn(self, function);
    }
}

// --- reading types ---------------------------------------------------------

/// The base identifier of a type, seen through [`TRANSPARENT`] wrappers.
///
/// Matched by *exact* identifier, never by substring: `EvidenceBundle` is not
/// `Evidence`, and a guard that confused them would fail on an unrelated type
/// somebody legitimately added.
#[must_use]
pub fn base_ident(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Reference(inner) => base_ident(&inner.elem),
        syn::Type::Paren(inner) => base_ident(&inner.elem),
        syn::Type::Group(inner) => base_ident(&inner.elem),
        syn::Type::Path(path) => {
            let segment = path.path.segments.last()?;
            let name = segment.ident.to_string();
            if TRANSPARENT.contains(&name.as_str())
                && let Some(inner) = first_type_argument(&segment.arguments)
            {
                return base_ident(inner);
            }
            Some(name)
        }
        _ => None,
    }
}

/// Every base identifier a value of this type could be.
///
/// [`base_ident`] answers "what is this type", which has one answer only while
/// the type has one component. `-> (Evidence, u8)` produces an `Evidence` and a
/// `u8`, and a rule that read the pair as a single unnamed thing saw neither:
/// `base_ident` returns `None` for a tuple, so the function looked like it
/// returned nothing at all. Pairing the forbidden value with a throwaway is a
/// one-character bypass of a rule that keys on the return type.
fn base_idents(ty: &syn::Type, out: &mut Vec<String>) {
    match ty {
        syn::Type::Reference(inner) => base_idents(&inner.elem, out),
        syn::Type::Paren(inner) => base_idents(&inner.elem, out),
        syn::Type::Group(inner) => base_idents(&inner.elem, out),
        syn::Type::Tuple(tuple) => {
            for element in &tuple.elems {
                base_idents(element, out);
            }
        }
        syn::Type::Path(path) => {
            let Some(segment) = path.path.segments.last() else {
                return;
            };
            let name = segment.ident.to_string();
            // The transparent wrappers are unwrapped here too rather than being
            // left to `base_ident`, so that a tuple *inside* one is reached:
            // `Option<(u8, Artifact)>` hands back an `Artifact` and
            // `base_ident` alone stops at the tuple it cannot name.
            if TRANSPARENT.contains(&name.as_str())
                && let Some(inner) = first_type_argument(&segment.arguments)
            {
                base_idents(inner, out);
                return;
            }
            out.push(name);
        }
        _ => {}
    }
}

/// The type aliases declared by `items`, `type Ev = Evidence;` by name.
///
/// Every rule in this directory keys on a *written* name, and an alias is a
/// second written name for the same type: `type Ev = Evidence;` turns
/// `fn launder(run: &CheckRun) -> Ev` into a producer of `Evidence` that no
/// rule about `Evidence` can see. Rust resolves the alias and the guard did
/// not, which is the whole gap.
///
/// Same-file only, and that is a deliberate stopping point rather than an
/// oversight — see [`resolve_alias`].
fn type_aliases(items: &[syn::Item]) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for item in items {
        let syn::Item::Type(alias) = item else {
            continue;
        };
        if let Some(target) = base_ident(&alias.ty) {
            out.insert(alias.ident.to_string(), target);
        }
    }
    out
}

/// Follow an alias chain to the name it finally stands for.
///
/// `type A = B; type B = Evidence;` resolves `A` to `Evidence`. The step limit
/// is there because `type A = B; type B = A;` does not compile but this reader
/// is not a compiler, and a guard that hangs is a guard that gets killed.
///
/// # What this deliberately does not resolve
///
/// An alias visible only through a `use` from another file, an associated type
/// (`Self::Output`), and a generic parameter bound to the forbidden type
/// elsewhere. Each needs name resolution across crates or trait resolution —
/// which is to say, a type checker — and half a type checker inside a guard is
/// a thing that is wrong in ways nobody can predict. The same-file case is the
/// one a person actually reaches for, because an alias is written where it is
/// used; the rest is recorded here as known, not as handled.
fn resolve_alias(name: &str, aliases: &std::collections::BTreeMap<String, String>) -> String {
    let mut current = name.to_owned();
    for _ in 0..16 {
        match aliases.get(&current) {
            Some(next) if *next != current => current = next.clone(),
            _ => break,
        }
    }
    current
}

/// The first type among a path segment's angle-bracketed arguments.
fn first_type_argument(arguments: &syn::PathArguments) -> Option<&syn::Type> {
    let syn::PathArguments::AngleBracketed(bracketed) = arguments else {
        return None;
    };
    bracketed.args.iter().find_map(|argument| match argument {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })
}

/// The last identifier of a path: `crate::bundle::BundleCore` is `BundleCore`.
fn last_ident(path: &syn::Path) -> Option<String> {
    path.segments.last().map(|s| s.ident.to_string())
}

/// The attributes on an item, whatever kind of item it is.
fn attrs_of(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::ExternCrate(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::ForeignMod(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::TraitAlias(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// The traits named in this item's `#[derive(…)]` attributes.
fn derived_traits(attrs: &[syn::Attribute]) -> Vec<String> {
    let mut out = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("derive") {
            continue;
        }
        let parsed = attr.parse_args_with(
            syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
        );
        let Ok(paths) = parsed else {
            continue;
        };
        out.extend(paths.iter().filter_map(last_ident));
    }
    out
}

/// The names of the modules declared by `items`, `mod foo;` and `mod foo {}`
/// alike.
#[must_use]
pub fn module_declarations(items: &[syn::Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Mod(module) => Some(module.ident.to_string()),
            _ => None,
        })
        .collect()
}

/// Reduce a written visibility to the three cases the guards ask about.
fn visibility_of(vis: &syn::Visibility) -> Vis {
    match vis {
        syn::Visibility::Public(_) => Vis::Public,
        syn::Visibility::Restricted(_) => Vis::Restricted,
        syn::Visibility::Inherited => Vis::Inherited,
    }
}

/// Every function and method in `items`: free, inherent, trait impl, and trait
/// declaration.
///
/// Return types are resolved through any type alias `items` declares, so
/// `type Ev = Evidence; fn launder(run: &CheckRun) -> Ev` is read as the
/// producer of an `Evidence` that it is.
#[must_use]
pub fn functions(items: &[syn::Item]) -> Vec<Function> {
    let aliases = type_aliases(items);
    let mut out = Vec::new();

    for item in items {
        match item {
            syn::Item::Fn(function) => {
                out.push(described(
                    &function.sig,
                    visibility_of(&function.vis),
                    None,
                    None,
                    &aliases,
                ));
            }
            syn::Item::Impl(block) => {
                let owner = base_ident(&block.self_ty);
                let owner_trait = implemented_trait(block);
                for member in &block.items {
                    let syn::ImplItem::Fn(function) = member else {
                        continue;
                    };
                    if !ships(&function.attrs) {
                        continue;
                    }
                    out.push(described(
                        &function.sig,
                        visibility_of(&function.vis),
                        owner.clone(),
                        owner_trait.clone(),
                        &aliases,
                    ));
                }
            }
            syn::Item::Trait(declaration) => {
                let name = declaration.ident.to_string();
                for member in &declaration.items {
                    let syn::TraitItem::Fn(function) = member else {
                        continue;
                    };
                    if !ships(&function.attrs) {
                        continue;
                    }
                    out.push(described(
                        &function.sig,
                        Vis::Inherited,
                        None,
                        Some(name.clone()),
                        &aliases,
                    ));
                }
            }
            _ => {}
        }
    }

    out
}

/// Build a [`Function`] from a signature and the context around it.
fn described(
    sig: &syn::Signature,
    visibility: Vis,
    owner: Option<String>,
    owner_trait: Option<String>,
    aliases: &std::collections::BTreeMap<String, String>,
) -> Function {
    let receiver = match sig.inputs.first() {
        Some(syn::FnArg::Receiver(receiver)) => {
            // `mut self` has `mutability` set and no `reference`, and is taken
            // by value — which is the consuming builder, not a mutator.
            if receiver.reference.is_none() {
                Receiver::Value
            } else if receiver.mutability.is_some() {
                Receiver::RefMut
            } else {
                Receiver::Ref
            }
        }
        _ => Receiver::None,
    };

    let mutably_borrows = sig
        .inputs
        .iter()
        .filter_map(|argument| {
            let syn::FnArg::Typed(typed) = argument else {
                return None;
            };
            let syn::Type::Reference(reference) = &*typed.ty else {
                return None;
            };
            reference.mutability?;
            base_ident(&reference.elem)
        })
        .collect();

    let mut produced = Vec::new();
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        base_idents(ty, &mut produced);
    }
    let produces: Vec<String> = produced
        .into_iter()
        .map(|name| {
            let resolved = if name == "Self" {
                owner.clone().unwrap_or(name)
            } else {
                name
            };
            resolve_alias(&resolved, aliases)
        })
        .collect();

    Function {
        name: sig.ident.to_string(),
        owner,
        owner_trait,
        visibility,
        receiver,
        mutably_borrows,
        returns: produces.first().cloned(),
        produces,
    }
}

/// The base identifier of the trait an `impl` block implements, if it is a
/// trait impl and not a negative one.
fn implemented_trait(block: &syn::ItemImpl) -> Option<String> {
    let (negation, path, _) = block.trait_.as_ref()?;
    if negation.is_some() {
        return None;
    }
    last_ident(path)
}

/// Every `impl` block whose self type has the base identifier `name`.
#[must_use]
pub fn impls_for<'a>(items: &'a [syn::Item], name: &str) -> Vec<&'a syn::ItemImpl> {
    items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Impl(block) if base_ident(&block.self_ty).as_deref() == Some(name) => {
                Some(block)
            }
            _ => None,
        })
        .collect()
}

/// The base identifiers of every trait the type `name` gets, whether written as
/// an `impl` or as a `#[derive(…)]` entry.
///
/// `#[derive(Default)]` and `impl Default for T` produce the same public
/// `T::default()`, and the derive is the more idiomatic of the two — so a rule
/// about the impl that could not see the derive was a rule about spelling.
#[must_use]
pub fn traits_implemented_for(items: &[syn::Item], name: &str) -> Vec<String> {
    let mut out: Vec<String> = impls_for(items, name)
        .into_iter()
        .filter_map(implemented_trait)
        .collect();

    for item in items {
        let attrs = match item {
            syn::Item::Struct(declaration) if declaration.ident == name => &declaration.attrs,
            syn::Item::Enum(declaration) if declaration.ident == name => &declaration.attrs,
            syn::Item::Union(declaration) if declaration.ident == name => &declaration.attrs,
            _ => continue,
        };
        out.extend(derived_traits(attrs));
    }

    out
}

/// The fields of the named struct, with their visibility.
///
/// # Panics
///
/// If no such struct is declared. A guard on a field of a type that is not
/// there is a guard on nothing.
#[must_use]
pub fn struct_fields(items: &[syn::Item], name: &str) -> Vec<(String, Vis)> {
    let declaration = items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(declaration) if declaration.ident == name => Some(declaration),
            _ => None,
        })
        .unwrap_or_else(|| panic!("`{name}` must be declared in this file"));

    declaration
        .fields
        .iter()
        .map(|field| {
            let ident = field
                .ident
                .as_ref()
                .map_or_else(|| "<tuple field>".to_owned(), ToString::to_string);
            (ident, visibility_of(&field.vis))
        })
        .collect()
}

/// Every `From`, `TryFrom` and `Into` impl in `items`, normalized so that
/// `source` is always the type converted from.
///
/// Both ends are resolved through any type alias `items` declares:
/// `type Ev = Evidence; impl From<CheckRun> for Ev` is the forbidden
/// conversion under a second name, and the compiler agrees even though a rule
/// matching the written word `Evidence` does not.
#[must_use]
pub fn conversions(items: &[syn::Item]) -> Vec<Conversion> {
    let aliases = type_aliases(items);
    let mut out = Vec::new();

    for item in items {
        let syn::Item::Impl(block) = item else {
            continue;
        };
        let Some((negation, path, _)) = &block.trait_ else {
            continue;
        };
        if negation.is_some() {
            continue;
        }
        let Some(via) = last_ident(path) else {
            continue;
        };
        if !matches!(via.as_str(), "From" | "TryFrom" | "Into") {
            continue;
        }
        // `impl From<A> for B` names `A` in the trait's arguments and `B` as
        // the self type; `impl Into<B> for A` is the same conversion with the
        // two swapped.
        let Some(segment) = path.segments.last() else {
            continue;
        };
        let Some(argument) = first_type_argument(&segment.arguments) else {
            continue;
        };
        let Some(named) = base_ident(argument) else {
            continue;
        };
        let Some(own) = base_ident(&block.self_ty) else {
            continue;
        };

        let (source, target) = if via == "Into" {
            (own, named)
        } else {
            (named, own)
        };
        out.push(Conversion {
            via,
            source: resolve_alias(&source, &aliases),
            target: resolve_alias(&target, &aliases),
        });
    }

    out
}

/// Every struct literal in a source, with the function containing it.
///
/// `Self { … }` is resolved to the enclosing `impl`'s type, because
/// `impl Default for BundleCore { fn default() -> Self { Self { … } } }` is a
/// second construction site that names the type nowhere. Macro expansions are
/// walked too: a literal written inside a `macro_rules!` body is a construction
/// site in every expansion of it.
#[must_use]
pub fn struct_literals(source: &Source) -> Vec<Literal> {
    let mut walk = LiteralWalk {
        found: Vec::new(),
        enclosing_impl: Vec::new(),
        enclosing_fn: Vec::new(),
    };

    syn::visit::Visit::visit_file(&mut walk, &source.file);
    for expansion in &source.macro_files {
        syn::visit::Visit::visit_file(&mut walk, expansion);
    }
    for expansion in &source.macro_exprs {
        syn::visit::Visit::visit_expr(&mut walk, expansion);
    }

    walk.found
}

/// Tracks the `impl` and function a struct literal was written inside.
struct LiteralWalk {
    found: Vec<Literal>,
    enclosing_impl: Vec<Option<String>>,
    enclosing_fn: Vec<String>,
}

impl<'ast> syn::visit::Visit<'ast> for LiteralWalk {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !ships(attrs_of(item)) {
            return;
        }
        syn::visit::visit_item(self, item);
    }

    fn visit_item_impl(&mut self, block: &'ast syn::ItemImpl) {
        self.enclosing_impl.push(base_ident(&block.self_ty));
        syn::visit::visit_item_impl(self, block);
        self.enclosing_impl.pop();
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        self.enclosing_fn.push(function.sig.ident.to_string());
        syn::visit::visit_item_fn(self, function);
        self.enclosing_fn.pop();
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        if !ships(&function.attrs) {
            return;
        }
        self.enclosing_fn.push(function.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, function);
        self.enclosing_fn.pop();
    }

    fn visit_expr_struct(&mut self, literal: &'ast syn::ExprStruct) {
        if let Some(name) = last_ident(&literal.path) {
            let written_as_self = name == "Self";
            let resolved = if written_as_self {
                self.enclosing_impl
                    .last()
                    .cloned()
                    .flatten()
                    .unwrap_or(name)
            } else {
                name
            };
            self.found.push(Literal {
                type_name: resolved,
                function: self
                    .enclosing_fn
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "<none>".to_owned()),
                written_as_self,
            });
        }
        syn::visit::visit_expr_struct(self, literal);
    }
}
