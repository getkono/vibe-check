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
//! compiler does: [`syn::parse_file`] once per source file, and every question
//! answered off the tree.
//!
//! Parsing also retires the two scanning regimes the text-based guards carried.
//! A `#[cfg(test)]` module is skipped by its *attribute*, so code written below
//! one is still seen — which is precisely where a second construction site or a
//! `mod helpers;` would be written by someone working around a guard.
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

/// Type constructors that do not change what a value *is*.
///
/// `Box<Evidence>`, `Option<Evidence>` and `&Evidence` are all ways of handing
/// someone an `Evidence`, so a guard on `Evidence` has to see through them.
/// Deliberately short: `Result<_, _>` and `ForgeResult<_>` are **not** here,
/// because "a fallible operation that may yield one" is a different claim from
/// "a value of this type", and unwrapping them would make `Forge::download`,
/// whose whole job is to produce an `Artifact` from downloaded bytes, report
/// itself.
const TRANSPARENT: [&str; 5] = ["Box", "Arc", "Rc", "Option", "Vec"];

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

/// A function or method, with the `impl` it was found in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Function {
    /// Its name.
    pub name: String,
    /// Base identifier of the type the enclosing `impl` is for, if any.
    pub owner: Option<String>,
    /// Base identifier of the trait the enclosing `impl` implements, if any.
    pub owner_trait: Option<String>,
    /// Visibility as written.
    pub visibility: Vis,
    /// How it takes `self`.
    pub receiver: Receiver,
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

/// The workspace root, derived from this crate's manifest directory.
#[must_use]
pub fn workspace_root() -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Utf8Path::parent)
        .expect("the model crate sits two levels below the workspace root")
        .to_owned()
}

/// Every `.rs` file under `crates/*/src`, parsed, in a stable order.
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
pub fn workspace_sources() -> Vec<(Utf8PathBuf, syn::File)> {
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
            let parsed = parse(path.as_str(), &text);
            (path, parsed)
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

/// Whether any attribute is a `#[cfg(…)]` mentioning `test`.
///
/// The token stream is compared token by token rather than by substring, so a
/// `#[cfg(feature = "testkit")]` is not mistaken for a test gate.
fn is_test_gated_attrs(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let syn::Meta::List(list) = &attr.meta else {
            return false;
        };
        list.path.is_ident("cfg")
            && list
                .tokens
                .to_string()
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|token| token == "test")
    })
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

/// Whether an item is gated out of the compiled crate by `#[cfg(test)]`.
#[must_use]
pub fn is_test_gated(item: &syn::Item) -> bool {
    is_test_gated_attrs(attrs_of(item))
}

/// Every item in a file, flattened through inline modules, `#[cfg(test)]` gone.
///
/// The gate is read from the *attribute*, so an item written below a test
/// module is still returned. That is the whole gap the previous truncating
/// scanner had: a `mod helpers;` or a second construction site written under
/// `mod tests` was invisible to it, and that is exactly where one would be
/// written by someone working around a guard.
#[must_use]
pub fn items(file: &syn::File) -> Vec<&syn::Item> {
    let mut out = Vec::new();
    push_items(&file.items, &mut out);
    out
}

fn push_items<'a>(source: &'a [syn::Item], out: &mut Vec<&'a syn::Item>) {
    for item in source {
        if is_test_gated(item) {
            continue;
        }
        out.push(item);
        if let syn::Item::Mod(module) = item
            && let Some((_, inner)) = &module.content
        {
            push_items(inner, out);
        }
    }
}

/// The top-level items of a file, `#[cfg(test)]` gone and modules not entered.
///
/// What the "this module has no children" guards ask about: whether *this* file
/// declares a submodule, not what any submodule contains.
#[must_use]
pub fn top_level_items(file: &syn::File) -> Vec<&syn::Item> {
    file.items
        .iter()
        .filter(|item| !is_test_gated(item))
        .collect()
}

/// The names of the modules declared by `items`, `mod foo;` and `mod foo {}`
/// alike.
#[must_use]
pub fn module_declarations(items: &[&syn::Item]) -> Vec<String> {
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

/// Every function and method reachable from `items`: free, inherent, trait
/// impl, and trait declaration.
#[must_use]
pub fn functions(items: &[&syn::Item]) -> Vec<Function> {
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
                    if is_test_gated_attrs(&function.attrs) {
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
                for member in &declaration.items {
                    let syn::TraitItem::Fn(function) = member else {
                        continue;
                    };
                    if is_test_gated_attrs(&function.attrs) {
                        continue;
                    }
                    out.push(described(&function.sig, Vis::Inherited, None, None));
                }
            }
            _ => {}
        }
    }

    out
}

/// Build a [`Function`] from a signature and the `impl` context around it.
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
pub fn impls_for<'a>(items: &[&'a syn::Item], name: &str) -> Vec<&'a syn::ItemImpl> {
    items
        .iter()
        .copied()
        .filter_map(|item| match item {
            syn::Item::Impl(block) if base_ident(&block.self_ty).as_deref() == Some(name) => {
                Some(block)
            }
            _ => None,
        })
        .collect()
}

/// The base identifiers of every trait implemented *for* the type `name`.
#[must_use]
pub fn traits_implemented_for(items: &[&syn::Item], name: &str) -> Vec<String> {
    impls_for(items, name)
        .into_iter()
        .filter_map(implemented_trait)
        .collect()
}

/// The fields of the named struct, with their visibility.
///
/// # Panics
///
/// If no such struct is declared. A guard on a field of a type that is not
/// there is a guard on nothing.
#[must_use]
pub fn struct_fields(items: &[&syn::Item], name: &str) -> Vec<(String, Vis)> {
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
pub fn conversions(items: &[&syn::Item]) -> Vec<Conversion> {
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

/// Every struct literal in a file, with the function containing it.
///
/// `Self { … }` is resolved to the enclosing `impl`'s type, because
/// `impl Default for BundleCore { fn default() -> Self { Self { … } } }` is a
/// second construction site that names the type nowhere.
#[must_use]
pub fn struct_literals(file: &syn::File) -> Vec<Literal> {
    let mut walk = LiteralWalk {
        found: Vec::new(),
        enclosing_impl: Vec::new(),
        enclosing_fn: Vec::new(),
    };
    syn::visit::Visit::visit_file(&mut walk, file);
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
        if is_test_gated(item) {
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
        if is_test_gated_attrs(&function.attrs) {
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
