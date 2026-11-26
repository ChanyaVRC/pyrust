//! Overload-set dispatcher and vectorcall fast-entry codegen.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, Ident, LitStr};

use crate::builtin_fn::{
    NativeStateCtx, RegEntry, emit_builtin_reg, emit_name_static, emit_native_state_helpers,
};
use crate::clinic::{KW_ONLY_POS_SENTINEL, emit_msg_name_binding, emit_validation_phase};
use crate::signature::{ModuleFn, ModuleFnArgs, TypedParam, params_all_positional_only};

/// Emit a multi-fn overload group's artefacts: one shared name constant,
/// N typed-arg body fns, one dispatcher (plus optional vectorcall fast
/// dispatcher), and one `BuiltinReg { … }` element for the `REGS` slice.
pub(crate) fn emit_overload_set_artefacts(
    group: &[&ModuleFn],
    name_suffix: &str,
    rust_suffix: &str,
    is_class_method: bool,
    extra_cfg_attrs: &[Attribute],
    native_state_ctx: Option<NativeStateCtx<'_>>,
) -> (Vec<TokenStream>, RegEntry, Ident) {
    let head = group[0];
    let head_span = head.short_name.span();
    let suffix_lit = LitStr::new(name_suffix, head_span);
    let bare_lit = LitStr::new(&head.py_and_rust_short_name().0, head_span);
    let name_static_ident = format_ident!("__pyfn_{}_NAME", rust_suffix, span = head_span);
    let dispatcher_ident = format_ident!("__pyfn_{}", rust_suffix, span = head_span);

    let mut all_cfg_attrs: Vec<Attribute> =
        Vec::with_capacity(extra_cfg_attrs.len() + head.cfg_attrs.len());
    all_cfg_attrs.extend(extra_cfg_attrs.iter().cloned());
    all_cfg_attrs.extend(head.cfg_attrs.iter().cloned());

    let ModuleFnArgs::Typed { params: ref_params } = &head.args else {
        unreachable!("validate_overload_set already rejected legacy overloads");
    };

    // Overload sets disallow defaults and varargs, so min_pos == max_pos.
    let pos_count = ref_params.iter().filter(|p| !p.keyword_only).count();
    let all_positional_only = params_all_positional_only(ref_params);

    let per_arg_locate = emit_per_arg_locate(ref_params, all_positional_only);
    let (body_fns, dispatch_branches) = emit_overload_bodies_and_branches(
        group,
        rust_suffix,
        &name_static_ident,
        &all_cfg_attrs,
        native_state_ctx,
    );

    let no_match_tail = emit_no_match_tail(ref_params);
    let msg_name_binding = emit_msg_name_binding(head.bare_name, &bare_lit);
    let phase1 = emit_validation_phase(
        ref_params,
        head.arity_style,
        all_positional_only,
        pos_count,
        pos_count,
    );

    let dispatcher = quote! {
        #(#all_cfg_attrs)*
        #[allow(non_snake_case)]
        fn #dispatcher_ident(
            _interp: &mut crate::Interpreter,
            __pyrust_args: &[ExpandedCallArg],
        ) -> crate::error::Result<crate::value::Value> {
            #[allow(non_snake_case)]
            let FN_NAME: &'static str = #name_static_ident;
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
                #(#all_cfg_attrs)*
                #[allow(non_snake_case)]
                fn #fast_ident(
                    _interp: &mut crate::Interpreter,
                    __pyrust_fast_args: &[Value],
                ) -> crate::error::Result<crate::value::Value> {
                    #[allow(non_snake_case)]
                    let FN_NAME: &'static str = #name_static_ident;
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
    items.push(emit_name_static(
        &name_static_ident,
        &suffix_lit,
        &all_cfg_attrs,
    ));
    items.extend(body_fns);
    items.push(dispatcher);
    items.push(fast_fn_item);

    let reg_entry = emit_builtin_reg(
        &name_static_ident,
        &dispatcher_ident,
        name_suffix,
        &suffix_lit,
        is_class_method || head.method_descriptor,
        fast_reg_fields,
        &all_cfg_attrs,
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
    rust_suffix: &str,
    name_static_ident: &Ident,
    all_cfg_attrs: &[Attribute],
    native_state_ctx: Option<NativeStateCtx<'_>>,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let mut body_fns = Vec::with_capacity(group.len());
    let mut dispatch_branches = Vec::with_capacity(group.len());
    let state_helpers = emit_native_state_helpers(native_state_ctx);

    for (idx, f) in group.iter().enumerate() {
        let ModuleFnArgs::Typed { params } = &f.args else {
            unreachable!("validate_overload_set already rejected legacy overloads");
        };
        let body_ident = format_ident!(
            "__pyfn_{}_overload_{}",
            rust_suffix,
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
            #(#all_cfg_attrs)*
            #[allow(non_snake_case)]
            #(#attrs)*
            fn #body_ident(
                _interp: &mut crate::Interpreter,
                #(#body_typed_params),*
            ) -> crate::error::Result<crate::value::Value> {
                #[allow(non_snake_case)]
                let FN_NAME: &'static str = #name_static_ident;
                let _ = FN_NAME;
                #state_helpers
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
                    let #n: #ty = <#ty as crate::interpreter::builtin_args::FromValue>::try_from_value_with_interp(
                        _interp, #arg_ident, __pyrust_msg_name, #name_lit,
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
