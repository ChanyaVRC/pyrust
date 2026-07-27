// ─── C-level arity wordings (#2331) ────────────────────────────────────────────
//
// CPython's hand-written C builtins do *not* use the argument-clinic
// "takes N positional arguments but M were given" / "missing required
// argument" wordings the default dialect emits; they raise distinct
// C-level messages.  The `#[arity_style(...)]` dialect attribute selects
// one of these so a migrated builtin reproduces CPython byte-for-byte.
// See `pyrust-derive`'s `ArityStyle` and the typed-prelude emit.

/// `takes exactly one argument (N given)` — the METH_O / one-argument
/// C-builtin wording (`len`, `repr`, `hash`, `ord`, `chr`, `abs`,
/// `math.sqrt`, …).  Used for both the too-few and too-many cases (any
/// `positional_len != 1`), so the per-parameter `missing_arg` path is
/// never reached for these functions.
pub(crate) fn check_exactly_one_argument(fn_name: &str, positional_len: usize) -> Result<()> {
    if positional_len != 1 {
        return Err(type_error(format!(
            "{fn_name}() takes exactly one argument ({positional_len} given)"
        )));
    }
    Ok(())
}

/// `NAME expected N arguments, got M` (and the `at least` / `at most`
/// variants) — the METH_VARARGS C-builtin wording used by `isinstance`,
/// `issubclass`, `divmod`, `hasattr`, … .  Note CPython prints the bare
/// function name **without** trailing `()` for this style (unlike every
/// other dialect message), so `fn_name` is interpolated raw.  Handles
/// both the lower and upper bound; the per-parameter `missing_arg` path
/// is unreachable when this guard is used.
pub(crate) fn check_arity_expected_got(
    fn_name: &str,
    positional_len: usize,
    min: usize,
    max: usize,
) -> Result<()> {
    if positional_len < min || positional_len > max {
        let bound = if min == max {
            let plural = if max == 1 { "argument" } else { "arguments" };
            format!("expected {max} {plural}")
        } else if positional_len < min {
            let plural = if min == 1 { "argument" } else { "arguments" };
            format!("expected at least {min} {plural}")
        } else {
            let plural = if max == 1 { "argument" } else { "arguments" };
            format!("expected at most {max} {plural}")
        };
        return Err(type_error(format!(
            "{fn_name} {bound}, got {positional_len}"
        )));
    }
    Ok(())
}

/// Construct the "no overload matched" error.  Emitted by the macro-
/// generated dispatcher of a typed-overload builtin when every declared
/// overload's parameter types failed `FromValue::matches` against the
/// actual call args.  Unreachable in practice when the overload set
/// includes a `PyValue` catch-all (whose `matches` is unconditional);
/// reachable otherwise — the user supplied types not covered by any
/// overload.
///
/// The wording follows CPython's binary-op `unsupported operand type(s)
/// for +: 'int' and 'str'` shape — terse, prints only the actual
/// argument types, omits the declared overload signatures.  Per the
/// design review on #395 (comment 4443208232): "actual types only, no
/// signature dump unless behind a debug flag."
///
/// `actuals` is the type-name list of the *call site*'s args (e.g.
/// `["str", "int"]`).
pub(crate) fn no_overload_matched<T>(
    fn_name: &str,
    actuals: &[std::borrow::Cow<'static, str>],
) -> Result<T> {
    let joined = actuals
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(type_error(format!(
        "{fn_name}(): unsupported argument type(s): ({joined})",
    )))
}
