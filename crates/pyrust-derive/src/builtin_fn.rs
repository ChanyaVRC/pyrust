//! Single-function builtin codegen (`#[pyfunction]` and `pyrust_module!` fns)
//! plus shared callable metadata / registry / attribute-binding helpers.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{Expr, ExprLit, Ident, ItemFn, Lit, LitStr, Meta, Token, parse_macro_input};

use crate::attrs::unraw_ident;
use crate::clinic::{emit_typed_fast_prelude, emit_typed_prelude};
use crate::signature::{ModuleFn, ModuleFnArgs};

/// Emit the immutable class-identity tag for a primitive/object descriptor
/// owner.  The owner is the segment immediately before the callable name;
/// this also handles legacy suffixes such as
/// `builtins.TypeAliasType.__repr__` without treating the registration-module
/// prefix as the owner.
pub(crate) fn canonical_descriptor_owner(qualname: &str, span: Span) -> Option<TokenStream2> {
    let owner_qualname = qualname.rsplit_once('.')?.0;
    let owner = owner_qualname.rsplit('.').next()?;
    let variant = match owner {
        "object" => "Object",
        "bool" => "Bool",
        "bytearray" => "Bytearray",
        "bytes" => "Bytes",
        "complex" => "Complex",
        "dict" => "Dict",
        "ellipsis" => "Ellipsis",
        "float" => "Float",
        "frozenset" => "Frozenset",
        "int" => "Int",
        "list" => "List",
        "mappingproxy" => "MappingProxy",
        "NoneType" => "NoneType",
        "NotImplementedType" => "NotImplementedType",
        "set" => "Set",
        "str" => "Str",
        "tuple" => "Tuple",
        _ => return None,
    };
    let variant = Ident::new(variant, span);
    Some(quote! { pyrust_core::CanonicalClassTag::#variant })
}

/// Emit the module-scope `LazyLock<&'static str>` holding the leaked
/// `FN_PREFIX + suffix` callable name.
pub(crate) fn emit_name_static(name_static_ident: &Ident, suffix_lit: &LitStr) -> TokenStream2 {
    quote! {
        #[allow(non_upper_case_globals)]
        static #name_static_ident: std::sync::LazyLock<&'static str> =
            std::sync::LazyLock::new(|| {
                ::std::boxed::Box::leak(
                    format!("{}{}", FN_PREFIX, #suffix_lit).into_boxed_str(),
                )
            });
    }
}

/// Emit `BuiltinCallableMetadata::*` for a module function or method descriptor.
pub(crate) fn emit_callable_metadata(
    name_suffix: &str,
    suffix_lit: &LitStr,
    is_method_descriptor: bool,
) -> TokenStream2 {
    if is_method_descriptor || name_suffix.contains('.') {
        if let Some(owner) = canonical_descriptor_owner(name_suffix, suffix_lit.span()) {
            quote! {
                crate::builtin_registry::BuiltinCallableMetadata::canonical_method_descriptor(
                    MODULE_NAME,
                    #suffix_lit,
                    #owner,
                )
            }
        } else {
            quote! {
                crate::builtin_registry::BuiltinCallableMetadata::method_descriptor(
                    MODULE_NAME,
                    #suffix_lit,
                )
            }
        }
    } else {
        quote! {
            crate::builtin_registry::BuiltinCallableMetadata::module_function(
                MODULE_NAME,
                #suffix_lit,
            )
        }
    }
}

/// Emit a `BuiltinReg` struct literal for either a single-body builtin or an
/// overload dispatcher.
pub(crate) fn emit_builtin_reg(
    name_static_ident: &Ident,
    dispatch_ident: &Ident,
    name_suffix: &str,
    suffix_lit: &LitStr,
    is_method_descriptor: bool,
    fast_reg_fields: TokenStream2,
) -> TokenStream2 {
    let metadata = emit_callable_metadata(name_suffix, suffix_lit, is_method_descriptor);
    quote! {
        crate::builtin_registry::BuiltinReg {
            name: *#name_static_ident,
            dispatch: #dispatch_ident,
            metadata: #metadata,
            #fast_reg_fields
        }
    }
}

/// Emit `attrs.insert(<short>, Value::[fresh_]builtin_function(*<name_static>))`,
/// shared by top-level module functions and class methods.
pub(crate) fn emit_attr_insert(short_lit: &LitStr, name_static_ident: &Ident) -> TokenStream2 {
    quote! {
        attrs.insert(
            #short_lit.to_string(),
            if FN_PREFIX.is_empty() {
                crate::value::Value::builtin_function(*#name_static_ident)
            } else {
                crate::value::Value::fresh_builtin_function(*#name_static_ident)
            },
        );
    }
}

/// Expansion of `#[pyfunction(name = "module.fn")]`.
pub(crate) fn expand_pyfunction(attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);
    let metas = parse_macro_input!(attr with Punctuated::<Meta, Token![,]>::parse_terminated);

    let mut name: Option<LitStr> = None;
    for meta in metas {
        if let Meta::NameValue(nv) = &meta
            && nv.path.is_ident("name")
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
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
    let py_name_value = py_name.value();
    let (py_module, py_short_name) = py_name_value
        .rsplit_once('.')
        .unwrap_or(("builtins", py_name_value.as_str()));
    let py_module = LitStr::new(py_module, py_name.span());
    let py_short_name = LitStr::new(py_short_name, py_name.span());

    let fn_ident = &func.sig.ident;
    // Strip a leading `r#` from raw idents (`r#type` -> `type`) before
    // upper-casing so the generated const ident never contains `#`.
    let const_ident = Ident::new(&unraw_ident(fn_ident).to_uppercase(), fn_ident.span());

    quote! {
        #func

        #[allow(non_upper_case_globals)]
        pub const #const_ident: crate::builtin_registry::BuiltinReg =
            crate::builtin_registry::BuiltinReg {
                name: #py_name,
                dispatch: #fn_ident,
                metadata: crate::builtin_registry::BuiltinCallableMetadata::module_function(
                    #py_module,
                    #py_short_name,
                ),
                fast: None,
                min_arity: 0,
                max_arity: 0,
            };
    }
    .into()
}

/// Emit the per-fn artefacts for one single-body function or class method:
/// - a module-scope `LazyLock<&'static str>` holding the leaked Python name
/// - a dispatch `fn` named `__pyfn_<rust_ident_suffix>`
/// - an optional vectorcall fast entry `__pyfn_<rust_ident_suffix>_fast`
/// - a `BuiltinReg` entry for the registry
///
/// Returns `(fn_item, reg_entry, name_static_ident)`.
pub(crate) fn emit_fn_artefacts(
    f: &ModuleFn,
    name_suffix: &str,
    rust_ident_suffix: &str,
    is_class_method: bool,
) -> (TokenStream2, TokenStream2, Ident) {
    let short = &f.short_name;
    let rust_fn_ident = format_ident!("__pyfn_{}", rust_ident_suffix, span = short.span());
    let name_static_ident = format_ident!("__pyfn_{}_NAME", rust_ident_suffix, span = short.span());
    let attrs = &f.attrs;
    let body_stmts = &f.body.stmts;
    let suffix_lit = LitStr::new(name_suffix, short.span());
    let bare_lit = LitStr::new(&f.py_and_rust_short_name().0, short.span());

    let (args_binding, typed_prelude) = match &f.args {
        ModuleFnArgs::Legacy { args_ident } => (quote!(#args_ident), quote!()),
        ModuleFnArgs::Typed { params } => (
            quote!(__pyrust_args),
            emit_typed_prelude(params, f.arity_style, f.bare_name, &bare_lit),
        ),
    };

    // Optional "vectorcall" fast entry: a typed signature with no keyword-only
    // params can be filled straight from a positional `&[Value]`.
    let (fast_fn_item, fast_reg_fields) = match &f.args {
        ModuleFnArgs::Typed { params } if params.iter().all(|p| !p.keyword_only) => {
            let fast_ident =
                format_ident!("__pyfn_{}_fast", rust_ident_suffix, span = short.span());
            let min_arity = params.iter().filter(|p| p.default.is_none()).count() as u8;
            let max_arity = params.len() as u8;
            let fast_prelude = emit_typed_fast_prelude(params, f.bare_name, &bare_lit);
            let item = quote! {
                #[allow(non_snake_case)]
                #(#attrs)*
                fn #fast_ident(
                    _interp: &mut crate::Interpreter,
                    __pyrust_fast_args: &[Value],
                ) -> Result<Value> {
                    #[allow(non_snake_case)]
                    let FN_NAME: &'static str = *#name_static_ident;
                    let _ = FN_NAME;
                    #fast_prelude
                    #(#body_stmts)*
                }
            };
            let fields = quote! {
                fast: Some(#fast_ident),
                min_arity: #min_arity,
                max_arity: #max_arity,
            };
            (item, fields)
        }
        _ => (quote!(), quote! { fast: None, min_arity: 0, max_arity: 0, }),
    };

    let name_static = emit_name_static(&name_static_ident, &suffix_lit);
    let fn_item = quote! {
        #name_static

        // Class-method dispatch idents look like `__pyfn_Counter____init__`,
        // which trips `non_snake_case`; these are internal macro-generated names.
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

        #fast_fn_item
    };

    let reg_entry = emit_builtin_reg(
        &name_static_ident,
        &rust_fn_ident,
        name_suffix,
        &suffix_lit,
        is_class_method || f.method_descriptor,
        fast_reg_fields,
    );

    (fn_item, reg_entry, name_static_ident)
}
