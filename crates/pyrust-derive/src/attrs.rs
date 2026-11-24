//! Attribute parsing and identifier helpers for `pyrust-derive`.

use syn::parse::ParseStream;
use syn::{Attribute, Expr, Ident, Lit, LitStr, Meta};

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

/// Extracted outer attributes on a `fn` declaration inside `pyrust_module!`.
pub(crate) struct FnAttrs {
    /// Outer attributes forwarded onto the generated Rust fn (`#[doc = …]`, etc.).
    pub(crate) retained: Vec<Attribute>,
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
}

/// Extracted attributes on a single typed-dialect parameter.
pub(crate) struct ParamAttrs {
    pub(crate) default: Option<Expr>,
    pub(crate) positional_only: bool,
    pub(crate) keyword_only: bool,
}

/// Strip a leading `r#` raw-identifier prefix so Python names (e.g. `r#mod` ->
/// `"mod"`, `r#type` -> `"type"`) match Python's surface names and error wording.
pub(crate) fn unraw_ident(ident: &Ident) -> String {
    let s = ident.to_string();
    s.strip_prefix("r#").map(str::to_owned).unwrap_or(s)
}

/// Returns true if `s` is a Rust snake_case identifier — lowercase letters,
/// digits, and underscores only.
pub(crate) fn is_snake_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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

/// Partition outer attributes on a `pyrust_module!` fn into recognised
/// macro directives and retained Rust attributes.
pub(crate) fn parse_fn_attrs(raw_attrs: Vec<Attribute>) -> syn::Result<FnAttrs> {
    let mut py_name_override: Option<LitStr> = None;
    let mut arity_style = ArityStyle::Standard;
    let mut arity_style_set = false;
    let mut bare_name = false;
    let mut method_descriptor = false;
    let mut retained = Vec::with_capacity(raw_attrs.len());

    for attr in raw_attrs {
        if attr.path().is_ident("arity_style") {
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
        py_name_override,
        arity_style,
        arity_style_set,
        bare_name,
        method_descriptor,
    })
}

/// Harvest recognised parameter attributes (`#[default(expr)]`,
/// `#[positional_only]`, `#[keyword_only]`) and reject unknown ones.
pub(crate) fn parse_param_attrs(raw_attrs: &[Attribute]) -> syn::Result<ParamAttrs> {
    let mut default: Option<Expr> = None;
    let mut positional_only = false;
    let mut keyword_only = false;

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

    Ok(ParamAttrs {
        default,
        positional_only,
        keyword_only,
    })
}

/// Validate outer attributes on a `class ClassName { … }` block.
///
/// Only doc comments are accepted on a class block (which lowers to a
/// statement, so `#[cfg]` / `#[allow]` would be silently dropped).
pub(crate) fn validate_class_attrs(attrs: &[Attribute]) -> syn::Result<()> {
    if attrs.iter().any(|a| a.path().is_ident("py_name")) {
        return Err(syn::Error::new_spanned(
            &attrs[0],
            "`#[py_name = \"...\"]` may not appear on a `class` declaration; \
             use it on individual methods if a Python-level name needs overriding",
        ));
    }
    if attrs.iter().any(|a| a.path().is_ident("method_descriptor")) {
        return Err(syn::Error::new_spanned(
            &attrs[0],
            "`#[method_descriptor]` may only appear on a `fn` declaration; \
             methods inside a `class` block are always method descriptors",
        ));
    }
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            return Err(syn::Error::new_spanned(
                attr,
                "only doc-comment attrs are supported on `class { … }`; \
                 other attrs (e.g. `#[cfg]`, `#[allow]`) would be silently dropped",
            ));
        }
    }
    Ok(())
}
