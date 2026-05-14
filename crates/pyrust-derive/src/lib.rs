//! `pyrust-derive` — proc-macros for declaring built-in Python modules in Rust.
//!
//! Two entry points:
//!
//! - **`pyrust_module! { … }`** (function-like, primary) — declares a
//!   whole built-in module's content (optional constants and a sequence
//!   of functions).  The **module's Python-level name is *not* given here**;
//!   the macro reads it from a sibling `MODULE_NAME: &str` constant that
//!   the surrounding `pyrust_builtin_modules!` invocation in
//!   `builtin_modules/mod.rs` injects.  This makes that `mod.rs`
//!   the single source of truth for the set of built-in modules and
//!   their Python-level names.
//!
//!   Each `fn name(args)` is expanded to a full Rust fn with the
//!   canonical dispatch signature; a `FN_NAME: &str` local at the top of
//!   the body lets call sites (helpers, error messages) reference the
//!   Python-level full name without re-spelling it.
//!
//! - **`#[pyfunction(name = "module.fn")]`** (attribute, one-off fallback)
//!   — drop-in for moving a single arm of the legacy cascade
//!   incrementally.  Same output as a single `fn` inside
//!   `pyrust_module!` minus the `regs`/`module()` plumbing.
//!
//! ## `pyrust_module!` syntax
//!
//! ```ignore
//! // bodies/math.rs (included from `pub mod math { … }` declared by
//! // `pyrust_builtin_modules!` in mod.rs):
//! pyrust_module! {
//!     constants {
//!         "pi" => Value::float(std::f64::consts::PI),
//!     }
//!
//!     /// CPython: math.sqrt(x) → float.
//!     fn sqrt(args) -> Result<Value> {
//!         Ok(Value::float(single_float(FN_NAME, args)?.sqrt()))
//!     }
//! }
//! ```
//!
//! Generated, in the surrounding `mod math { … }`:
//! - `fn math_sqrt(_interp, args) -> Result<Value> { /* FN_NAME = "math.sqrt"; body */ }`
//! - `pub(crate) fn regs() -> &'static [BuiltinReg]` — every fn's
//!   `BuiltinReg`, names composed once via leak from `FN_PREFIX +
//!   short_name` (e.g. `"math." + "sqrt"`, or `"" + "abs"` for the
//!   flat-namespace `builtins` module).
//! - `pub(crate) fn module() -> Value` — the `PyModule` carrying the
//!   declared constants plus each fn bound to its
//!   `Value::builtin_function(...)`.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{ToTokens, format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{
    Block, Expr, Ident, ItemFn, LitStr, Meta, Token, parse_macro_input, punctuated::Punctuated,
};

// ─── `#[pyfunction]` (one-off attribute form) ────────────────────────────────

/// `#[pyfunction(name = "module.fn")]` — emits a sibling registration
/// constant so the function is picked up by the built-in dispatch
/// registry.  Use this for migrating individual arms; for a whole
/// module, prefer [`pyrust_module`].
#[proc_macro_attribute]
pub fn pyfunction(attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);
    let metas = parse_macro_input!(attr with Punctuated::<Meta, Token![,]>::parse_terminated);

    let mut name: Option<LitStr> = None;
    for meta in metas {
        if let Meta::NameValue(nv) = &meta
            && nv.path.is_ident("name")
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
        {
            name = Some(s.clone());
        }
    }

    let py_name = match name {
        Some(n) => n,
        None => {
            return syn::Error::new(
                Span::call_site(),
                "#[pyfunction] requires a `name = \"...\"` attribute",
            )
            .to_compile_error()
            .into();
        }
    };

    let fn_ident = &func.sig.ident;
    // Strip a leading `r#` from raw idents (`r#type` → `type`) before
    // upper-casing — otherwise the generated const ident would contain
    // `#`, which isn't a legal identifier character.
    let fn_ident_str = fn_ident.to_string();
    let unraw_str = fn_ident_str.strip_prefix("r#").unwrap_or(&fn_ident_str);
    let const_ident = Ident::new(&unraw_str.to_uppercase(), fn_ident.span());

    let expanded = quote! {
        #func

        #[allow(non_upper_case_globals)]
        pub const #const_ident: crate::builtin_registry::BuiltinReg =
            crate::builtin_registry::BuiltinReg {
                name: #py_name,
                dispatch: #fn_ident,
            };
    };

    expanded.into()
}

// ─── `pyrust_module!` (function-like, file-scoped) ────────────────────────────

/// Parsed `pyrust_module! { … }` input.
///
/// The Python-level module name is *not* part of the input — it's read
/// from a sibling `MODULE_NAME: &str` constant injected by
/// `pyrust_builtin_modules!`.
struct ModuleInput {
    constants: Vec<(LitStr, Expr)>,
    funcs: Vec<ModuleFn>,
    classes: Vec<ModuleClass>,
}

/// A single `fn` declaration inside `pyrust_module!`.
struct ModuleFn {
    /// Outer attributes that should remain on the generated fn (`#[doc = …]`
    /// and friends).  `#[py_name = "..."]` is extracted out and stored in
    /// `py_name_override` so it doesn't leak through to the emitted code.
    attrs: Vec<syn::Attribute>,
    short_name: Ident,
    /// Optional override for the Python-level name, set via
    /// `#[py_name = "..."]`.  Used when the desired Python name is a Rust
    /// strict keyword that can't be a raw identifier (`super`, etc.).
    py_name_override: Option<LitStr>,
    /// The argument-list dialect this fn uses.  See [`ModuleFnArgs`].
    args: ModuleFnArgs,
    body: Block,
}

/// Argument-list dialect — legacy `(args: &[ExpandedCallArg])` or typed.
///
/// **Legacy** is the historical form: the body sees one raw `args` slice and
/// must hand-validate / type-check every parameter.  **Typed** is the new
/// dialect (#395): per-parameter `name: PyType` declarations, the macro emits
/// a prelude that validates + binds typed locals, and the body sees them
/// directly.
enum ModuleFnArgs {
    Legacy { args_ident: Ident },
    Typed { params: Vec<TypedParam> },
}

/// One parameter in a typed signature.  Outer attrs (`#[default(expr)]`,
/// `#[positional_only]`, `#[keyword_only]`) are extracted at parse time and
/// stored on this struct; only doc / cfg / allow attrs remain.
struct TypedParam {
    /// Parameter name (the local that gets bound in the body).
    name: Ident,
    /// The wrapper type — e.g. `PyInt`, `PyFloat`, `Option<PyStr>`.
    ty: syn::Type,
    /// `#[default(expr)]` if present.  Lazy: only evaluated when the slot is
    /// missing at call time.
    default: Option<syn::Expr>,
    /// `#[positional_only]` — passing by keyword is rejected.
    positional_only: bool,
    /// `#[keyword_only]` — passing positionally is rejected.
    keyword_only: bool,
}

/// A `class ClassName { fn … }` block inside `pyrust_module!`.
///
/// Each method generates a separate registry entry under the qualified
/// name `<FN_PREFIX><ClassName>.<method>` (e.g.
/// `"collections.Counter.__init__"`).  At `module()` construction time,
/// the class is built as a real `PyClass` with the methods bound as
/// `BuiltinFunction` attrs, so pyrust's standard class machinery (the
/// dunder dispatch sites already exercised by user-defined classes)
/// picks up `__init__`, `__iter__`, `__getitem__`, etc.
///
/// ## Method-body conventions
///
/// - `args` always has `self` as `args[0]` (the instance `Value`).
///   User-level args start at `args[1..]`.
/// - `_interp: &mut crate::Interpreter` is in scope, identical to
///   module-level fns — methods can re-enter the interpreter via
///   `_interp.call_function_expanded(...)` to dispatch other callables.
/// - `FN_NAME: &'static str` resolves to the fully-qualified Python
///   name (e.g. `"collections.Counter.__init__"`), useful for error
///   messages that don't want to drift from the registry name.
struct ModuleClass {
    /// Outer attributes (mostly `#[doc = …]`).
    attrs: Vec<syn::Attribute>,
    name: Ident,
    methods: Vec<ModuleFn>,
}

impl Parse for ModuleInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Optional `constants { ... }` block — declarations begin with
        // either `fn` (kw) or one of the contextual idents `class` /
        // `constants`.  Only consume here if the leading ident is
        // literally "constants"; everything else falls through to the
        // main loop below.
        let mut constants: Vec<(LitStr, Expr)> = Vec::new();
        if input.peek(Ident) {
            let lookahead: Ident = input.fork().parse()?;
            if lookahead == "constants" {
                let _: Ident = input.parse()?;
                let content;
                syn::braced!(content in input);
                while !content.is_empty() {
                    let key: LitStr = content.parse()?;
                    let _: Token![=>] = content.parse()?;
                    let value: Expr = content.parse()?;
                    constants.push((key, value));
                    if content.peek(Token![,]) {
                        let _: Token![,] = content.parse()?;
                    }
                }
                if input.peek(Token![,]) {
                    let _: Token![,] = input.parse()?;
                }
            }
            // Otherwise the ident is `class` (handled below) or an
            // unexpected token (`fn`-led decls don't trip this branch
            // because `Token![fn]` isn't peeked as `Ident`).  The main
            // loop's error reporting is sharper for invalid input than
            // a generic message here.
        }

        // Zero-or-more `fn` and `class` declarations, in any order.
        let mut funcs: Vec<ModuleFn> = Vec::new();
        let mut classes: Vec<ModuleClass> = Vec::new();
        while !input.is_empty() {
            // Pull outer attrs once at the head; we'll route them to either
            // the fn or the class depending on what comes next.  `#[py_name]`
            // is fn-only; classes don't accept it.
            let raw_attrs = input.call(syn::Attribute::parse_outer)?;

            // Peek the next ident to disambiguate `class Foo { … }` from
            // `fn name(...)`.  `class` is not a Rust keyword, so it parses
            // as a regular ident — we treat it as a contextual keyword.
            let is_class = input.peek(Ident) && {
                let lookahead: Ident = input.fork().parse()?;
                lookahead == "class"
            };

            if is_class {
                // `#[py_name = "..."]` doesn't apply to class declarations.
                if raw_attrs.iter().any(|a| a.path().is_ident("py_name")) {
                    return Err(syn::Error::new_spanned(
                        &raw_attrs[0],
                        "`#[py_name = \"...\"]` may not appear on a `class` declaration; \
                         use it on individual methods if a Python-level name needs overriding",
                    ));
                }
                classes.push(Self::parse_class(input, raw_attrs)?);
            } else {
                funcs.push(Self::parse_fn(input, raw_attrs)?);
            }
        }

        Ok(ModuleInput {
            constants,
            funcs,
            classes,
        })
    }
}

impl ModuleInput {
    /// Parse one `fn name(args) [-> Type] { body }` declaration.  `raw_attrs`
    /// is the outer-attr list already harvested by the caller (so `#[py_name]`
    /// dispatch happens once at the boundary between fn and class blocks).
    fn parse_fn(input: ParseStream, raw_attrs: Vec<syn::Attribute>) -> syn::Result<ModuleFn> {
        // Pull `#[py_name = "..."]` out of the attr list so it's not
        // emitted on the generated Rust fn (which wouldn't recognise it).
        let mut py_name_override: Option<LitStr> = None;
        let mut attrs: Vec<syn::Attribute> = Vec::with_capacity(raw_attrs.len());
        for attr in raw_attrs {
            if attr.path().is_ident("py_name") {
                let nv = attr.meta.require_name_value().map_err(|_| {
                    syn::Error::new_spanned(
                        &attr,
                        "`#[py_name = \"...\"]` must use the name-value form",
                    )
                })?;
                if let Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = &nv.value
                {
                    if py_name_override.is_some() {
                        return Err(syn::Error::new_spanned(
                            &attr,
                            "`#[py_name]` may appear at most once per fn",
                        ));
                    }
                    py_name_override = Some(s.clone());
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.value,
                        "`#[py_name = \"...\"]` value must be a string literal",
                    ));
                }
            } else {
                attrs.push(attr);
            }
        }
        let _fn_kw: Token![fn] = input.parse()?;
        // Function ident must be snake_case so the generated
        // SCREAMING_SNAKE constant name is unique without lossy
        // case-folding (e.g. `fn isDir` and `fn isdir` would both
        // produce the same const ident).  Accepts raw identifiers
        // (`r#type`) so callables whose Python name collides with a
        // Rust keyword can be declared without uglifying the name.
        let short_name: Ident = input.parse()?;
        // `Ident::to_string()` keeps the `r#` prefix on raw idents
        // (e.g. `r#type` round-trips as `"r#type"`).  Strip it so the
        // Python-level registration name is just `"type"`.
        let short_str = short_name
            .to_string()
            .strip_prefix("r#")
            .map_or_else(|| short_name.to_string(), str::to_owned);
        if !is_snake_case(&short_str) {
            return Err(syn::Error::new(
                short_name.span(),
                format!(
                    "`pyrust_module!` function names must be snake_case (got `{short_str}`); \
                     CamelCase would risk const-ident collisions with other functions",
                ),
            ));
        }
        let arg_content;
        syn::parenthesized!(arg_content in input);
        let args = Self::parse_fn_args(&arg_content)?;
        // Optional return-type annotation (ignored — always Result<Value>).
        if input.peek(Token![->]) {
            let _: Token![->] = input.parse()?;
            let _: syn::Type = input.parse()?;
        }
        let body: Block = input.parse()?;
        Ok(ModuleFn {
            attrs,
            short_name,
            py_name_override,
            args,
            body,
        })
    }

    /// Parse the contents of the fn parameter list.  Two dialects:
    ///
    /// - **Legacy** — a single bare `args` ident, optionally annotated with
    ///   `: &[ExpandedCallArg]`.  Recognised when there is exactly one
    ///   param without per-param attrs, the type (if any) is the expected
    ///   slice, and there is no default value.
    /// - **Typed** — one or more `name: PyType` pairs, each optionally
    ///   preceded by `#[default(expr)]` / `#[positional_only]` /
    ///   `#[keyword_only]` attributes.  Selected for anything that isn't
    ///   the legacy shape, including the empty parameter list `()`.
    ///
    /// The "single bare ident" name doesn't have to be literally `args` —
    /// any single un-annotated ident is treated as a legacy slice binding,
    /// preserving back-compat with bodies that happen to call it something
    /// like `xs` or `_args`.
    fn parse_fn_args(input: ParseStream) -> syn::Result<ModuleFnArgs> {
        let mut params: Vec<TypedParam> = Vec::new();
        while !input.is_empty() {
            params.push(Self::parse_one_param(input)?);
            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            } else if !input.is_empty() {
                return Err(input.error("expected `,` or `)` after parameter"));
            }
        }

        // Legacy detection — a single bare ident with no attrs / default /
        // explicit type (or the legacy slice type) → legacy slice binding.
        if params.len() == 1 {
            let p = &params[0];
            let attrs_empty = p.default.is_none() && !p.positional_only && !p.keyword_only;
            let ty_str = ty_to_canonical_string(&p.ty);
            let is_legacy_type = ty_str.is_empty()
                || ty_str == "&[ExpandedCallArg]"
                || ty_str == "&[crate::interpreter::ExpandedCallArg]";
            if attrs_empty && is_legacy_type {
                return Ok(ModuleFnArgs::Legacy {
                    args_ident: params.into_iter().next().unwrap().name,
                });
            }
        }

        // Typed dialect — enforce two structural invariants the codegen
        // assumes downstream:
        //
        // 1. Every parameter has an explicit type annotation.  Without
        //    this guard a user writes `#[default("r".into())] mode` (no
        //    `: PyStr`), the `()` placeholder leaks into the typed
        //    prelude, and rustc emits "`FromValue` not implemented for
        //    `()`" pointed at the macro expansion — a confusing diagnostic
        //    at the wrong layer.  Reject up front with a clear message.
        // 2. Parameter names are unique.  Duplicates would silently
        //    shadow in the generated body (same name → same `let` ident),
        //    and Rust's "unused variable" lint isn't fired on
        //    macro-generated bindings.
        let mut seen: std::collections::HashSet<String> =
            std::collections::HashSet::with_capacity(params.len());
        for p in &params {
            if ty_to_canonical_string(&p.ty).is_empty() {
                return Err(syn::Error::new(
                    p.name.span(),
                    format!(
                        "typed parameter `{}` requires a type annotation (e.g. `: PyStr`); \
                         drop the attributes if you intended the legacy `(args)` form",
                        p.name,
                    ),
                ));
            }
            if !seen.insert(p.name.to_string()) {
                return Err(syn::Error::new(
                    p.name.span(),
                    format!(
                        "duplicate parameter name `{}` — each parameter must have a unique name",
                        p.name,
                    ),
                ));
            }
        }
        Ok(ModuleFnArgs::Typed { params })
    }

    /// Parse one parameter: `[#[...]]* name [: Type]`.
    /// Returns a `TypedParam` whose `ty` defaults to the unit type `()` when
    /// no annotation is given — `parse_fn_args` then uses the absence of
    /// type-info to decide between legacy and typed.
    fn parse_one_param(input: ParseStream) -> syn::Result<TypedParam> {
        let raw_attrs = input.call(syn::Attribute::parse_outer)?;

        // Harvest the recognised attributes; reject unknown ones (better
        // diagnostic than silent drop).  Each attribute that toggles a
        // flag also checks the dual flag inline, so the conflict-error
        // span lands on the offending attr (the second one declared)
        // rather than the parameter ident or a post-hoc cursor position.
        let mut default: Option<syn::Expr> = None;
        let mut positional_only = false;
        let mut keyword_only = false;
        for attr in &raw_attrs {
            if attr.path().is_ident("default") {
                if default.is_some() {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "`#[default]` may appear at most once per parameter",
                    ));
                }
                let lit = attr.parse_args::<syn::Expr>()?;
                default = Some(lit);
            } else if attr.path().is_ident("positional_only") {
                if keyword_only {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "a parameter cannot be both `#[positional_only]` and `#[keyword_only]`",
                    ));
                }
                positional_only = true;
            } else if attr.path().is_ident("keyword_only") {
                if positional_only {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "a parameter cannot be both `#[positional_only]` and `#[keyword_only]`",
                    ));
                }
                keyword_only = true;
            } else if attr.path().is_ident("doc")
                || attr.path().is_ident("cfg")
                || attr.path().is_ident("allow")
            {
                // Permitted but not consumed — `pyrust_module!` doesn't
                // forward per-param doc comments anywhere yet, but the
                // user may have written them for source-level documentation.
            } else {
                return Err(syn::Error::new_spanned(
                    attr,
                    "unsupported parameter attribute — expected one of \
                     `#[default(expr)]`, `#[positional_only]`, `#[keyword_only]`",
                ));
            }
        }

        let name: Ident = input.parse()?;

        // Optional `: Type` — absent for the bare `(args)` legacy form.
        let ty: syn::Type = if input.peek(Token![:]) {
            let _: Token![:] = input.parse()?;
            input.parse()?
        } else {
            // Placeholder unit type that the dialect-detection step in
            // `parse_fn_args` recognises as "no type given".
            syn::parse_quote!(())
        };

        Ok(TypedParam {
            name,
            ty,
            default,
            positional_only,
            keyword_only,
        })
    }

    /// Parse one `class ClassName { fn method(args) … fn method(args) … }`
    /// declaration.  The `class` ident is at the head of `input`.
    fn parse_class(input: ParseStream, attrs: Vec<syn::Attribute>) -> syn::Result<ModuleClass> {
        // We only know how to handle doc-comment attrs on a class — those
        // are dropped from emission below (statements can't carry doc
        // comments, and the class block lowers to a statement).  Anything
        // else (`#[cfg(...)]`, `#[allow(...)]`, etc.) would be silently
        // discarded with a real semantic effect lost, so reject it up
        // front rather than letting bug reports come from "my `cfg` did
        // nothing".
        for attr in &attrs {
            if !attr.path().is_ident("doc") {
                return Err(syn::Error::new_spanned(
                    attr,
                    "only doc-comment attrs are supported on `class { … }`; \
                     other attrs (e.g. `#[cfg]`, `#[allow]`) would be silently dropped",
                ));
            }
        }
        let class_kw: Ident = input.parse()?;
        debug_assert_eq!(class_kw.to_string(), "class");
        let name: Ident = input.parse()?;
        let content;
        syn::braced!(content in input);
        let mut methods: Vec<ModuleFn> = Vec::new();
        // Track method names within this class so we can flag duplicates
        // at macro-expand time.  Without this guard, two methods sharing
        // a Python-level name would both register as `BuiltinReg`s under
        // the same key — the duplicate-name `assert!` in
        // `builtin_registry::REGISTRY` catches it at first lookup, but
        // the macro-time error is far more actionable.
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        while !content.is_empty() {
            let method_attrs = content.call(syn::Attribute::parse_outer)?;
            let method = Self::parse_fn(&content, method_attrs)?;
            // Use the Python-level name (post `#[py_name]` override) since
            // that's what reaches the registry — collisions on the
            // *Rust* ident are caught by rustc later anyway.
            let py_name = match &method.py_name_override {
                Some(lit) => lit.value(),
                None => method
                    .short_name
                    .to_string()
                    .strip_prefix("r#")
                    .map_or_else(|| method.short_name.to_string(), str::to_owned),
            };
            if !seen_names.insert(py_name.clone()) {
                return Err(syn::Error::new(
                    method.short_name.span(),
                    format!(
                        "duplicate method `{py_name}` in class `{name}` \
                         (the second declaration would silently shadow the first)",
                    ),
                ));
            }
            methods.push(method);
        }
        if methods.is_empty() {
            return Err(syn::Error::new(
                name.span(),
                format!(
                    "`class {name}` must declare at least one method (`fn __init__(args) {{ … }}`)",
                ),
            ));
        }
        Ok(ModuleClass {
            attrs,
            name,
            methods,
        })
    }
}

/// Returns true if `s` is a Rust snake_case identifier — lowercase
/// letters, digits, and underscores only.
fn is_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Emit the per-fn artefacts for one method:
/// - a dispatch `fn` named `__pyfn_<rust_ident>` (or namespaced under a class)
/// - a module-scope `LazyLock<&'static str>` holding the leaked Python name
/// - a `BuiltinReg { name, dispatch }` entry for the registry
///
/// `name_suffix` is what gets concatenated after `FN_PREFIX` to form the
/// Python-level name: `"sqrt"` for a top-level fn, `"Counter.__init__"`
/// for a class method.  `rust_ident_suffix` is what gets concatenated
/// after `__pyfn_` to form the Rust ident.
///
/// Returns `(fn_item, reg_entry, name_static_ident)`.  The class-builder
/// path uses `name_static_ident` to thread the same leaked pointer into
/// the class's method attrs; module-level fns use it via the module's
/// attr_entries.
fn emit_fn_artefacts(
    f: &ModuleFn,
    name_suffix: &str,
    rust_ident_suffix: &str,
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream, Ident) {
    let short = &f.short_name;
    let rust_fn_ident = format_ident!("__pyfn_{}", rust_ident_suffix, span = short.span());
    let name_static_ident = format_ident!("__pyfn_{}_NAME", rust_ident_suffix, span = short.span());
    let attrs = &f.attrs;
    let body_stmts = &f.body.stmts;
    let suffix_lit = LitStr::new(name_suffix, short.span());

    // Dialect-specific binding of the args slice + optional typed prelude.
    let (args_binding, typed_prelude) = match &f.args {
        ModuleFnArgs::Legacy { args_ident } => (quote!(#args_ident), quote!()),
        ModuleFnArgs::Typed { params } => (quote!(__pyrust_args), emit_typed_prelude(params)),
    };

    let fn_item = quote! {
        #[allow(non_upper_case_globals)]
        static #name_static_ident: std::sync::LazyLock<&'static str> =
            std::sync::LazyLock::new(|| {
                ::std::boxed::Box::leak(
                    format!("{}{}", FN_PREFIX, #suffix_lit).into_boxed_str(),
                )
            });

        // Class-method dispatch idents look like `__pyfn_Counter____init__`,
        // which trips `non_snake_case` (the `Counter` segment isn't
        // lowercase).  This lint is about *user-facing* idents — the macro
        // names are internal and never appear in the surface API.
        #[allow(non_snake_case)]
        #(#attrs)*
        fn #rust_fn_ident(
            _interp: &mut crate::Interpreter,
            #args_binding: &[ExpandedCallArg],
        ) -> Result<Value> {
            #[allow(non_snake_case)]
            let FN_NAME: &'static str = *#name_static_ident;
            let _ = FN_NAME; // suppress unused warning if body ignores it
            #typed_prelude
            #(#body_stmts)*
        }
    };

    let reg_entry = quote! {
        crate::builtin_registry::BuiltinReg {
            name: *#name_static_ident,
            dispatch: #rust_fn_ident,
        }
    };

    (fn_item, reg_entry, name_static_ident)
}

/// Emit the prelude that validates + binds typed parameters before the
/// user-written body runs.  See the typed-dialect docs on
/// [`crate::interpreter::builtin_args`] for the full contract.
///
/// Generated shape (for `(path: PyStr, mode: PyStr = "r")`):
///
/// ```ignore
/// use crate::interpreter::builtin_args::{FromValue, PyStr, ...};
/// // 1. Reject unknown kwargs + collect positionals.
/// let __positional = crate::interpreter::builtin_args::validate_kwargs_and_collect_positional(
///     __pyrust_args, FN_NAME, &["path", "mode"],
/// )?;
/// // 2. Bound on positional count.
/// crate::interpreter::builtin_args::check_positional_count(
///     FN_NAME, __positional.len(), /* required */ 1, /* total */ 2,
/// )?;
/// // 3. Per-param extraction.
/// let path: PyStr = { ... resolve positional/kw, default or missing-arg, FromValue ... };
/// let mode: PyStr = { ... };
/// ```
fn emit_typed_prelude(params: &[TypedParam]) -> proc_macro2::TokenStream {
    // Names of every parameter that may be passed as a keyword argument.
    // Positional-only params are excluded so the kwarg-validation step
    // produces "got an unexpected keyword argument 'x'" for them.
    let allowed_kwargs: Vec<LitStr> = params
        .iter()
        .filter(|p| !p.positional_only)
        .map(|p| LitStr::new(&p.name.to_string(), p.name.span()))
        .collect();

    // Min positional = count of params with no default that aren't keyword-only.
    // Max positional = count of params that aren't keyword-only.
    let min_pos = params
        .iter()
        .filter(|p| !p.keyword_only && p.default.is_none())
        .count();
    let max_pos = params.iter().filter(|p| !p.keyword_only).count();

    // Fast path: every parameter is `#[positional_only]` (the
    // CPython-builtin shape — `abs`, `repr`, `len`, …).  No
    // SmallVec collection of positionals (the raw args slice IS the
    // positional list), no per-param kwarg lookup branch in
    // `locate_arg`.  Microbench (#403 perf-fix follow-up) showed the
    // collection + lookup ate ~5-10 ns/call on these one-shot Tier 1
    // builtins; the fast path closes most of that gap.
    let all_positional_only = !params.is_empty()
        && params.iter().all(|p| p.positional_only)
        && params.iter().all(|p| !p.keyword_only);

    if all_positional_only {
        return emit_typed_prelude_positional_only(params, min_pos, max_pos);
    }

    let mut per_param: Vec<proc_macro2::TokenStream> = Vec::new();
    // Track the position-in-signature for non-keyword-only params; this is
    // the index passed to `locate_arg`.  Keyword-only params are absent
    // from the positional list, so they get a sentinel value that the
    // `positional.get(_)` lookup never matches.
    const KW_ONLY_POS_SENTINEL: usize = usize::MAX;
    let mut pos_index: usize = 0;
    for p in params {
        let name_ident = &p.name;
        let name_lit = LitStr::new(&p.name.to_string(), p.name.span());
        let ty = &p.ty;
        let kw_allowed = !p.positional_only;
        let this_pos: usize = if p.keyword_only {
            KW_ONLY_POS_SENTINEL
        } else {
            let i = pos_index;
            pos_index += 1;
            i
        };

        let default_branch = match &p.default {
            Some(expr) => quote! { Ok::<#ty, crate::error::PyError>(#expr) },
            None => {
                quote! { crate::interpreter::builtin_args::missing_arg::<#ty>(FN_NAME, #name_lit) }
            }
        };

        per_param.push(quote! {
            let #name_ident: #ty = {
                let __found = crate::interpreter::builtin_args::locate_arg(
                    __pyrust_args, &__positional, FN_NAME, #name_lit, #this_pos, #kw_allowed,
                )?;
                match __found {
                    Some(__v) => <#ty as crate::interpreter::builtin_args::FromValue>::try_from_value(
                        __v, FN_NAME, #name_lit,
                    )?,
                    None => (#default_branch)?,
                }
            };
        });
    }

    quote! {
        // Pull the validation helpers + FromValue trait into scope.
        #[allow(unused_imports)]
        use crate::interpreter::builtin_args::FromValue as _;
        let __positional: crate::interpreter::builtin_args::PositionalArgs<'_> =
            crate::interpreter::builtin_args::validate_kwargs_and_collect_positional(
                __pyrust_args,
                FN_NAME,
                &[#(#allowed_kwargs),*],
            )?;
        crate::interpreter::builtin_args::check_positional_count(
            FN_NAME,
            __positional.len(),
            #min_pos,
            #max_pos,
        )?;
        #(#per_param)*
    }
}

/// Tighter typed-prelude emit for signatures whose every parameter is
/// `#[positional_only]`.  Skips the `SmallVec<&ExpandedCallArg>`
/// collection that the general path uses — `__pyrust_args` is already
/// the positional list when no kwargs can be legal — and avoids the
/// kwarg-branch in `locate_arg`.  Same wording / same semantics; just
/// less work per call.  Reachable when `all_positional_only` is true
/// in [`emit_typed_prelude`].
fn emit_typed_prelude_positional_only(
    params: &[TypedParam],
    min_pos: usize,
    max_pos: usize,
) -> proc_macro2::TokenStream {
    let mut per_param: Vec<proc_macro2::TokenStream> = Vec::new();
    for (idx, p) in params.iter().enumerate() {
        let name_ident = &p.name;
        let name_lit = LitStr::new(&p.name.to_string(), p.name.span());
        let ty = &p.ty;

        let default_branch = match &p.default {
            Some(expr) => quote! { Ok::<#ty, crate::error::PyError>(#expr) },
            None => {
                quote! { crate::interpreter::builtin_args::missing_arg::<#ty>(FN_NAME, #name_lit) }
            }
        };

        per_param.push(quote! {
            let #name_ident: #ty = {
                match __pyrust_args.get(#idx).map(|__a| &__a.value) {
                    Some(__v) => <#ty as crate::interpreter::builtin_args::FromValue>::try_from_value(
                        __v, FN_NAME, #name_lit,
                    )?,
                    None => (#default_branch)?,
                }
            };
        });
    }

    quote! {
        // Fast path — all parameters are `#[positional_only]`, so any
        // keyword argument is a user error and the raw `__pyrust_args`
        // slice IS the positional list.
        #[allow(unused_imports)]
        use crate::interpreter::builtin_args::FromValue as _;
        crate::interpreter::builtin_args::reject_named_args(__pyrust_args, FN_NAME)?;
        crate::interpreter::builtin_args::check_positional_count(
            FN_NAME,
            __pyrust_args.len(),
            #min_pos,
            #max_pos,
        )?;
        #(#per_param)*
    }
}

/// Emit a singleton group's worth of artefacts for an overload set:
/// one shared name static, N typed-arg body fns, one dispatcher, one
/// reg entry, returning the artefacts in the form
/// [`pyrust_module`]'s main loop expects.
///
/// Generated shape (for `abs(PyInt)` + `abs(PyFloat)` + `abs(PyValue)`):
///
/// ```ignore
/// static __pyfn_abs_NAME: LazyLock<&'static str> = …;
///
/// fn __pyfn_abs_overload_0(_interp, x: PyInt<'_>) -> Result<Value> { /* int body */ }
/// fn __pyfn_abs_overload_1(_interp, x: PyFloat)   -> Result<Value> { /* float body */ }
/// fn __pyfn_abs_overload_2(_interp, x: PyValue)   -> Result<Value> { /* catch-all */ }
///
/// fn __pyfn_abs(_interp, __pyrust_args: &[ExpandedCallArg]) -> Result<Value> {
///     let FN_NAME = *__pyfn_abs_NAME;
///     // Phase 1 — shared kwarg + arity validation.
///     let __positional = validate_kwargs_and_collect_positional(__pyrust_args, FN_NAME, &["x"])?;
///     check_positional_count(FN_NAME, __positional.len(), 1, 1)?;
///     let __arg_x = locate_arg(__pyrust_args, &__positional, FN_NAME, "x", 0, true)?
///         .ok_or_else(|| missing_arg::<()>(FN_NAME, "x").unwrap_err())?;
///     // Phase 2 — try each overload's predicate in declaration order.
///     if <PyInt as FromValue>::matches(__arg_x) {
///         let x = <PyInt as FromValue>::try_from_value(__arg_x, FN_NAME, "x")?;
///         return __pyfn_abs_overload_0(_interp, x);
///     }
///     // … same for PyFloat, then PyValue (always matches) …
///     no_overload_matched(FN_NAME, &[…actual types…])
/// }
/// ```
fn emit_overload_set_artefacts(
    group: &[&ModuleFn],
    py_short: &str,
    rust_short: &str,
) -> syn::Result<(
    Vec<proc_macro2::TokenStream>,
    proc_macro2::TokenStream,
    Ident,
)> {
    let head = group[0];
    let head_span = head.short_name.span();
    let suffix_lit = LitStr::new(py_short, head_span);
    let name_static_ident = format_ident!("__pyfn_{}_NAME", rust_short, span = head_span);
    let dispatcher_ident = format_ident!("__pyfn_{}", rust_short, span = head_span);

    // The reference signature (validated to be uniform) — use it to drive
    // Phase 1 of the dispatcher.
    let ref_params = match &head.args {
        ModuleFnArgs::Typed { params } => params,
        // group_funcs_by_name's validation already rejected non-typed
        // overloads; this is unreachable in practice.
        ModuleFnArgs::Legacy { .. } => unreachable!("legacy in overload set"),
    };

    let allowed_kwargs: Vec<LitStr> = ref_params
        .iter()
        .filter(|p| !p.positional_only)
        .map(|p| LitStr::new(&p.name.to_string(), p.name.span()))
        .collect();
    let min_pos = ref_params.iter().filter(|p| !p.keyword_only).count();
    let max_pos = min_pos; // overload sets disallow defaults, so min == max

    // Mirror of the single-body fast-path detection.  All-positional-only
    // overload sets (the CPython-builtin shape — `abs`, `hex`, …) avoid
    // the SmallVec collection and per-arg kwarg-branch.
    let all_positional_only = !ref_params.is_empty()
        && ref_params.iter().all(|p| p.positional_only)
        && ref_params.iter().all(|p| !p.keyword_only);

    // For each parameter, the same positional-index / kw-allowed pair the
    // single-body prelude would compute — overload sets share this.
    let mut per_arg_locate: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut pos_index: usize = 0;
    const KW_ONLY_POS_SENTINEL: usize = usize::MAX;
    for p in ref_params {
        let name_lit = LitStr::new(&p.name.to_string(), p.name.span());
        let __arg_ident = format_ident!("__arg_{}", p.name, span = p.name.span());
        let kw_allowed = !p.positional_only;
        let this_pos: usize = if p.keyword_only {
            KW_ONLY_POS_SENTINEL
        } else {
            let i = pos_index;
            pos_index += 1;
            i
        };
        if all_positional_only {
            // Fast path: direct slice indexing, no kwarg branch.  Since
            // `check_positional_count` already enforces min == max == arity,
            // the `get(idx)` is always Some here — the `ok_or_else` is
            // defence-in-depth and unreachable in practice.
            let idx = this_pos;
            per_arg_locate.push(quote! {
                let #__arg_ident: &crate::value::Value = __pyrust_args
                    .get(#idx)
                    .map(|__a| &__a.value)
                    .ok_or_else(|| crate::interpreter::builtin_args::missing_arg::<()>(
                        FN_NAME, #name_lit,
                    ).unwrap_err())?;
            });
        } else {
            per_arg_locate.push(quote! {
                let #__arg_ident: &crate::value::Value =
                    crate::interpreter::builtin_args::locate_arg(
                        __pyrust_args, &__positional, FN_NAME, #name_lit, #this_pos, #kw_allowed,
                    )?
                    .ok_or_else(|| crate::interpreter::builtin_args::missing_arg::<()>(
                        FN_NAME, #name_lit,
                    ).unwrap_err())?;
            });
        }
    }

    // Per-overload body fns + dispatcher branches.
    let mut body_fns: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut dispatch_branches: Vec<proc_macro2::TokenStream> = Vec::new();
    for (idx, f) in group.iter().enumerate() {
        let params = match &f.args {
            ModuleFnArgs::Typed { params } => params,
            ModuleFnArgs::Legacy { .. } => unreachable!(),
        };
        let body_ident = format_ident!(
            "__pyfn_{}_overload_{}",
            rust_short,
            idx,
            span = f.short_name.span()
        );

        // Compose the typed-arg list for the body fn.
        let body_typed_params: Vec<proc_macro2::TokenStream> = params
            .iter()
            .map(|p| {
                let n = &p.name;
                let t = &p.ty;
                quote!(#n: #t)
            })
            .collect();

        // The body itself — same shape as a single-body typed fn, minus
        // the kwarg/arity prelude (the dispatcher already did that work).
        let attrs = &f.attrs;
        let body_stmts = &f.body.stmts;
        body_fns.push(quote! {
            #[allow(non_snake_case)]
            #(#attrs)*
            fn #body_ident(
                _interp: &mut crate::Interpreter,
                #(#body_typed_params),*
            ) -> crate::error::Result<crate::value::Value> {
                #[allow(non_snake_case)]
                let FN_NAME: &'static str = *#name_static_ident;
                let _ = FN_NAME;
                #(#body_stmts)*
            }
        });

        // Dispatcher branch: predicate over every param, then convert + call.
        let predicate_terms: Vec<proc_macro2::TokenStream> = params
            .iter()
            .map(|p| {
                let ty = &p.ty;
                let __arg_ident = format_ident!("__arg_{}", p.name, span = p.name.span());
                quote!(<#ty as crate::interpreter::builtin_args::FromValue>::matches(#__arg_ident))
            })
            .collect();
        let convert_args: Vec<proc_macro2::TokenStream> = params
            .iter()
            .map(|p| {
                let n = &p.name;
                let ty = &p.ty;
                let name_lit = LitStr::new(&p.name.to_string(), p.name.span());
                let __arg_ident = format_ident!("__arg_{}", p.name, span = p.name.span());
                quote! {
                    let #n: #ty = <#ty as crate::interpreter::builtin_args::FromValue>::try_from_value(
                        #__arg_ident, FN_NAME, #name_lit,
                    )?;
                }
            })
            .collect();
        let call_args: Vec<&Ident> = params.iter().map(|p| &p.name).collect();
        dispatch_branches.push(quote! {
            if #(#predicate_terms)&&* {
                #(#convert_args)*
                return #body_ident(_interp, #(#call_args),*);
            }
        });
    }

    // Build the dispatcher.  When `__pyrust_args` is empty (zero-param
    // overload) we still emit the kwarg / count guard so unexpected
    // input is rejected the same way single-body builtins reject it.
    //
    // The `__actuals` slice formats the *actual* arg types observed at
    // the call site — used by `no_overload_matched` for the "no
    // overload matches" TypeError on the unreachable-when-PyValue path.
    let no_match_args: Vec<proc_macro2::TokenStream> = ref_params
        .iter()
        .map(|p| {
            let __arg_ident = format_ident!("__arg_{}", p.name, span = p.name.span());
            quote! { pyrust_core::builtin_type_name(#__arg_ident) }
        })
        .collect();

    let phase1 = if all_positional_only {
        // Fast path — every parameter is `#[positional_only]`, so any
        // keyword argument is a user error and the raw `__pyrust_args`
        // slice IS the positional list.  No SmallVec, no kwarg loop.
        quote! {
            crate::interpreter::builtin_args::reject_named_args(__pyrust_args, FN_NAME)?;
            crate::interpreter::builtin_args::check_positional_count(
                FN_NAME,
                __pyrust_args.len(),
                #min_pos,
                #max_pos,
            )?;
        }
    } else {
        quote! {
            let __positional: crate::interpreter::builtin_args::PositionalArgs<'_> =
                crate::interpreter::builtin_args::validate_kwargs_and_collect_positional(
                    __pyrust_args,
                    FN_NAME,
                    &[#(#allowed_kwargs),*],
                )?;
            crate::interpreter::builtin_args::check_positional_count(
                FN_NAME,
                __positional.len(),
                #min_pos,
                #max_pos,
            )?;
        }
    };

    let dispatcher = quote! {
        #[allow(non_snake_case)]
        fn #dispatcher_ident(
            _interp: &mut crate::Interpreter,
            __pyrust_args: &[ExpandedCallArg],
        ) -> crate::error::Result<crate::value::Value> {
            #[allow(non_snake_case)]
            let FN_NAME: &'static str = *#name_static_ident;
            let _ = FN_NAME;

            // Phase 1 — shared validation.
            #phase1
            #(#per_arg_locate)*

            // Phase 2 — try each overload in declaration order.
            #(#dispatch_branches)*

            // No overload matched.  Terse `unsupported argument type(s)`
            // wording matching CPython's binary-op error shape — actual
            // arg types only, no declared-signature dump (per the design
            // review on #395, comment 4443208232).  Unreachable when any
            // overload uses `PyValue` (whose `matches` is unconditional).
            let __actuals: &[&str] = &[#(#no_match_args),*];
            crate::interpreter::builtin_args::no_overload_matched::<crate::value::Value>(
                FN_NAME, __actuals,
            )
        }
    };

    let mut items: Vec<proc_macro2::TokenStream> = Vec::new();
    items.push(quote! {
        #[allow(non_upper_case_globals)]
        static #name_static_ident: std::sync::LazyLock<&'static str> =
            std::sync::LazyLock::new(|| {
                ::std::boxed::Box::leak(
                    format!("{}{}", FN_PREFIX, #suffix_lit).into_boxed_str(),
                )
            });
    });
    items.extend(body_fns);
    items.push(dispatcher);

    let reg_entry = quote! {
        crate::builtin_registry::BuiltinReg {
            name: *#name_static_ident,
            dispatch: #dispatcher_ident,
        }
    };

    Ok((items, reg_entry, name_static_ident))
}

/// Render a `syn::Type` to its canonical token string with all whitespace
/// removed.  Used by [`parse_fn_args`] to decide between the legacy slice
/// dialect (`&[ExpandedCallArg]`) and the typed dialect.
fn ty_to_canonical_string(ty: &syn::Type) -> String {
    // The `()` placeholder for "no annotation" must come back as `""` so
    // the legacy-detection path treats it as un-annotated.
    if let syn::Type::Tuple(t) = ty
        && t.elems.is_empty()
    {
        return String::new();
    }
    ty.to_token_stream()
        .to_string()
        .split_whitespace()
        .collect::<String>()
}

/// Compute the Python-level short name (with `r#` stripped, `#[py_name]`
/// applied if present) for one method-or-fn.  Returned alongside the
/// raw Rust-side ident for downstream codegen.
fn py_short_and_rust_short(f: &ModuleFn) -> (String, String) {
    let short = &f.short_name;
    let rust_short = short
        .to_string()
        .strip_prefix("r#")
        .map_or_else(|| short.to_string(), str::to_owned);
    let py_short = match &f.py_name_override {
        Some(lit) => lit.value(),
        None => rust_short.clone(),
    };
    (py_short, rust_short)
}

/// Group module-level `fn` declarations by Python-level short name.
/// Singleton groups → single-body builtins.  Multi-fn groups → overload
/// sets that get dispatched at the same registry name; the per-group
/// order preserves source order (which the dispatcher relies on — first
/// matching overload wins).  Groups themselves are returned in
/// source-order-of-first-declaration so emit output is deterministic.
///
/// Returns an error if any overload set is internally inconsistent —
/// see [`validate_overload_set`] for the rules.
fn group_funcs_by_name(funcs: &[ModuleFn]) -> syn::Result<Vec<Vec<&ModuleFn>>> {
    let mut groups: Vec<Vec<&ModuleFn>> = Vec::new();
    let mut name_to_idx: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for f in funcs {
        let (py_short, _) = py_short_and_rust_short(f);
        match name_to_idx.get(&py_short).copied() {
            Some(idx) => groups[idx].push(f),
            None => {
                name_to_idx.insert(py_short, groups.len());
                groups.push(vec![f]);
            }
        }
    }
    for group in &groups {
        if group.len() > 1 {
            validate_overload_set(group)?;
        }
    }
    Ok(groups)
}

/// Enforce structural compatibility across overloads of a single
/// Python-level name.  Rules (v1):
///
/// 1. All overloads must use the **typed** dialect — mixing the legacy
///    `(args)` form into an overload set would defeat the type-based
///    dispatch.
/// 2. Same **arity** (parameter count).  Different arities should live
///    on separate Python names; for "0-or-1 trailing arg" patterns,
///    declare a single signature with `Option<T>` + `#[default(None)]`.
/// 3. Same **parameter names** in the same positions, and same
///    `#[positional_only]` / `#[keyword_only]` flags.  Otherwise the
///    dispatcher's shared kwarg-validation step couldn't unify them.
/// 4. **No `#[default(...)]`** on any parameter.  The dispatcher
///    applies defaults before knowing which overload matches, but the
///    default's *type* would have to be compatible with every
///    overload's parameter — that's the same chicken-and-egg the
///    overload mechanism is meant to break.  Mix overloads with
///    defaults via a single signature using a typed-overload-aware
///    helper inside the body.
fn validate_overload_set(group: &[&ModuleFn]) -> syn::Result<()> {
    // Reference signature — the first overload — defines the shape.
    let reference = group[0];
    let ref_params = match &reference.args {
        ModuleFnArgs::Typed { params } => params,
        ModuleFnArgs::Legacy { args_ident } => {
            return Err(syn::Error::new(
                args_ident.span(),
                "overload sets must use the typed dialect — the legacy `(args)` \
                 form cannot participate in type-based dispatch",
            ));
        }
    };
    for p in ref_params {
        if p.default.is_some() {
            return Err(syn::Error::new(
                p.name.span(),
                format!(
                    "parameter `{}` has a `#[default(...)]`, which is not \
                     allowed in overload sets — declare a single signature \
                     instead, or move the default into the dispatched body",
                    p.name,
                ),
            ));
        }
    }
    for (idx, f) in group.iter().enumerate().skip(1) {
        let params = match &f.args {
            ModuleFnArgs::Typed { params } => params,
            ModuleFnArgs::Legacy { args_ident } => {
                return Err(syn::Error::new(
                    args_ident.span(),
                    "overload sets must use the typed dialect — the legacy \
                     `(args)` form cannot participate in type-based dispatch",
                ));
            }
        };
        if params.len() != ref_params.len() {
            return Err(syn::Error::new(
                f.short_name.span(),
                format!(
                    "overload #{idx} has {} parameter(s) but the first \
                     overload has {}; all overloads must share the same arity",
                    params.len(),
                    ref_params.len(),
                ),
            ));
        }
        for (i, (a, b)) in ref_params.iter().zip(params.iter()).enumerate() {
            if a.name != b.name {
                return Err(syn::Error::new(
                    b.name.span(),
                    format!(
                        "overload parameter #{i} is named `{}` here but `{}` \
                         in the first overload; all overloads must agree on \
                         parameter names",
                        b.name, a.name,
                    ),
                ));
            }
            if a.positional_only != b.positional_only {
                return Err(syn::Error::new(
                    b.name.span(),
                    format!(
                        "parameter `{}` has conflicting `#[positional_only]` \
                         flags across overloads",
                        b.name,
                    ),
                ));
            }
            if a.keyword_only != b.keyword_only {
                return Err(syn::Error::new(
                    b.name.span(),
                    format!(
                        "parameter `{}` has conflicting `#[keyword_only]` \
                         flags across overloads",
                        b.name,
                    ),
                ));
            }
            if b.default.is_some() {
                return Err(syn::Error::new(
                    b.name.span(),
                    format!(
                        "parameter `{}` has a `#[default(...)]`, which is \
                         not allowed in overload sets",
                        b.name,
                    ),
                ));
            }
        }
    }

    // Catch-all-must-be-last (per design review on #395, concern 1).
    //
    // `PyValue::matches` is unconditional, so any overload after a
    // `PyValue`-only one is reachable only via the (currently absent)
    // path that doesn't match `PyValue` — i.e. never.  Emit a clear
    // diagnostic at macro-expand time so users don't get silent dead
    // code.  An overload counts as a "catch-all" only when *every*
    // parameter is `PyValue` (a mixed `(PyValue, PyInt)` overload
    // still requires the second arg to actually be an int, so later
    // overloads remain reachable for non-int second-args).
    for (idx, f) in group.iter().enumerate() {
        if idx == group.len() - 1 {
            // The last overload is allowed to be a catch-all — that's
            // the whole point of the pattern.
            break;
        }
        let params = match &f.args {
            ModuleFnArgs::Typed { params } => params,
            ModuleFnArgs::Legacy { .. } => unreachable!("checked above"),
        };
        if is_catch_all_overload(params) {
            return Err(syn::Error::new(
                f.short_name.span(),
                "a `PyValue`-only catch-all overload must be declared last — \
                 every overload after it would be unreachable (silently \
                 shadowed by `PyValue::matches`, which always returns true)",
            ));
        }
    }

    Ok(())
}

/// Returns true if every parameter in `params` is an *unconditional*
/// matcher — either bare `PyValue` or `Option<PyValue>`.  Both shadow
/// every overload declared after them (the former because
/// `PyValue::matches` is unconditional, the latter because `None ||
/// PyValue::matches` is also unconditional).  Per the review on #397:
/// without the `Option<PyValue>` arm, that form silently slips past
/// the catch-all-must-be-last guard.
fn is_catch_all_overload(params: &[TypedParam]) -> bool {
    if params.is_empty() {
        // A zero-arg overload is its own thing — kwarg validation
        // catches mismatches, so the "catch-all" question doesn't apply.
        return false;
    }
    params.iter().all(|p| is_unconditional_matcher_ty(&p.ty))
}

/// True if the given type's `FromValue::matches` is unconditional —
/// `PyValue` and `Option<PyValue>` (in any qualified form).  Used by
/// the catch-all-must-be-last check; missing one of these arms would
/// let an overload silently shadow everything after it.
fn is_unconditional_matcher_ty(ty: &syn::Type) -> bool {
    let s = ty_to_canonical_string(ty);
    // Users typically `use` the wrapper and write it bare; the
    // qualified form is supported defensively.
    s == "PyValue"
        || s.ends_with("::PyValue")
        || s == "Option<PyValue>"
        || s.ends_with("<PyValue>") && s.starts_with("Option<")
        || s.ends_with("::PyValue>") && s.starts_with("Option<")
}

/// File-scoped module declaration: see crate-level docs for the full
/// syntax and the contract with `pyrust_builtin_modules!`.
#[proc_macro]
pub fn pyrust_module(input: TokenStream) -> TokenStream {
    let ModuleInput {
        constants,
        funcs,
        classes,
    } = parse_macro_input!(input as ModuleInput);

    let mut fn_items = Vec::new();
    let mut reg_entries: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut attr_entries: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut class_items: Vec<proc_macro2::TokenStream> = Vec::new();

    // ── module-level fns ────────────────────────────────────────────────
    //
    // Group declarations by their Python-level short name.  A singleton
    // group emits the existing single-body shape; multi-fn groups emit
    // an overload set (dispatcher + per-overload body fns).  Source
    // order is preserved so the dispatcher tries overloads in
    // declaration order — strict overloads first, `PyValue` catch-all
    // last, by convention.
    let groups = match group_funcs_by_name(&funcs) {
        Ok(g) => g,
        Err(e) => return e.to_compile_error().into(),
    };
    for group in &groups {
        let (py_short, rust_short) = py_short_and_rust_short(group[0]);
        let short = &group[0].short_name;
        let short_lit = LitStr::new(&py_short, short.span());

        let (group_fn_items, reg_entry, name_static_ident) = if group.len() == 1 {
            let (fn_item, reg_entry, name_static_ident) =
                emit_fn_artefacts(group[0], &py_short, &rust_short);
            (vec![fn_item], reg_entry, name_static_ident)
        } else {
            match emit_overload_set_artefacts(group, &py_short, &rust_short) {
                Ok(triple) => triple,
                Err(e) => return e.to_compile_error().into(),
            }
        };

        for it in group_fn_items {
            fn_items.push(it);
        }
        reg_entries.push(reg_entry);
        attr_entries.push(quote! {
            attrs.insert(
                #short_lit.to_string(),
                crate::value::Value::builtin_function(*#name_static_ident),
            );
        });
    }

    // ── classes (`class Foo { fn … }`) ──────────────────────────────────
    //
    // Each class's methods register as `"<FN_PREFIX><ClassName>.<method>"`.
    // The class is constructed at `module()` build time as a `PyClass`
    // whose `attrs` map every method's short name → its leaked
    // BuiltinFunction Value.  pyrust's existing class machinery
    // (subscript / iter / call / __init__ / __len__ / __bool__ / __contains__
    // / __delitem__ / __setitem__ / __next__ dispatch sites — all
    // recently unified through `invoke_class_method`) picks up dunders
    // without per-type plumbing.
    for class in &classes {
        let class_name_ident = &class.name;
        let class_name_str = class_name_ident.to_string();
        let class_name_lit = LitStr::new(&class_name_str, class_name_ident.span());
        let class_attrs = &class.attrs;

        let mut method_attr_inserts: Vec<proc_macro2::TokenStream> = Vec::new();
        for method in &class.methods {
            let (py_short, rust_short) = py_short_and_rust_short(method);
            let method_short = &method.short_name;
            let short_lit = LitStr::new(&py_short, method_short.span());
            // Method's Python-level name is `<ClassName>.<method>` so the
            // registry key tells you which class it belongs to.
            let name_suffix = format!("{class_name_str}.{py_short}");
            // Rust-side namespacing — `__pyfn_<Class>__<method>` — avoids
            // collisions between two classes' `__init__` etc.
            let rust_suffix = format!("{class_name_str}__{rust_short}");
            let (fn_item, reg_entry, name_static_ident) =
                emit_fn_artefacts(method, &name_suffix, &rust_suffix);
            fn_items.push(fn_item);
            reg_entries.push(reg_entry);
            method_attr_inserts.push(quote! {
                attrs.insert(
                    #short_lit.to_string(),
                    crate::value::Value::builtin_function(*#name_static_ident),
                );
            });
        }

        // Build the PyClass at module() time.  `class_name` is *not*
        // qualified (it's just `Counter`, not `collections.Counter`) so
        // `type(c).__name__ == "Counter"` matches CPython.
        //
        // `class_attrs` (the `///` doc comments on the `class { … }`
        // block) are intentionally dropped here.  They document the
        // class for humans reading the source; emitting them in front
        // of `attrs.insert(...)` would trip `unused_doc_comments`
        // because that's a statement, not an item.  Accept the comment
        // as source-only documentation rather than fighting the lint.
        let _suppress_unused = class_attrs; // silence the field-read warning
        class_items.push(quote! {
            attrs.insert(#class_name_lit.to_string(), {
                use std::cell::RefCell;
                use std::rc::Rc;
                use indexmap::IndexMap;
                let mut attrs: IndexMap<String, crate::value::Value> = IndexMap::new();
                #(#method_attr_inserts)*
                crate::value::Value::py_class(Rc::new(RefCell::new(crate::value::PyClass {
                    name: #class_name_lit.to_string(),
                    base: None,
                    attrs,
                })))
            });
        });
    }

    let const_entries: Vec<proc_macro2::TokenStream> = constants
        .iter()
        .map(|(k, v)| {
            quote! {
                attrs.insert(#k.to_string(), #v);
            }
        })
        .collect();

    let expanded = quote! {
        // Per-function bodies.
        #(#fn_items)*

        /// The per-module registry slice.  Composed at first call by
        /// leaking `FN_PREFIX + short_name` into a `'static` string for
        /// each entry.  Consumed by `crate::builtin_modules::all_regs`.
        pub(crate) fn regs() -> &'static [crate::builtin_registry::BuiltinReg] {
            static REGS_CELL: std::sync::LazyLock<Vec<crate::builtin_registry::BuiltinReg>> =
                std::sync::LazyLock::new(|| {
                    vec![
                        #(#reg_entries),*
                    ]
                });
            REGS_CELL.as_slice()
        }

        // Back-compat: the previous design exposed `pub(crate) const
        // REGS: &[BuiltinReg]`.  Some callers may still reference that;
        // expose a fn of the same name as a thin shim.  (Hidden — the
        // canonical entry point is `regs()`.)
        #[doc(hidden)]
        pub(crate) fn REGS() -> &'static [crate::builtin_registry::BuiltinReg] {
            regs()
        }

        /// Build the PyModule for this built-in module.  Called from the
        /// interpreter's `load_module` path on first `import`.
        pub(crate) fn module() -> crate::value::Value {
            use std::cell::RefCell;
            use std::collections::HashMap;
            use std::rc::Rc;
            let mut attrs: HashMap<String, crate::value::Value> = HashMap::new();
            #(#const_entries)*
            #(#attr_entries)*
            // `class` blocks expand to `attrs.insert(<ClassName>, …PyClass…)`.
            // Each PyClass is built with its method table populated from
            // the registry-leaked `&'static str` names — see
            // `emit_fn_artefacts` for the alloc-once leak pattern that's
            // shared with module-level fns.
            #(#class_items)*
            crate::value::Value::py_module(Rc::new(RefCell::new(crate::value::PyModule {
                name: MODULE_NAME.to_string(),
                attrs,
            })))
        }
    };

    expanded.into()
}
