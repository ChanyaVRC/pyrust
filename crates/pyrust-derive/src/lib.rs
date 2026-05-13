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
    args_ident: Ident,
    body: Block,
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
        let args_ident: Ident = arg_content.parse()?;
        // If the user wrote a type annotation, validate it.
        if arg_content.peek(Token![:]) {
            let _: Token![:] = arg_content.parse()?;
            let ty: syn::Type = arg_content.parse()?;
            let ty_str = ty
                .to_token_stream()
                .to_string()
                .split_whitespace()
                .collect::<String>();
            let accepted = matches!(
                ty_str.as_str(),
                "&[ExpandedCallArg]" | "&[crate::interpreter::ExpandedCallArg]"
            );
            if !accepted {
                return Err(syn::Error::new(
                    ty.span(),
                    "`pyrust_module!` fn args type must be `&[ExpandedCallArg]` \
                     (the macro injects the canonical signature; omit the type \
                     annotation entirely if unsure).",
                ));
            }
        }
        if !arg_content.is_empty() {
            return Err(arg_content.error(
                "unexpected token in fn argument list; \
                 `pyrust_module!` accepts only `(args)` or `(args: &[ExpandedCallArg])`",
            ));
        }
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
            args_ident,
            body,
        })
    }

    /// Parse one `class ClassName { fn method(args) … fn method(args) … }`
    /// declaration.  The `class` ident is at the head of `input`.
    fn parse_class(input: ParseStream, attrs: Vec<syn::Attribute>) -> syn::Result<ModuleClass> {
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
    let args_ident = &f.args_ident;
    let suffix_lit = LitStr::new(name_suffix, short.span());

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
            #args_ident: &[ExpandedCallArg],
        ) -> Result<Value> {
            #[allow(non_snake_case)]
            let FN_NAME: &'static str = *#name_static_ident;
            let _ = FN_NAME; // suppress unused warning if body ignores it
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
    // The Rust ident is always derived from the fn name (with `r#` stripped
    // off raw idents).  The Python-level name is the same *unless* the
    // user supplied `#[py_name = "..."]` — used for Python names that are
    // strict Rust keywords with no raw form (`super`).
    for f in &funcs {
        let (py_short, rust_short) = py_short_and_rust_short(f);
        let short = &f.short_name;
        let short_lit = LitStr::new(&py_short, short.span());
        let (fn_item, reg_entry, name_static_ident) = emit_fn_artefacts(f, &py_short, &rust_short);
        fn_items.push(fn_item);
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
                use std::collections::HashMap;
                use std::rc::Rc;
                let mut attrs: HashMap<String, crate::value::Value> = HashMap::new();
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
