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
//! 2. **Item bodies.** `const _: () = { impl From<String> for Evidence {…} };`
//!    registers that impl globally, and it is nested inside a `const`
//!    initialiser rather than at the top level of a file. The walk descends into
//!    every body, not only into modules.
//! 3. **Macro bodies.** `syn` hands a `macro_rules!` definition back as opaque
//!    tokens, so an `impl` written inside one is invisible — while the old text
//!    scan could still see it, which made a naive parse a *regression*. The
//!    expansions are re-parsed here, with metavariables substituted for plain
//!    identifiers, and an expansion that cannot be parsed at all is a loud
//!    failure rather than a silent skip.
//!
//! A guard that stops catching something silently is worse than no guard: the
//! green is what a reviewer trusts.
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
    /// Base identifier of the return type, with `Self` resolved to [`owner`].
    ///
    /// [`owner`]: Function::owner
    pub returns: Option<String>,
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
    /// Anything else — `feature = "e2e"`, `unix`, `debug_assertions`. Not
    /// evaluable here, and conservatively treated as satisfied: an item behind
    /// one ships in *some* configuration, and a guard that skipped it would be
    /// a guard someone could hide behind a feature flag.
    Other,
}

impl Parse for Cfg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let path: syn::Path = input.parse()?;
        let name = path.get_ident().map(ToString::to_string);

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

impl Cfg {
    /// Whether the predicate holds in an ordinary build — the one that produces
    /// the artifact people link against, in which `test` is off.
    fn holds(&self) -> bool {
        match self {
            Cfg::Test => false,
            Cfg::Not(inner) => !inner.holds(),
            Cfg::All(list) => list.iter().all(Cfg::holds),
            Cfg::Any(list) => list.iter().any(Cfg::holds),
            Cfg::Other => true,
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
        let predicate: Cfg = syn::parse2(list.tokens.clone()).unwrap_or_else(|error| {
            panic!(
                "a `#[cfg(…)]` this guard cannot classify is a gate it cannot \
                 reason about, and guessing is how an item vanishes from a \
                 guard silently: {error}"
            )
        });
        predicate.holds()
    })
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
    file: syn::File,
    macro_files: Vec<syn::File>,
    macro_exprs: Vec<syn::Expr>,
    items: Vec<syn::Item>,
}

impl Source {
    /// Every item that ships, flattened.
    #[must_use]
    pub fn items(&self) -> &[syn::Item] {
        &self.items
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

    let mut walk = ItemWalk {
        label: label.to_owned(),
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
        file,
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

    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("a listed source file is readable");
            let source = read(path.as_str(), &text);
            (path, source)
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

/// The expansion of each rule in a `macro_rules!` body: the token group that
/// follows each `=>`.
fn rule_expansions(body: &TokenStream) -> Vec<TokenStream> {
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
        if let Some(TokenTree::Group(group)) = trees.get(index + 2) {
            out.push(group.stream());
        }
    }

    out
}

/// Replace macro metavariables with plain identifiers so an expansion parses.
///
/// `$name` becomes `metavar_name`, and a `$( … )sep*` repetition is spliced in
/// once with its operator dropped. The result is not what the macro expands to
/// — it is a *representative* of every expansion, which is exactly what a
/// structural guard needs: `impl From<$name> for Evidence` is the forbidden
/// impl in each of the eleven types that macro is invoked for.
fn substitute(stream: TokenStream) -> TokenStream {
    let trees: Vec<TokenTree> = stream.into_iter().collect();
    let mut out: Vec<TokenTree> = Vec::new();
    let mut index = 0usize;

    while index < trees.len() {
        match &trees[index] {
            TokenTree::Punct(punct) if punct.as_char() == '$' => {
                match trees.get(index + 1) {
                    Some(TokenTree::Ident(name)) => {
                        out.push(TokenTree::Ident(Ident::new(
                            &format!("metavar_{name}"),
                            Span::call_site(),
                        )));
                        index += 2;
                    }
                    Some(TokenTree::Group(group)) => {
                        out.extend(substitute(group.stream()));
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
                    substitute(group.stream()),
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

// --- the walk --------------------------------------------------------------

/// Collects every shipped item, descending into bodies and macro definitions.
struct ItemWalk {
    label: String,
    items: Vec<syn::Item>,
    macro_files: Vec<syn::File>,
    macro_exprs: Vec<syn::Expr>,
}

impl ItemWalk {
    /// Re-parse a `macro_rules!` definition's expansions.
    ///
    /// # Panics
    ///
    /// If an expansion parses as neither an item list, an expression, nor a
    /// block. `syn` treats a macro body as opaque, so the alternative is to
    /// skip it — and skipping is what let `impl From<$name> for Evidence` hide
    /// inside `id_newtype!`.
    fn expand(&mut self, definition: &syn::ItemMacro) {
        if definition.ident.is_none() || !definition.mac.path.is_ident("macro_rules") {
            return;
        }
        let name = definition
            .ident
            .as_ref()
            .map_or_else(|| "<anonymous>".to_owned(), ToString::to_string);

        for expansion in rule_expansions(&definition.mac.tokens) {
            let tokens = substitute(expansion);

            if let Ok(file) = syn::parse2::<syn::File>(tokens.clone()) {
                self.macro_files.push(file);
                continue;
            }
            if let Ok(expr) = syn::parse2::<syn::Expr>(tokens.clone()) {
                self.macro_exprs.push(expr);
                continue;
            }
            let braced: TokenStream = TokenTree::Group(Group::new(Delimiter::Brace, tokens)).into();
            if let Ok(block) = syn::parse2::<syn::Block>(braced) {
                self.macro_exprs.push(syn::Expr::Block(syn::ExprBlock {
                    attrs: Vec::new(),
                    label: None,
                    block,
                }));
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
#[must_use]
pub fn functions(items: &[syn::Item]) -> Vec<Function> {
    let mut out = Vec::new();

    for item in items {
        match item {
            syn::Item::Fn(function) => {
                out.push(described(
                    &function.sig,
                    visibility_of(&function.vis),
                    None,
                    None,
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

    let returns = match &sig.output {
        syn::ReturnType::Type(_, ty) => base_ident(ty).map(|name| {
            if name == "Self" {
                owner.clone().unwrap_or(name)
            } else {
                name
            }
        }),
        syn::ReturnType::Default => None,
    };

    Function {
        name: sig.ident.to_string(),
        owner,
        owner_trait,
        visibility,
        receiver,
        mutably_borrows,
        returns,
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
#[must_use]
pub fn conversions(items: &[syn::Item]) -> Vec<Conversion> {
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
            source,
            target,
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
