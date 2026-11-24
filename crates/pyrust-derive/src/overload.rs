//! Overload-set dispatcher and vectorcall fast-entry codegen.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, LitStr};

use crate::builtin_fn::{emit_builtin_reg, emit_name_static};
use crate::clinic::{KW_ONLY_POS_SENTINEL, emit_msg_name_binding, emit_validation_phase};
use crate::signature::{ModuleFn, ModuleFnArgs, TypedParam, params_all_positional_only};

/// Emit a multi-fn overload group's artefacts: one shared name static,
/// N typed-arg body fns, one dispatcher (plus optional vectorcall fast
/// dispatcher), and one `BuiltinReg` entry.
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
pub(crate) fn emit_overload_set_artefacts(
    group: &[&ModuleFn],
    py_short: &str,
    rust_short: &str,
) -> (Vec<TokenStream>, TokenStream, Ident) {
    let head = group[0];
    let head_span = head.short_name.span();
    let suffix_lit = LitStr::new(py_short, head_span);
    let name_static_ident = format_ident!("__pyfn_{}_NAME", rust_short, span = head_span);
    let dispatcher_ident = format_ident!("__pyfn_{}", rust_short, span = head_span);

    let ModuleFnArgs::Typed { params: ref_params } = &head.args else {
        unreachable!("validate_overload_set already rejected legacy overloads");
    };

    // Overload sets disallow defaults, so min_pos == max_pos.
    let pos_count = ref_params.iter().filter(|p| !p.keyword_only).count();
    let all_positional_only = params_all_positional_only(ref_params);

    let per_arg_locate = emit_per_arg_locate(ref_params, all_positional_only);
    let (body_fns, dispatch_branches) =
        emit_overload_bodies_and_branches(group, rust_short, &name_static_ident);

    let no_match_tail = emit_no_match_tail(ref_params);
    let bare_lit = LitStr::new(py_short, head_span);
    let msg_name_binding = emit_msg_name_binding(head.bare_name, &bare_lit);
    let phase1 = emit_validation_phase(
        ref_params,
        head.arity_style,
        all_positional_only,
        pos_count,
        pos_count,
    );

    let dispatcher = quote! {
        #[allow(non_snake_case)]
        fn #dispatcher_ident(
            _interp: &mut crate::Interpreter,
            __pyrust_args: &[ExpandedCallArg],
        ) -> crate::error::Result<crate::value::Value> {
            #[allow(non_snake_case)]
            let FN_NAME: &'static str = *#name_static_ident;
            let _ = FN_NAME;
            #msg_name_binding

            // Phase 1 — shared validation.
            #phase1
            #(#per_arg_locate)*

            // Phase 2 — try each overload in declaration order.
            #(#dispatch_branches)*

            #no_match_tail
        }
    };

    // Optional vectorcall fast entry when no parameter is keyword-only.
    let (fast_fn_item, fast_reg_fields) =
        if !ref_params.is_empty() && ref_params.iter().all(|p| !p.keyword_only) {
            let fast_ident = format_ident!("{}_fast", dispatcher_ident, span = head_span);
            let arity = ref_params.len() as u8;
            let fast_locate: Vec<TokenStream> = ref_params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let arg_ident = overload_arg_ident(p);
                    quote! {
                        let #arg_ident: &crate::value::Value = &__pyrust_fast_args[#i];
                    }
                })
                .collect();
            let item = quote! {
                #[allow(non_snake_case)]
                fn #fast_ident(
                    _interp: &mut crate::Interpreter,
                    __pyrust_fast_args: &[Value],
                ) -> crate::error::Result<crate::value::Value> {
                    #[allow(non_snake_case)]
                    let FN_NAME: &'static str = *#name_static_ident;
                    let _ = FN_NAME;
                    #msg_name_binding
                    #(#fast_locate)*
                    #(#dispatch_branches)*
                    #no_match_tail
                }
            };
            (
                item,
                quote! { fast: Some(#fast_ident), min_arity: #arity, max_arity: #arity, },
            )
        } else {
            (quote!(), quote! { fast: None, min_arity: 0, max_arity: 0, })
        };

    let mut items = Vec::with_capacity(body_fns.len() + 3);
    items.push(emit_name_static(&name_static_ident, &suffix_lit));
    items.extend(body_fns);
    items.push(dispatcher);
    items.push(fast_fn_item);

    let reg_entry = emit_builtin_reg(
        &name_static_ident,
        &dispatcher_ident,
        py_short,
        &suffix_lit,
        head.method_descriptor,
        fast_reg_fields,
    );

    (items, reg_entry, name_static_ident)
}

fn overload_arg_ident(p: &TypedParam) -> Ident {
    format_ident!("__arg_{}", p.name, span = p.name.span())
}

fn emit_per_arg_locate(ref_params: &[TypedParam], all_positional_only: bool) -> Vec<TokenStream> {
    let mut per_arg_locate = Vec::with_capacity(ref_params.len());
    let mut pos_index: usize = 0;

    for p in ref_params {
        let name_lit = p.name_lit();
        let arg_ident = overload_arg_ident(p);
        let kw_allowed = !p.positional_only;
        let this_pos = if p.keyword_only {
            KW_ONLY_POS_SENTINEL
        } else {
            let i = pos_index;
            pos_index += 1;
            i
        };
        let lookup = if all_positional_only {
            quote! {
                __pyrust_args.get(#this_pos).map(|__a| &__a.value)
            }
        } else {
            quote! {
                crate::interpreter::builtin_args::locate_arg(
                    __pyrust_args, &__positional, __pyrust_msg_name, #name_lit, #this_pos, #kw_allowed,
                )?
            }
        };
        per_arg_locate.push(quote! {
            let #arg_ident: &crate::value::Value = #lookup
                .ok_or_else(|| crate::interpreter::builtin_args::missing_arg::<()>(
                    __pyrust_msg_name, #name_lit,
                ).unwrap_err())?;
        });
    }

    per_arg_locate
}

fn emit_overload_bodies_and_branches(
    group: &[&ModuleFn],
    rust_short: &str,
    name_static_ident: &Ident,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let mut body_fns = Vec::with_capacity(group.len());
    let mut dispatch_branches = Vec::with_capacity(group.len());

    for (idx, f) in group.iter().enumerate() {
        let ModuleFnArgs::Typed { params } = &f.args else {
            unreachable!("validate_overload_set already rejected legacy overloads");
        };
        let body_ident = format_ident!(
            "__pyfn_{}_overload_{}",
            rust_short,
            idx,
            span = f.short_name.span()
        );
        let body_typed_params: Vec<TokenStream> = params
            .iter()
            .map(|p| {
                let n = &p.name;
                let t = &p.ty;
                quote!(#n: #t)
            })
            .collect();

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

        let predicate_terms: Vec<TokenStream> = params
            .iter()
            .map(|p| {
                let ty = &p.ty;
                let arg_ident = overload_arg_ident(p);
                quote!(<#ty as crate::interpreter::builtin_args::FromValue>::matches(#arg_ident))
            })
            .collect();
        let convert_args: Vec<TokenStream> = params
            .iter()
            .map(|p| {
                let n = &p.name;
                let ty = &p.ty;
                let name_lit = p.name_lit();
                let arg_ident = overload_arg_ident(p);
                quote! {
                    let #n: #ty = <#ty as crate::interpreter::builtin_args::FromValue>::try_from_value(
                        #arg_ident, __pyrust_msg_name, #name_lit,
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

    (body_fns, dispatch_branches)
}

fn emit_no_match_tail(ref_params: &[TypedParam]) -> TokenStream {
    let no_match_args: Vec<TokenStream> = ref_params
        .iter()
        .map(|p| {
            let arg_ident = overload_arg_ident(p);
            quote! { pyrust_core::builtin_type_name(#arg_ident) }
        })
        .collect();
    quote! {
        let __actuals: &[std::borrow::Cow<'static, str>] = &[#(#no_match_args),*];
        crate::interpreter::builtin_args::no_overload_matched::<crate::value::Value>(
            __pyrust_msg_name, __actuals,
        )
    }
}
