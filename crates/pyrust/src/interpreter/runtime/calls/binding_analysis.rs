/// Format missing parameter names using CPython's diagnostic grammar.
fn format_missing_args(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => format!("'{only}'"),
        [first, second] => format!("'{first}' and '{second}'"),
        [init @ .., last] => {
            let quoted: Vec<String> = init.iter().map(|n| format!("'{n}'")).collect();
            format!("{}, and '{last}'", quoted.join(", "))
        }
    }
}

/// Validate required arguments after binding. Positional misses take
/// precedence over keyword-only misses, matching CPython.
fn check_missing_args(
    display: &str,
    missing_positional: &[&str],
    missing_kwonly: &[&str],
) -> Result<()> {
    if !missing_positional.is_empty() {
        let count = missing_positional.len();
        let arg_word = if count == 1 { "argument" } else { "arguments" };
        let names = format_missing_args(missing_positional);
        return Err(pyrust_core::type_err!(
            "{display}() missing {count} required positional {arg_word}: {names}"
        ));
    }
    if !missing_kwonly.is_empty() {
        let count = missing_kwonly.len();
        let arg_word = if count == 1 { "argument" } else { "arguments" };
        let names = format_missing_args(missing_kwonly);
        return Err(pyrust_core::type_err!(
            "{display}() missing {count} required keyword-only {arg_word}: {names}"
        ));
    }
    Ok(())
}

impl Interpreter {
    /// Decide whether a keyword call to `function` with `npos` positional
    /// arguments and the keyword names `kwnames` (in call order) binds *simply*
    /// — i.e. can be served by the `CallKw` fast bind without any CPython-parity
    /// diagnostic.  Returns `Some(slots)` (one parameter index per keyword name)
    /// when simple, or `None` when the call must take the general binder (which
    /// raises the correct TypeError, or absorbs leftovers into `**kwargs`).
    ///
    /// "Simple" requires, matching the general binder's success conditions:
    /// - the callee has no `*args` / `**kwargs` parameter (a `**kwargs` could
    ///   absorb an otherwise-unexpected keyword, so leave it to the slow path);
    /// - every keyword names a real, non-positional-only parameter;
    /// - no parameter is bound twice (a keyword duplicating a positional, or two
    ///   keywords naming the same param — the latter can't happen from distinct
    ///   names but is checked defensively);
    /// - the `npos` positionals fill leading non-keyword-only params and don't
    ///   overflow the positional capacity;
    /// - every still-unbound parameter has a default (no missing required arg).
    ///
    /// Any deviation returns `None`, so the general binder owns all error wording.
    pub(super) fn kwcall_resolve_simple(
        function: &Rc<UserFunction>,
        npos: usize,
        kwnames: &[Value],
    ) -> Option<smallvec::SmallVec<[u32; 4]>> {
        let params = &function.params;
        let nparams = params.len();
        // Reject variadic callees outright — the slow path handles *args/**kwargs
        // absorption and the diagnostics that depend on them.
        if params.iter().any(|p| p.is_args || p.is_kwargs) {
            return None;
        }
        // Positionals must fill leading params that can accept positional args.
        // A keyword-only param at/within 0..npos means a positional overflowed.
        if npos > nparams {
            return None;
        }
        for p in params.iter().take(npos) {
            if p.is_keyword_only {
                return None;
            }
        }
        // Per-param bound flags: positionals fill 0..npos.
        let mut bound: smallvec::SmallVec<[bool; 16]> = smallvec![false; nparams];
        for b in bound.iter_mut().take(npos) {
            *b = true;
        }
        let mut slots: smallvec::SmallVec<[u32; 4]> =
            smallvec::SmallVec::with_capacity(kwnames.len());
        for name_val in kwnames {
            let name = name_val.as_str()?;
            let pi = params.iter().position(|p| p.name == name)?;
            // Positional-only param named by keyword → general path raises the
            // "positional-only ... passed as keyword" TypeError.
            if params[pi].is_positional_only {
                return None;
            }
            // Already bound (by a positional or an earlier keyword) → general
            // path raises "multiple values for argument".
            if bound[pi] {
                return None;
            }
            bound[pi] = true;
            slots.push(pi as u32);
        }
        // Every unbound param must have a default (override-aware), else missing.
        for (pi, b) in bound.iter().enumerate() {
            if !*b {
                let has_default = if params[pi].is_keyword_only {
                    function.kwonly_default(pi).is_some()
                } else {
                    function.positional_default(pi).is_some()
                };
                if !has_default {
                    return None;
                }
            }
        }
        Some(slots)
    }
}
