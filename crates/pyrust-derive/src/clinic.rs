//! Typed-signature argument unpacking, arity checks, and Argument Clinic codegen.

use proc_macro2::TokenStream;
use quote::quote;
use syn::LitStr;

use crate::attrs::ArityStyle;
use crate::signature::{
    TypedParam, allowed_kwarg_lits, params_all_positional_only, positional_arity_bounds,
};

/// Sentinel positional index for `#[keyword_only]` parameters so `locate_arg`
/// never matches them against the positional slice.
pub(crate) const KW_ONLY_POS_SENTINEL: usize = usize::MAX;

/// Emit the `let __pyrust_msg_name: &str = …;` binding interpolated by every
/// dialect-generated error message (#2331).
pub(crate) fn emit_msg_name_binding(bare_name: bool, bare_lit: &LitStr) -> TokenStream {
    if bare_name {
        quote! { let __pyrust_msg_name: &str = #bare_lit; }
    } else {
        quote! { let __pyrust_msg_name: &str = FN_NAME; }
    }
}

/// Emit the positional-count guard call for `arity_style` over
/// `__pyrust_msg_name` (#2331).
pub(crate) fn emit_arity_check(
    arity_style: ArityStyle,
    count_expr: TokenStream,
    min_pos: usize,
    max_pos: usize,
) -> TokenStream {
    match arity_style {
        ArityStyle::Standard | ArityStyle::Clinic => quote! {
            crate::interpreter::builtin_args::check_positional_count(
                __pyrust_msg_name,
                #count_expr,
                #min_pos,
                #max_pos,
            )?;
        },
        ArityStyle::TakesExactlyOne => quote! {
            crate::interpreter::builtin_args::check_exactly_one_argument(
                __pyrust_msg_name,
                #count_expr,
            )?;
        },
        ArityStyle::TakesNoArgs => quote! {
            crate::interpreter::builtin_args::check_no_arguments(
                __pyrust_msg_name,
                #count_expr,
            )?;
        },
        ArityStyle::ExpectedGot => quote! {
            crate::interpreter::builtin_args::check_arity_expected_got(
                __pyrust_msg_name,
                #count_expr,
                #min_pos,
                #max_pos,
            )?;
        },
        ArityStyle::ExpectedGotParens => quote! {
            crate::interpreter::builtin_args::check_arity_expected_got_parens(
                __pyrust_msg_name,
                #count_expr,
                #min_pos,
                #max_pos,
            )?;
        },
    }
}

/// Emit Phase 1 validation (keyword rejection / collection + arity guard),
/// shared between single-body typed preludes and overload dispatchers.
pub(crate) fn emit_validation_phase(
    params: &[TypedParam],
    arity_style: ArityStyle,
    all_positional_only: bool,
    min_pos: usize,
    max_pos: usize,
) -> TokenStream {
    if all_positional_only {
        let arity_check =
            emit_arity_check(arity_style, quote!(__pyrust_args.len()), min_pos, max_pos);
        quote! {
            crate::interpreter::builtin_args::reject_named_args(__pyrust_args, __pyrust_msg_name)?;
            #arity_check
        }
    } else {
        let allowed_kwargs = allowed_kwarg_lits(params);
        let arity_check =
            emit_arity_check(arity_style, quote!(__positional.len()), min_pos, max_pos);
        quote! {
            let __positional: crate::interpreter::builtin_args::PositionalArgs<'_> =
                crate::interpreter::builtin_args::validate_kwargs_and_collect_positional(
                    __pyrust_args,
                    __pyrust_msg_name,
                    &[#(#allowed_kwargs),*],
                )?;
            #arity_check
        }
    }
}

/// Emit one typed local binding `let <name>: <ty> = { ... };`, deduplicating
/// the `FromValue::try_from_value` + lazy default / `missing_arg` boilerplate
/// across Clinic, general, positional-only, and vectorcall fast preludes.
fn emit_param_binding(
    param: &TypedParam,
    lookup_stmt: Option<TokenStream>,
    value_expr: TokenStream,
) -> TokenStream {
    let name_ident = &param.name;
    let name_lit = param.name_lit();
    let ty = &param.ty;
    let default_branch = match &param.default {
        Some(expr) => quote! { Ok::<#ty, crate::error::PyError>(#expr) },
        None => {
            quote! { crate::interpreter::builtin_args::missing_arg::<#ty>(__pyrust_msg_name, #name_lit) }
        }
    };
    quote! {
        let #name_ident: #ty = {
            #lookup_stmt
            match #value_expr {
                Some(__v) => <#ty as crate::interpreter::builtin_args::FromValue>::try_from_value(
                    __v, __pyrust_msg_name, #name_lit,
                )?,
                None => (#default_branch)?,
            }
        };
    }
}

/// Emit the Argument Clinic (`#[arity_style(clinic)]`) typed prelude.
fn emit_typed_prelude_clinic(
    params: &[TypedParam],
    bare_name: bool,
    bare_lit: &LitStr,
) -> TokenStream {
    let param_specs: Vec<TokenStream> = params
        .iter()
        .map(|p| {
            let name_lit = p.name_lit();
            let positional_only = p.positional_only;
            let keyword_only = p.keyword_only;
            let required = p.default.is_none();
            quote! {
                crate::interpreter::builtin_args::ClinicParam {
                    name: #name_lit,
                    positional_only: #positional_only,
                    keyword_only: #keyword_only,
                    required: #required,
                }
            }
        })
        .collect();

    let per_param: Vec<TokenStream> = params
        .iter()
        .enumerate()
        .map(|(idx, p)| emit_param_binding(p, None, quote!(__clinic_slots[#idx])))
        .collect();

    let msg_name_binding = emit_msg_name_binding(bare_name, bare_lit);
    quote! {
        #[allow(unused_imports)]
        use crate::interpreter::builtin_args::FromValue as _;
        #msg_name_binding
        const __CLINIC_PARAMS: &[crate::interpreter::builtin_args::ClinicParam] = &[
            #(#param_specs),*
        ];
        let __clinic_slots = crate::interpreter::builtin_args::bind_clinic_args(
            __pyrust_args,
            __pyrust_msg_name,
            __CLINIC_PARAMS,
        )?;
        #(#per_param)*
    }
}

/// Emit the prelude that validates + binds typed parameters before the
/// user-written body runs.  See the typed-dialect docs on
/// `crate::interpreter::builtin_args` for the full contract.
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
pub(crate) fn emit_typed_prelude(
    params: &[TypedParam],
    arity_style: ArityStyle,
    bare_name: bool,
    bare_lit: &LitStr,
) -> TokenStream {
    if arity_style == ArityStyle::Clinic {
        return emit_typed_prelude_clinic(params, bare_name, bare_lit);
    }

    let (min_pos, max_pos) = positional_arity_bounds(params);
    let all_positional_only = (params.is_empty() && arity_style == ArityStyle::TakesNoArgs)
        || params_all_positional_only(params);

    let per_param: Vec<TokenStream> = if all_positional_only {
        params
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                emit_param_binding(
                    p,
                    None,
                    quote!(__pyrust_args.get(#idx).map(|__a| &__a.value)),
                )
            })
            .collect()
    } else {
        let mut pos_index: usize = 0;
        params
            .iter()
            .map(|p| {
                let name_lit = p.name_lit();
                let kw_allowed = !p.positional_only;
                let this_pos = if p.keyword_only {
                    KW_ONLY_POS_SENTINEL
                } else {
                    let i = pos_index;
                    pos_index += 1;
                    i
                };
                let lookup = quote! {
                    let __found = crate::interpreter::builtin_args::locate_arg(
                        __pyrust_args, &__positional, __pyrust_msg_name, #name_lit, #this_pos, #kw_allowed,
                    )?;
                };
                emit_param_binding(p, Some(lookup), quote!(__found))
            })
            .collect()
    };

    let msg_name_binding = emit_msg_name_binding(bare_name, bare_lit);
    let phase1 = emit_validation_phase(params, arity_style, all_positional_only, min_pos, max_pos);
    quote! {
        #[allow(unused_imports)]
        use crate::interpreter::builtin_args::FromValue as _;
        #msg_name_binding
        #phase1
        #(#per_param)*
    }
}

/// Prelude for the "vectorcall" fast entry: bind each typed parameter directly
/// from `__pyrust_fast_args: &[Value]` (positional, no names).  The caller has
/// already guaranteed `min_arity <= len <= max_arity`.
pub(crate) fn emit_typed_fast_prelude(
    params: &[TypedParam],
    bare_name: bool,
    bare_lit: &LitStr,
) -> TokenStream {
    let per_param: Vec<TokenStream> = params
        .iter()
        .enumerate()
        .map(|(idx, p)| emit_param_binding(p, None, quote!(__pyrust_fast_args.get(#idx))))
        .collect();
    let msg_name_binding = emit_msg_name_binding(bare_name, bare_lit);
    quote! {
        #[allow(unused_imports)]
        use crate::interpreter::builtin_args::FromValue as _;
        #msg_name_binding
        #(#per_param)*
    }
}
