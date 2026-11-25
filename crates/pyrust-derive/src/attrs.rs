//! Attribute parsing and identifier helpers for `pyrust-derive`.

use proc_macro2::Span;
use quote::ToTokens;
use syn::parse::ParseStream;
use syn::{Attribute, Expr, Ident, Lit, LitStr, Meta, Type};

/// Arity-error message wording for a typed-dialect builtin (#2331).
///
/// CPython's hand-written C builtins emit several distinct argument-count
/// error wordings that the default argument-clinic dialect cannot
/// reproduce.  `#[arity_style(...)]` selects one so a migrated builtin
/// stays byte-identical with CPython.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArityStyle {
    /// `NAME() takes N positional arguments but M were given` (too-many)
    /// and `NAME() missing required argument: 'x'` (too-few).  The
    /// historical default; unchanged when no `#[arity_style]` is given.
    Standard,
    /// `NAME() takes exactly one argument (M given)` for any count `!= 1`
    /// (METH_O builtins — `len`, `repr`, `ord`, `abs`, `math.sqrt`, …).
    /// Only valid on single-parameter signatures.
    TakesExactlyOne,
    /// `NAME expected N arguments, got M` (and `at least` / `at most`
    /// variants).  Note: bare name, no trailing `()`.  METH_VARARGS
    /// builtins (`isinstance`, `issubclass`, `divmod`, `hasattr`, …).
    ExpectedGot,
    /// `NAME() expected N arguments, got M` (and `at least` / `at most`
    /// variants) — same as [`ExpectedGot`] with trailing `()` (e.g. `super`).
    ExpectedGotParens,
    /// `NAME() takes no arguments (M given)` for any count `!= 0`
    /// (METH_NOARGS builtins — `globals`, `locals`).
    /// Only valid on zero-parameter signatures.
    TakesNoArgs,
    /// CPython Argument Clinic (`_PyArg_UnpackKeywords`) wording:
    /// - `NAME() takes at most N [keyword ]arguments (M given)`
    /// - `NAME() takes at least N positional arguments (M given)`
    /// - `NAME() missing required argument 'x' (pos I)`
    /// - `'k' is an invalid keyword argument for NAME()`
    /// - `argument for NAME() given by name ('k') and position (I)`
    Clinic,
}

/// Method binding kind inside a `class { … }` block.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum MethodKind {
    Regular,
    ClassMethod,
    StaticMethod,
    Getter(Option<String>),
    Setter(Option<String>),
}

/// Extracted outer attributes on a `fn` declaration inside `pyrust_module!`.
pub(crate) struct FnAttrs {
    /// Outer attributes forwarded onto the generated Rust fn (`#[doc = …]`, `#[allow(…)]`).
    /// Does NOT include `#[cfg(…)]` (stored separately in `cfg_attrs`).
    pub(crate) retained: Vec<Attribute>,
    /// `#[cfg(…)]` attributes gating the function, its name static, registry
    /// entry, and module/class attribute binding.
    pub(crate) cfg_attrs: Vec<Attribute>,
    /// Optional Python-level name override set via `#[py_name = "..."]`.
    pub(crate) py_name_override: Option<LitStr>,
    /// Selected arity-error wording (`ArityStyle::Standard` by default).
    pub(crate) arity_style: ArityStyle,
    /// Whether `#[arity_style(...)]` was explicitly provided.
    pub(crate) arity_style_set: bool,
    /// Whether `#[bare_name]` was present.
    pub(crate) bare_name: bool,
    /// Whether `#[method_descriptor]` was present.
    pub(crate) method_descriptor: bool,
    /// Class method kind (`Regular`, `ClassMethod`, `StaticMethod`, `Getter`, `Setter`).
    pub(crate) method_kind: MethodKind,
    /// Span of the `#[classmethod]`/`#[staticmethod]`/`#[getter]`/`#[setter]` attr.
    pub(crate) method_kind_span: Option<Span>,
}

/// Extracted attributes on a single typed-dialect parameter.
pub(crate) struct ParamAttrs {
    pub(crate) default: Option<Expr>,
    pub(crate) positional_only: bool,
    pub(crate) keyword_only: bool,
    pub(crate) varargs: bool,
    pub(crate) varkwargs: bool,
    pub(crate) state: bool,
    pub(crate) state_mut: bool,
}

/// Extracted outer attributes on a `class Name { … }` block.
pub(crate) struct ClassAttrs {
    pub(crate) cfg_attrs: Vec<Attribute>,
    pub(crate) native_state: Option<Type>,
}

/// Extracted outer attributes on an `exception Name(Base);` item.
pub(crate) struct ExceptionAttrs {
    pub(crate) cfg_attrs: Vec<Attribute>,
    pub(crate) doc: Option<String>,
}

/// Extracted outer attributes on a `constants { … }` entry.
pub(crate) struct ConstAttrs {
    pub(crate) cfg_attrs: Vec<Attribute>,
}

/// Strip a leading `r#` raw-identifier prefix so Python names (e.g. `r#mod` ->
/// `"mod"`, `r#type` -> `"type"`) match Python's surface names and error wording.
pub(crate) fn unraw_ident(ident: &Ident) -> String {
    let s = ident.to_string();
    s.strip_prefix("r#").map(str::to_owned).unwrap_or(s)
}

/// Canonical string key for a slice of `#[cfg(...)]` attributes so overload
/// grouping can distinguish mutually exclusive platform implementations.
pub(crate) fn cfg_key(cfg_attrs: &[Attribute]) -> String {
    cfg_attrs
        .iter()
        .map(|a| {
            a.to_token_stream()
                .to_string()
                .split_whitespace()
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Peek whether the next token in `input` is a contextual identifier equal to `kw`.
pub(crate) fn peek_contextual_kw(input: ParseStream, kw: &str) -> syn::Result<bool> {
    if !input.peek(Ident) {
        return Ok(false);
    }
    let lookahead: Ident = input.fork().parse()?;
    Ok(lookahead == kw)
}

/// Parse a path-only marker attribute (`#[bare_name]`, `#[method_descriptor]`)
/// ensuring it takes no arguments and appears at most once per fn.
fn parse_marker_attr(attr: &Attribute, flag: &mut bool, name: &str) -> syn::Result<()> {
    if !matches!(attr.meta, Meta::Path(_)) {
        return Err(syn::Error::new_spanned(
            attr,
            format!("`#[{name}]` is a marker attribute and takes no arguments"),
        ));
    }
    if *flag {
        return Err(syn::Error::new_spanned(
            attr,
            format!("`#[{name}]` may appear at most once per fn"),
        ));
    }
    *flag = true;
    Ok(())
}

/// Parse `#[getter]` or `#[getter("prop_name")]` (and similarly for `setter`).
fn parse_accessor_attr(attr: &Attribute, name: &str) -> syn::Result<Option<String>> {
    match &attr.meta {
        Meta::Path(_) => Ok(None),
        Meta::List(_) => {
            let prop_name: LitStr = attr.parse_args().map_err(|_| {
                syn::Error::new_spanned(
                    attr,
                    format!("`#[{name}(\"...\")]` takes a single string literal property name"),
                )
            })?;
            Ok(Some(prop_name.value()))
        }
        Meta::NameValue(_) => Err(syn::Error::new_spanned(
            attr,
            format!("use `#[{name}]` or `#[{name}(\"prop_name\")]`"),
        )),
    }
}

/// Parse `#[arity_style(<kind>)]`.
fn parse_arity_style(attr: &Attribute) -> syn::Result<ArityStyle> {
    let kind: Ident = attr.parse_args().map_err(|_| {
        syn::Error::new_spanned(
            attr,
            "`#[arity_style(...)]` takes a single bare style name, \
             e.g. `#[arity_style(takes_exactly_one)]`",
        )
    })?;
    match kind.to_string().as_str() {
        "standard" => Ok(ArityStyle::Standard),
        "takes_exactly_one" => Ok(ArityStyle::TakesExactlyOne),
        "takes_no_args" => Ok(ArityStyle::TakesNoArgs),
        "expected_got" => Ok(ArityStyle::ExpectedGot),
        "expected_got_parens" => Ok(ArityStyle::ExpectedGotParens),
        "clinic" => Ok(ArityStyle::Clinic),
        other => Err(syn::Error::new(
            kind.span(),
            format!(
                "unknown `#[arity_style]` value `{other}` — expected one of \
                 `standard`, `takes_exactly_one`, `takes_no_args`, \
                 `expected_got`, `expected_got_parens`, `clinic`",
            ),
        )),
    }
}

fn set_method_kind(
    attr: &Attribute,
    slot: &mut MethodKind,
    span_slot: &mut Option<Span>,
    new_kind: MethodKind,
) -> syn::Result<()> {
    if !matches!(slot, MethodKind::Regular) {
        return Err(syn::Error::new_spanned(
            attr,
            "at most one of `#[getter]`, `#[setter]`, `#[classmethod]`, or `#[staticmethod]` \
             may appear on a function",
        ));
    }
    *slot = new_kind;
    *span_slot = Some(
        attr.path()
            .segments
            .first()
            .map_or_else(Span::call_site, |s| s.ident.span()),
    );
    Ok(())
}

/// Partition outer attributes on a `pyrust_module!` fn into recognised
/// macro directives, `#[cfg(...)]` gates, and retained Rust attributes.
pub(crate) fn parse_fn_attrs(raw_attrs: Vec<Attribute>) -> syn::Result<FnAttrs> {
    let mut py_name_override: Option<LitStr> = None;
    let mut arity_style = ArityStyle::Standard;
    let mut arity_style_set = false;
    let mut bare_name = false;
    let mut method_descriptor = false;
    let mut method_kind = MethodKind::Regular;
    let mut method_kind_span: Option<Span> = None;
    let mut cfg_attrs = Vec::new();
    let mut retained = Vec::with_capacity(raw_attrs.len());

    for attr in raw_attrs {
        if attr.path().is_ident("cfg") {
            cfg_attrs.push(attr);
        } else if attr.path().is_ident("arity_style") {
            if arity_style_set {
                return Err(syn::Error::new_spanned(
                    &attr,
                    "`#[arity_style(...)]` may appear at most once per fn",
                ));
            }
            arity_style = parse_arity_style(&attr)?;
            arity_style_set = true;
        } else if attr.path().is_ident("bare_name") {
            parse_marker_attr(&attr, &mut bare_name, "bare_name")?;
        } else if attr.path().is_ident("method_descriptor") {
            parse_marker_attr(&attr, &mut method_descriptor, "method_descriptor")?;
        } else if attr.path().is_ident("classmethod") {
            if !matches!(attr.meta, Meta::Path(_)) {
                return Err(syn::Error::new_spanned(
                    &attr,
                    "`#[classmethod]` is a marker attribute and takes no arguments",
                ));
            }
            set_method_kind(
                &attr,
                &mut method_kind,
                &mut method_kind_span,
                MethodKind::ClassMethod,
            )?;
        } else if attr.path().is_ident("staticmethod") {
            if !matches!(attr.meta, Meta::Path(_)) {
                return Err(syn::Error::new_spanned(
                    &attr,
                    "`#[staticmethod]` is a marker attribute and takes no arguments",
                ));
            }
            set_method_kind(
                &attr,
                &mut method_kind,
                &mut method_kind_span,
                MethodKind::StaticMethod,
            )?;
        } else if attr.path().is_ident("getter") {
            let prop_name = parse_accessor_attr(&attr, "getter")?;
            set_method_kind(
                &attr,
                &mut method_kind,
                &mut method_kind_span,
                MethodKind::Getter(prop_name),
            )?;
        } else if attr.path().is_ident("setter") {
            let prop_name = parse_accessor_attr(&attr, "setter")?;
            set_method_kind(
                &attr,
                &mut method_kind,
                &mut method_kind_span,
                MethodKind::Setter(prop_name),
            )?;
        } else if attr.path().is_ident("py_name") {
            let nv = attr.meta.require_name_value().map_err(|_| {
                syn::Error::new_spanned(
                    &attr,
                    "`#[py_name = \"...\"]` must use the name-value form",
                )
            })?;
            if let Expr::Lit(syn::ExprLit {
                lit: Lit::Str(s), ..
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
            retained.push(attr);
        }
    }

    Ok(FnAttrs {
        retained,
        cfg_attrs,
        py_name_override,
        arity_style,
        arity_style_set,
        bare_name,
        method_descriptor,
        method_kind,
        method_kind_span,
    })
}

/// Harvest recognised parameter attributes (`#[default(expr)]`,
/// `#[positional_only]`, `#[keyword_only]`, `#[varargs]`, `#[varkwargs]`,
/// `#[state]`, `#[state_mut]`) and reject unknown ones.
pub(crate) fn parse_param_attrs(raw_attrs: &[Attribute]) -> syn::Result<ParamAttrs> {
    let mut default: Option<Expr> = None;
    let mut positional_only = false;
    let mut keyword_only = false;
    let mut varargs = false;
    let mut varkwargs = false;
    let mut state = false;
    let mut state_mut = false;

    for attr in raw_attrs {
        if attr.path().is_ident("default") {
            if default.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`#[default]` may appear at most once per parameter",
                ));
            }
            default = Some(attr.parse_args::<Expr>()?);
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
        } else if attr.path().is_ident("varargs") {
            parse_marker_attr(attr, &mut varargs, "varargs")?;
        } else if attr.path().is_ident("varkwargs") {
            parse_marker_attr(attr, &mut varkwargs, "varkwargs")?;
        } else if attr.path().is_ident("state") {
            parse_marker_attr(attr, &mut state, "state")?;
        } else if attr.path().is_ident("state_mut") {
            parse_marker_attr(attr, &mut state_mut, "state_mut")?;
        } else if attr.path().is_ident("doc")
            || attr.path().is_ident("cfg")
            || attr.path().is_ident("allow")
        {
            // Permitted but not consumed — `pyrust_module!` doesn't forward
            // per-param doc comments anywhere yet, but users may write them
            // for source-level documentation.
        } else {
            return Err(syn::Error::new_spanned(
                attr,
                "unsupported parameter attribute — expected one of \
                 `#[default(expr)]`, `#[positional_only]`, `#[keyword_only]`",
            ));
        }
    }

    let special_count =
        usize::from(varargs) + usize::from(varkwargs) + usize::from(state) + usize::from(state_mut);
    if special_count > 1 {
        return Err(syn::Error::new_spanned(
            &raw_attrs[0],
            "a parameter may have at most one of `#[varargs]`, `#[varkwargs]`, `#[state]`, or `#[state_mut]`",
        ));
    }
    if special_count == 1 && (default.is_some() || positional_only || keyword_only) {
        return Err(syn::Error::new_spanned(
            &raw_attrs[0],
            "`#[varargs]`, `#[varkwargs]`, `#[state]`, and `#[state_mut]` cannot be combined with \
             `#[default(...)]`, `#[positional_only]`, or `#[keyword_only]`",
        ));
    }

    Ok(ParamAttrs {
        default,
        positional_only,
        keyword_only,
        varargs,
        varkwargs,
        state,
        state_mut,
    })
}

/// Parse outer attributes on a `class ClassName { … }` block.
pub(crate) fn parse_class_attrs(attrs: Vec<Attribute>) -> syn::Result<ClassAttrs> {
    let mut cfg_attrs = Vec::new();
    let mut native_state: Option<Type> = None;

    for attr in attrs {
        if attr.path().is_ident("py_name") {
            return Err(syn::Error::new_spanned(
                &attr,
                "`#[py_name = \"...\"]` may not appear on a `class` declaration; \
                 use it on individual methods if a Python-level name needs overriding",
            ));
        } else if attr.path().is_ident("method_descriptor") {
            return Err(syn::Error::new_spanned(
                &attr,
                "`#[method_descriptor]` may only appear on a `fn` declaration; \
                 methods inside a `class` block are always method descriptors",
            ));
        } else if attr.path().is_ident("cfg") {
            cfg_attrs.push(attr);
        } else if attr.path().is_ident("native_state") {
            if native_state.is_some() {
                return Err(syn::Error::new_spanned(
                    &attr,
                    "`#[native_state(...)]` may appear at most once per class",
                ));
            }
            let ty: Type = attr.parse_args().map_err(|_| {
                syn::Error::new_spanned(
                    &attr,
                    "`#[native_state(MyState)]` requires a Rust type argument",
                )
            })?;
            native_state = Some(ty);
        } else if attr.path().is_ident("doc") || attr.path().is_ident("allow") {
            // Source-only doc comments / lint allowances are accepted.
        } else {
            return Err(syn::Error::new_spanned(
                &attr,
                "unsupported attribute on `class { … }` — expected `#[cfg(...)]`, \
                 `#[native_state(T)]`, `#[doc = ...]`, or `#[allow(...)]`",
            ));
        }
    }

    Ok(ClassAttrs {
        cfg_attrs,
        native_state,
    })
}

/// Parse outer attributes on an `exception MyError(Base);` declaration.
pub(crate) fn parse_exception_attrs(attrs: Vec<Attribute>) -> syn::Result<ExceptionAttrs> {
    let mut cfg_attrs = Vec::new();
    let mut doc_lines: Vec<String> = Vec::new();

    for attr in attrs {
        if attr.path().is_ident("cfg") {
            cfg_attrs.push(attr);
        } else if attr.path().is_ident("doc") {
            if let Ok(nv) = attr.meta.require_name_value()
                && let Expr::Lit(syn::ExprLit {
                    lit: Lit::Str(s), ..
                }) = &nv.value
            {
                let raw = s.value();
                let trimmed = raw.strip_prefix(' ').unwrap_or(&raw);
                doc_lines.push(trimmed.to_string());
            }
        } else if attr.path().is_ident("allow") {
            // Ignored on statement.
        } else {
            return Err(syn::Error::new_spanned(
                &attr,
                "unsupported attribute on `exception` — expected `#[cfg(...)]` or doc comments",
            ));
        }
    }

    let doc = (!doc_lines.is_empty()).then(|| doc_lines.join("\n"));
    Ok(ExceptionAttrs { cfg_attrs, doc })
}

/// Parse outer attributes on a `constants { … }` entry.
pub(crate) fn parse_const_attrs(attrs: Vec<Attribute>) -> syn::Result<ConstAttrs> {
    let mut cfg_attrs = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("cfg") {
            cfg_attrs.push(attr);
        } else if attr.path().is_ident("doc") || attr.path().is_ident("allow") {
            // Accepted for documentation / lint suppression.
        } else {
            return Err(syn::Error::new_spanned(
                &attr,
                "unsupported attribute on `constants` entry — expected `#[cfg(...)]`",
            ));
        }
    }
    Ok(ConstAttrs { cfg_attrs })
}
