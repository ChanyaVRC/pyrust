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
    let const_ident = Ident::new(&fn_ident.to_string().to_uppercase(), fn_ident.span());

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
}

/// A single `fn` declaration inside `pyrust_module!`.
struct ModuleFn {
    attrs: Vec<syn::Attribute>,
    short_name: Ident,
    args_ident: Ident,
    body: Block,
}

impl Parse for ModuleInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Optional `constants { ... }` block — function declarations begin
        // with the `fn` keyword, so peeking for an Ident here cleanly
        // distinguishes the two cases.
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
            } else {
                return Err(syn::Error::new(
                    lookahead.span(),
                    format!(
                        "unexpected `{lookahead}`; expected either `constants {{ … }}` or a `fn` declaration",
                    ),
                ));
            }
        }

        // One or more `fn` declarations.
        let mut funcs: Vec<ModuleFn> = Vec::new();
        while !input.is_empty() {
            let attrs = input.call(syn::Attribute::parse_outer)?;
            let _fn_kw: Token![fn] = input.parse()?;
            // Function ident must be snake_case so the generated
            // SCREAMING_SNAKE constant name is unique without lossy
            // case-folding (e.g. `fn isDir` and `fn isdir` would both
            // produce the same const ident).
            let short_name: Ident = input.parse()?;
            let short_str = short_name.to_string();
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
            funcs.push(ModuleFn {
                attrs,
                short_name,
                args_ident,
                body,
            });
        }

        Ok(ModuleInput { constants, funcs })
    }
}

/// Returns true if `s` is a Rust snake_case identifier — lowercase
/// letters, digits, and underscores only.
fn is_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// File-scoped module declaration: see crate-level docs for the full
/// syntax and the contract with `pyrust_builtin_modules!`.
#[proc_macro]
pub fn pyrust_module(input: TokenStream) -> TokenStream {
    let ModuleInput { constants, funcs } = parse_macro_input!(input as ModuleInput);

    let mut fn_items = Vec::new();
    let mut reg_entries: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut attr_entries: Vec<proc_macro2::TokenStream> = Vec::new();

    for f in &funcs {
        let short = &f.short_name;
        let short_str = short.to_string();
        // Rust ident for the dispatch fn — prefix with the parent
        // module's basename (taken from the build artifact's symbol
        // mangling).  We can't read `MODULE_NAME` at proc-macro time, but
        // since each module's contents land in a distinct Rust `mod`, the
        // short name alone is sufficient for uniqueness *within* a
        // module.  Still prefix with `__pyfn_` to keep the generated
        // idents from clashing with user-defined helper functions.
        let rust_fn_ident = format_ident!("__pyfn_{}", short_str, span = short.span());
        let attrs = &f.attrs;
        let body_stmts = &f.body.stmts;
        let args_ident = &f.args_ident;
        let short_lit = LitStr::new(&short_str, short.span());

        fn_items.push(quote! {
            #(#attrs)*
            fn #rust_fn_ident(
                _interp: &mut crate::Interpreter,
                #args_ident: &[crate::interpreter::ExpandedCallArg],
            ) -> crate::error::Result<crate::value::Value> {
                // Compose the Python-level full name once per fn (lazy)
                // so callers (helpers, error messages) can refer to it as
                // `FN_NAME` instead of repeating the module prefix.
                // `FN_PREFIX` is injected by `pyrust_builtin_modules!` and is
                // either `"<module>."` (prefixed module) or `""` (flat / builtins).
                static FN_NAME_OWNED: std::sync::LazyLock<String> =
                    std::sync::LazyLock::new(|| {
                        format!("{}{}", FN_PREFIX, #short_lit)
                    });
                #[allow(non_snake_case)]
                let FN_NAME: &str = FN_NAME_OWNED.as_str();
                let _ = FN_NAME; // suppress unused warning if body ignores it
                #(#body_stmts)*
            }
        });

        // Each registry entry composes its name from FN_PREFIX at
        // first lookup, then leaks the string to satisfy
        // `BuiltinReg.name: &'static str`.  Cost: one allocation per
        // built-in at startup, amortised over every subsequent dispatch.
        reg_entries.push(quote! {
            crate::builtin_registry::BuiltinReg {
                name: ::std::boxed::Box::leak(
                    format!("{}{}", FN_PREFIX, #short_lit).into_boxed_str()
                ),
                dispatch: #rust_fn_ident,
            }
        });

        attr_entries.push(quote! {
            attrs.insert(
                #short_str.to_string(),
                crate::value::Value::builtin_function(
                    ::std::boxed::Box::leak(
                        format!("{}{}", FN_PREFIX, #short_lit).into_boxed_str()
                    ),
                ),
            );
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
            crate::value::Value::py_module(Rc::new(RefCell::new(crate::value::PyModule {
                name: MODULE_NAME.to_string(),
                attrs,
            })))
        }
    };

    expanded.into()
}
