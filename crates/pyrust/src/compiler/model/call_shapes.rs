/// Recognise the `f(<pos…>, **d)` shape eligible for the `CallEx` fast lowering
/// (issue #2393): exactly one `**d` double-splat, which must be the final arg,
/// preceded only by plain positional args (no `*a` splat, no literal `name=`
/// keyword).  Returns the number of leading positionals on a match, else `None`.
fn double_splat_fast_shape(args: &[crate::ast::CallArg]) -> Option<usize> {
    let n = args.len();
    if n == 0 {
        return None;
    }
    let last = &args[n - 1];
    if !last.double_splat {
        return None;
    }
    // Every preceding arg must be a plain positional.
    for a in &args[..n - 1] {
        if a.splat || a.double_splat || a.name.is_some() {
            return None;
        }
    }
    Some(n - 1)
}

/// Recognise the `f(<pos…>, *args[, **kw])` shape eligible for the `CallExArgs`
/// fast lowering (the decorator/wrapper shape): exactly one `*args` splat, an
/// optional trailing literal `kw=v` keyword args and one optional `**kw`
/// double-splat, preceded only by plain positional args (no positional after the
/// splat).  Returns `(npos, nkw)` — leading-positional count and literal-keyword
/// count — on a match, else `None`.
fn positional_splat_fast_shape(args: &[crate::ast::CallArg]) -> Option<(usize, usize)> {
    let n = args.len();
    if n == 0 {
        return None;
    }
    // Exactly one `*args` splat.
    if args.iter().filter(|a| a.splat).count() != 1 {
        return None;
    }
    // At most one `**kw`, and if present it must be the final arg.
    let ndsplat = args.iter().filter(|a| a.double_splat).count();
    if ndsplat > 1 || (ndsplat == 1 && !args[n - 1].double_splat) {
        return None;
    }
    let splat_pos = args.iter().position(|a| a.splat)?;
    // Every arg before the splat must be a plain positional.
    for a in &args[..splat_pos] {
        if a.splat || a.double_splat || a.name.is_some() {
            return None;
        }
    }
    // Between the splat and the optional trailing `**kw`, every arg must be a
    // literal `name=value` keyword (no second splat, no bare positional).
    let kw_end = if ndsplat == 1 { n - 1 } else { n };
    for a in &args[splat_pos + 1..kw_end] {
        if a.splat || a.double_splat || a.name.is_none() {
            return None;
        }
    }
    let npos = splat_pos;
    let nkw = kw_end - (splat_pos + 1);
    // A literal keyword together with a `**kw` splat can name the SAME key (e.g.
    // `f(*a, x=1, **{'x': 2})`), which CPython rejects as "got multiple values for
    // keyword argument 'x'" at merge time. Detecting that cross-source collision
    // is the generic materializing path's job (DICT_MERGE) — keep that shape on
    // it rather than silently letting one value win. Literal keywords alone are unique
    // (duplicate `kw=` is a syntax error) and never collide with each other.
    if nkw > 0 && ndsplat == 1 {
        return None;
    }
    // Leading positionals *before* the splat together with literal keywords *after*
    // it (`f(p, *a, kw=v)`) force CPython to materialise and ITERATE the splat while
    // building the positional tuple (`BUILD_LIST` + `LIST_EXTEND` + `LIST_TO_TUPLE`)
    // BEFORE it evaluates the keyword values — the iteration side effects of a
    // generator / iterator `*a` are observable in that order.  The `CallExArgs`
    // lowering instead defers splat iteration to call time (after the keyword values
    // are already evaluated), so it would reorder those side effects. Keep this
    // shape on the materializing path, which preserves the ordering. (With no leading
    // positional, CPython also defers the splat into `CALL_FUNCTION_EX`, so
    // `f(*a, kw=v)` stays on the fast path and matches.)
    if npos > 0 && nkw > 0 {
        return None;
    }
    // u8-encodable counts (the opcode stores `npos` and `nkw` as `u8`).
    if npos > u8::MAX as usize || nkw > u8::MAX as usize {
        return None;
    }
    Some((npos, nkw))
}
