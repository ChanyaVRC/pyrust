//! `pyrust-derive` — proc-macros for declaring built-in Python modules in Rust.
//!
//! Two complementary entry points:
//!
//! - **`pyrust_module! { … }`** (function-like) — declares a whole built-in
//!   module in one place: name, optional constants, and a sequence of
//!   functions whose Python names are derived from the Rust ident with the
//!   module prefix applied automatically.  Emits:
//!     - each `fn` with the unified
//!       `fn(&mut Interpreter, &[ExpandedCallArg]) -> Result<Value>`
//!       signature,
//!     - one `BuiltinReg` constant per function,
//!     - a `pub(crate) const REGS: &[BuiltinReg]` slice for the central
//!       registry,
//!     - a `pub fn module() -> Value` PyModule constructor consumed by the
//!       interpreter's `load_module`.
//!
//! - **`#[pyfunction(name = "module.fn")]`** (attribute) — drop-in for
//!   one-off built-ins that don't fit a whole module file (e.g. moving a
//!   single arm of the legacy cascade incrementally).  Same output as a
//!   single `fn` inside `pyrust_module!` minus the `REGS`/`module()`
//!   plumbing.
//!
//! Either form pairs cleanly with the central `crate::builtin_registry`.
//!
//! ## `pyrust_module!` syntax
//!
//! ```ignore
//! pyrust_module! {
//!     name = "math",
//!
//!     constants {
//!         "pi" => Value::float(std::f64::consts::PI),
//!         "e"  => Value::float(std::f64::consts::E),
//!     }
//!
//!     /// CPython: math.sqrt(x) → float.
//!     fn sqrt(args) -> Result<Value> {
//!         Ok(Value::float(single_float("math.sqrt", args)?.sqrt()))
//!     }
//!
//!     /// CPython: math.pow(x, y) → float.
//!     fn pow(args) -> Result<Value> {
//!         reject_keyword_args_expanded("math.pow", args)?;
//!         /* … */
//!     }
//! }
//! ```
//!
//! Each `fn foo(args)` is expanded to
//! `fn math_foo(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value>`,
//! registered as `"math.foo"`, and listed in `REGS`.  The `module()` fn
//! returns a `PyModule` whose `attrs` contain the declared constants plus
//! every function as `Value::builtin_function("math.foo")`.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{
    Block, Expr, Ident, ItemFn, LitStr, Meta, Token, parse_macro_input, punctuated::Punctuated,
};

/// `#[pyfunction(name = "module.fn")]` — generates a sibling registration
/// constant so the function is picked up by the built-in dispatch registry.
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

// ─── `pyrust_module!` ───────────────────────────────────────────────────────

/// Parsed `pyrust_module! { … }` input.
struct ModuleInput {
    module_name: LitStr,
    constants: Vec<(LitStr, Expr)>,
    funcs: Vec<ModuleFn>,
}

/// A single `fn` declaration inside `pyrust_module!`.  Looks like:
/// `[doc-attrs] fn name(args) -> Result<Value> { body }`.
struct ModuleFn {
    attrs: Vec<syn::Attribute>,
    short_name: Ident,
    args_ident: Ident,
    body: Block,
}

impl Parse for ModuleInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // `name = "module"`
        let name_ident: Ident = input.parse()?;
        if name_ident != "name" {
            return Err(syn::Error::new(name_ident.span(), "expected `name`"));
        }
        let _eq: Token![=] = input.parse()?;
        let module_name: LitStr = input.parse()?;
        let _comma: Token![,] = input.parse()?;

        // Optional `constants { ... }` block.
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
        }

        // One or more `fn` declarations.
        let mut funcs: Vec<ModuleFn> = Vec::new();
        while !input.is_empty() {
            let attrs = input.call(syn::Attribute::parse_outer)?;
            let _fn_kw: Token![fn] = input.parse()?;
            let short_name: Ident = input.parse()?;
            let arg_content;
            syn::parenthesized!(arg_content in input);
            let args_ident: Ident = arg_content.parse()?;
            // optional remainder of the args-declaration is ignored — we
            // always use the canonical `args: &[ExpandedCallArg]` shape.
            let _ = arg_content.parse::<proc_macro2::TokenStream>()?;
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

        Ok(ModuleInput {
            module_name,
            constants,
            funcs,
        })
    }
}

/// Function-like proc-macro: see crate-level docs for syntax.
#[proc_macro]
pub fn pyrust_module(input: TokenStream) -> TokenStream {
    let ModuleInput {
        module_name,
        constants,
        funcs,
    } = parse_macro_input!(input as ModuleInput);

    let module_name_str = module_name.value();

    // For each fn: build the full fn definition, its BuiltinReg const, and
    // collect the const ident for the REGS slice + the (short-name, Python
    // name) pair for the module() attrs.
    let mut fn_items = Vec::new();
    let mut reg_consts = Vec::new();
    let mut reg_idents: Vec<Ident> = Vec::new();
    let mut attr_entries: Vec<proc_macro2::TokenStream> = Vec::new();

    for f in &funcs {
        let short = &f.short_name;
        let short_str = short.to_string();
        let py_name = format!("{}.{}", module_name_str, short_str);
        let py_name_lit = LitStr::new(&py_name, short.span());
        let rust_fn_ident = format_ident!(
            "{}_{}",
            module_name_str.replace('.', "_"),
            short_str,
            span = short.span()
        );
        let const_ident = Ident::new(&rust_fn_ident.to_string().to_uppercase(), short.span());
        let attrs = &f.attrs;
        let body = &f.body;
        let args_ident = &f.args_ident;

        fn_items.push(quote! {
            #(#attrs)*
            fn #rust_fn_ident(
                _interp: &mut crate::Interpreter,
                #args_ident: &[crate::interpreter::ExpandedCallArg],
            ) -> crate::error::Result<crate::value::Value> {
                #body
            }
        });

        reg_consts.push(quote! {
            #[allow(non_upper_case_globals)]
            pub const #const_ident: crate::builtin_registry::BuiltinReg =
                crate::builtin_registry::BuiltinReg {
                    name: #py_name_lit,
                    dispatch: #rust_fn_ident,
                };
        });

        reg_idents.push(const_ident);
        attr_entries.push(quote! {
            attrs.insert(
                #short_str.to_string(),
                crate::value::Value::builtin_function(#py_name_lit),
            );
        });
    }

    // Constants → module() attrs.
    let const_entries: Vec<proc_macro2::TokenStream> = constants
        .iter()
        .map(|(k, v)| {
            quote! {
                attrs.insert(#k.to_string(), #v);
            }
        })
        .collect();

    let module_name_lit = module_name;

    let expanded = quote! {
        // Per-function bodies and BuiltinReg constants.
        #(#fn_items)*
        #(#reg_consts)*

        /// Slice of every `#py_name`-prefixed registration in this file —
        /// consumed by `crate::builtin_registry::REGISTRY`.
        pub(crate) const REGS: &[crate::builtin_registry::BuiltinReg] = &[
            #(#reg_idents),*
        ];

        /// Build the PyModule for this built-in module.  Called from the
        /// interpreter's `load_module` path on first `import`.
        pub fn module() -> crate::value::Value {
            use std::cell::RefCell;
            use std::collections::HashMap;
            use std::rc::Rc;
            let mut attrs: HashMap<String, crate::value::Value> = HashMap::new();
            #(#const_entries)*
            #(#attr_entries)*
            crate::value::Value::py_module(Rc::new(RefCell::new(crate::value::PyModule {
                name: #module_name_lit.to_string(),
                attrs,
            })))
        }
    };

    expanded.into()
}
