//! Function signature parsing, parameter classification, and overload validation.

use std::collections::{HashMap, HashSet};

use quote::ToTokens;
use syn::parse::ParseStream;
use syn::{Attribute, Block, Expr, Ident, LitStr, Token, Type};

use crate::attrs::{
    ArityStyle, FnAttrs, is_snake_case, parse_fn_attrs, parse_param_attrs, unraw_ident,
};

/// A single `fn` declaration inside `pyrust_module!` (top-level or class method).
pub(crate) struct ModuleFn {
    /// Outer attributes that remain on the generated fn (`#[doc = …]`, etc.).
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) short_name: Ident,
    /// Optional Python-level name override set via `#[py_name = "..."]`.
    pub(crate) py_name_override: Option<LitStr>,
    /// The argument-list dialect this fn uses.
    pub(crate) args: ModuleFnArgs,
    /// Arity-error wording style, set via `#[arity_style(...)]`.
    pub(crate) arity_style: ArityStyle,
    /// `true` when `#[bare_name]` is present — generated dialect messages
    /// use the unqualified short `__name__` instead of `FN_PREFIX + short`.
    pub(crate) bare_name: bool,
    /// `true` when `#[method_descriptor]` is present — explicitly marks an
    /// undotted top-level `fn` as `BuiltinCallableKind::MethodDescriptor`.
    pub(crate) method_descriptor: bool,
    pub(crate) body: Block,
}

impl ModuleFn {
    /// Compute the `(py_short, rust_short)` pair for this function:
    /// `rust_short` strips any leading `r#`; `py_short` applies `#[py_name]`
    /// if present and otherwise equals `rust_short`.
    pub(crate) fn py_and_rust_short_name(&self) -> (String, String) {
        let rust_short = unraw_ident(&self.short_name);
        let py_short = match &self.py_name_override {
            Some(lit) => lit.value(),
            None => rust_short.clone(),
        };
        (py_short, rust_short)
    }
}

/// Argument-list dialect — legacy `(args: &[ExpandedCallArg])` or typed.
pub(crate) enum ModuleFnArgs {
    Legacy { args_ident: Ident },
    Typed { params: Vec<TypedParam> },
}

/// One parameter in a typed signature.
pub(crate) struct TypedParam {
    /// Parameter name (the local bound in the generated body).
    pub(crate) name: Ident,
    /// Wrapper type — e.g. `PyInt`, `PyFloat`, `Option<PyStr>`.
    pub(crate) ty: Type,
    /// Lazy `#[default(expr)]` evaluated only when the slot is missing.
    pub(crate) default: Option<Expr>,
    /// `#[positional_only]` — passing by keyword is rejected.
    pub(crate) positional_only: bool,
    /// `#[keyword_only]` — passing positionally is rejected.
    pub(crate) keyword_only: bool,
}

impl TypedParam {
    /// Python-level parameter name literal (with any `r#` raw prefix stripped).
    pub(crate) fn name_lit(&self) -> LitStr {
        LitStr::new(&unraw_ident(&self.name), self.name.span())
    }
}

/// Parse one `fn name(args) [-> Type] { body }` declaration whose outer
/// attributes have already been harvested by the caller.
pub(crate) fn parse_module_fn(
    input: ParseStream,
    raw_attrs: Vec<Attribute>,
) -> syn::Result<ModuleFn> {
    let FnAttrs {
        retained: attrs,
        py_name_override,
        arity_style,
        arity_style_set,
        bare_name,
        method_descriptor,
    } = parse_fn_attrs(raw_attrs)?;

    let _fn_kw: Token![fn] = input.parse()?;
    let short_name: Ident = input.parse()?;
    let short_str = unraw_ident(&short_name);
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
    let args = parse_fn_args(&arg_content)?;

    // Optional return-type annotation (ignored — always Result<Value>).
    if input.peek(Token![->]) {
        let _: Token![->] = input.parse()?;
        let _: Type = input.parse()?;
    }
    let body: Block = input.parse()?;

    validate_fn_attr_compatibility(&short_name, &args, arity_style, arity_style_set, bare_name)?;

    Ok(ModuleFn {
        attrs,
        short_name,
        py_name_override,
        args,
        arity_style,
        bare_name,
        method_descriptor,
        body,
    })
}

/// Cross-validate function-level dialect attributes against the parsed
/// parameter list (#2331).
fn validate_fn_attr_compatibility(
    short_name: &Ident,
    args: &ModuleFnArgs,
    arity_style: ArityStyle,
    arity_style_set: bool,
    bare_name: bool,
) -> syn::Result<()> {
    match args {
        ModuleFnArgs::Legacy { .. } => {
            if arity_style_set {
                return Err(syn::Error::new(
                    short_name.span(),
                    "`#[arity_style(...)]` requires the typed-signature dialect; \
                     the legacy `(args)` form must build its own arity error",
                ));
            }
            if bare_name {
                return Err(syn::Error::new(
                    short_name.span(),
                    "`#[bare_name]` requires the typed-signature dialect; \
                     the legacy `(args)` form must build its own messages",
                ));
            }
        }
        ModuleFnArgs::Typed { params } => {
            if arity_style == ArityStyle::TakesExactlyOne {
                let pos = params.iter().filter(|p| !p.keyword_only).count();
                if pos != 1 || params.iter().any(|p| p.default.is_some()) {
                    return Err(syn::Error::new(
                        short_name.span(),
                        "`#[arity_style(takes_exactly_one)]` requires exactly one \
                         positional parameter with no default",
                    ));
                }
            }
            if arity_style == ArityStyle::TakesNoArgs && !params.is_empty() {
                return Err(syn::Error::new(
                    short_name.span(),
                    "`#[arity_style(takes_no_args)]` requires zero parameters",
                ));
            }
        }
    }
    Ok(())
}

/// Parse the contents of the fn parameter list into either the legacy
/// `(args)` slice dialect or the typed `(x: PyInt, ...)` dialect.
fn parse_fn_args(input: ParseStream) -> syn::Result<ModuleFnArgs> {
    let mut params: Vec<TypedParam> = Vec::new();
    while !input.is_empty() {
        params.push(parse_one_param(input)?);
        if input.peek(Token![,]) {
            let _: Token![,] = input.parse()?;
        } else if !input.is_empty() {
            return Err(input.error("expected `,` or `)` after parameter"));
        }
    }

    // Legacy detection — a single bare ident with no attrs / default /
    // explicit type (or the legacy slice type) -> legacy slice binding.
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

    // Typed dialect structural invariants:
    // 1. Every parameter has an explicit type annotation.
    // 2. Parameter names are unique.
    let mut seen: HashSet<String> = HashSet::with_capacity(params.len());
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
fn parse_one_param(input: ParseStream) -> syn::Result<TypedParam> {
    let raw_attrs = input.call(Attribute::parse_outer)?;
    let param_attrs = parse_param_attrs(&raw_attrs)?;
    let name: Ident = input.parse()?;

    let ty: Type = if input.peek(Token![:]) {
        let _: Token![:] = input.parse()?;
        input.parse()?
    } else {
        // Placeholder unit type recognised by `parse_fn_args` as "no type given".
        syn::parse_quote!(())
    };

    Ok(TypedParam {
        name,
        ty,
        default: param_attrs.default,
        positional_only: param_attrs.positional_only,
        keyword_only: param_attrs.keyword_only,
    })
}

/// Returns true when every parameter in a non-empty list is `#[positional_only]`.
pub(crate) fn params_all_positional_only(params: &[TypedParam]) -> bool {
    !params.is_empty() && params.iter().all(|p| p.positional_only && !p.keyword_only)
}

/// Names of every parameter that may be passed by keyword.
pub(crate) fn allowed_kwarg_lits(params: &[TypedParam]) -> Vec<LitStr> {
    params
        .iter()
        .filter(|p| !p.positional_only)
        .map(TypedParam::name_lit)
        .collect()
}

/// Compute `(min_positional, max_positional)` counts for a typed parameter list.
pub(crate) fn positional_arity_bounds(params: &[TypedParam]) -> (usize, usize) {
    let min_pos = params
        .iter()
        .filter(|p| !p.keyword_only && p.default.is_none())
        .count();
    let max_pos = params.iter().filter(|p| !p.keyword_only).count();
    (min_pos, max_pos)
}

/// Render a `syn::Type` to its canonical token string with all whitespace
/// removed.  The `()` placeholder returns `""` so un-annotated params are
/// detected cleanly.
pub(crate) fn ty_to_canonical_string(ty: &Type) -> String {
    if let Type::Tuple(t) = ty
        && t.elems.is_empty()
    {
        return String::new();
    }
    ty.to_token_stream()
        .to_string()
        .split_whitespace()
        .collect()
}

/// Group module-level `fn` declarations by Python-level short name in
/// deterministic source order and validate any multi-fn overload sets.
pub(crate) fn group_funcs_by_name(funcs: &[ModuleFn]) -> syn::Result<Vec<Vec<&ModuleFn>>> {
    let mut groups: Vec<Vec<&ModuleFn>> = Vec::new();
    let mut name_to_idx: HashMap<String, usize> = HashMap::new();
    for f in funcs {
        let (py_short, _) = f.py_and_rust_short_name();
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

/// Enforce structural compatibility across overloads of a single Python-level
/// name:
/// 1. All overloads use the typed dialect.
/// 2. Same arity, parameter names, and `#[positional_only]`/`#[keyword_only]` flags.
/// 3. No `#[default(...)]` on any parameter.
/// 4. Any unconditional `PyValue` / `Option<PyValue>` catch-all overload is last.
/// 5. All overloads agree on `#[arity_style]`, `#[bare_name]`, and `#[method_descriptor]`.
fn validate_overload_set(group: &[&ModuleFn]) -> syn::Result<()> {
    let reference = group[0];
    let ref_params = require_typed_overload_params(reference)?;
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
        let params = require_typed_overload_params(f)?;
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

    // Catch-all-must-be-last: an overload whose every param matches
    // unconditionally would shadow every overload declared after it.
    for (idx, f) in group.iter().enumerate() {
        if idx == group.len() - 1 {
            break;
        }
        let ModuleFnArgs::Typed { params } = &f.args else {
            unreachable!("checked above");
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

    // All overloads share one dispatcher and one registry entry, so they must
    // agree on arity wording, message name, and callable kind (#2331).
    for (idx, f) in group.iter().enumerate().skip(1) {
        if f.arity_style != reference.arity_style {
            return Err(syn::Error::new(
                f.short_name.span(),
                format!(
                    "overload #{idx} disagrees on `#[arity_style]` with the \
                     first overload — all overloads share one dispatcher and \
                     must agree on the arity-error wording",
                ),
            ));
        }
        if f.bare_name != reference.bare_name {
            return Err(syn::Error::new(
                f.short_name.span(),
                format!(
                    "overload #{idx} disagrees on `#[bare_name]` with the \
                     first overload — all overloads share one dispatcher and \
                     must agree on the message name",
                ),
            ));
        }
        if f.method_descriptor != reference.method_descriptor {
            return Err(syn::Error::new(
                f.short_name.span(),
                format!(
                    "overload #{idx} disagrees on `#[method_descriptor]` with the \
                     first overload — all overloads share one registry entry and \
                     must agree on callable kind",
                ),
            ));
        }
    }

    Ok(())
}

fn require_typed_overload_params(f: &ModuleFn) -> syn::Result<&[TypedParam]> {
    match &f.args {
        ModuleFnArgs::Typed { params } => Ok(params),
        ModuleFnArgs::Legacy { args_ident } => Err(syn::Error::new(
            args_ident.span(),
            "overload sets must use the typed dialect — the legacy `(args)` \
             form cannot participate in type-based dispatch",
        )),
    }
}

/// Returns true if every parameter in `params` is an unconditional matcher
/// (`PyValue`, `PyValueRef`, `OptValueRef`, or `Option`/`OptionalArg` thereof).
fn is_catch_all_overload(params: &[TypedParam]) -> bool {
    !params.is_empty() && params.iter().all(|p| is_unconditional_matcher_ty(&p.ty))
}

fn is_unconditional_matcher_ty(ty: &Type) -> bool {
    let s = ty_to_canonical_string(ty);
    let inner = s
        .strip_prefix("Option<")
        .or_else(|| s.strip_prefix("OptionalArg<"))
        .and_then(|rest| rest.strip_suffix('>'));

    if let Some(inner) = inner {
        return matches_bare_or_qualified(inner, &["PyValue", "PyValueRef"]);
    }
    matches_bare_or_qualified(&s, &["PyValue", "PyValueRef", "OptValueRef"])
}

fn matches_bare_or_qualified(ty_str: &str, names: &[&str]) -> bool {
    names.iter().any(|name| {
        ty_str == *name
            || ty_str
                .strip_suffix(name)
                .is_some_and(|prefix| prefix.ends_with("::"))
    })
}
