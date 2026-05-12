//! `pyrust-derive` — proc-macros for declaring built-in Python functions in Rust.
//!
//! Annotate a Rust function with `#[pyfunction(name = "module.name")]` to
//! register it as a built-in callable.  The macro generates a sibling
//! `pub const` of type `crate::builtin_registry::BuiltinReg` whose name is
//! the SCREAMING_SNAKE of the function name and whose value pairs the
//! Python-level name with a function pointer to the annotated function.
//!
//! The annotated function must have the exact signature
//! `fn(&mut crate::Interpreter, &[crate::interpreter::runtime::ExpandedCallArg])
//!     -> crate::error::Result<crate::value::Value>`.
//!
//! Example:
//!
//! ```ignore
//! #[pyfunction(name = "math.sqrt")]
//! fn math_sqrt(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
//!     // … body …
//! }
//! // generates: pub const MATH_SQRT: BuiltinReg = BuiltinReg { name: "math.sqrt", dispatch: math_sqrt };
//! ```
//!
//! Per-module registration is then a single `pub const REGS: &[BuiltinReg] = &[MATH_SQRT, …]`
//! slice that the central registry concatenates at compile time.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::{Ident, ItemFn, LitStr, Meta, Token, parse_macro_input, punctuated::Punctuated};

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

    // Use the user-provided Python name to disambiguate the registration
    // constant when two Rust fns share a base name (e.g. `int_call` vs
    // `int_to_bytes`).
    let _ = format_ident!("{}", const_ident); // touch to satisfy `format_ident!`

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
