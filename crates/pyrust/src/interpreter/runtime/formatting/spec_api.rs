// Python format-spec parsing and primitive rendering.

/// Apply a Python format spec string to a `Value` and return the formatted string.
///
/// Implements the [Python format-spec mini-language][docs]:
///
/// ```text
/// [[fill]align][sign][#][0][width][grouping][.precision][type]
/// ```
///
/// Supported components for the built-in numeric / string types:
/// - **fill / align** (`<`, `>`, `^`, `=`) with any single fill character
/// - **sign** (`+`, `-`, ` `)
/// - **alternate form** `#` for `b`, `o`, `x`, `X` (and float types)
/// - **zero-pad** `0` (implies sign-aware `=` alignment)
/// - **width** (decimal integer)
/// - **grouping** `,` (comma) or `_` (underscore)
/// - **precision** `.N` for floats and strings
/// - **type** `b`, `c`, `d`, `e`, `E`, `f`, `F`, `g`, `G`, `n`, `o`, `s`, `x`, `X`, `%`
///
/// Complex values support the bare width / fill / align spec (matching
/// CPython's `format(1+2j, ">10")` -> `"    (1+2j)"`) but do not yet accept
/// numeric type codes (`e`/`f`/`g`) or sign / precision / grouping / `#` /
/// `0` — those will raise ValueError.
///
/// Not yet implemented: locale-aware grouping (`n` and float `n` types),
/// Complex with explicit numeric type codes, and non-ASCII fill characters
/// in nested f-string specs round-trip through `str.format` as bytes rather
/// than chars.  These gaps mirror documented pyrust limitations.
///
/// [docs]: https://docs.python.org/3/library/string.html#format-specification-mini-language
pub(crate) fn apply_format_spec(value: &Value, spec: &str) -> Result<Value> {
    apply_format_spec_named(value, spec, None)
}

/// Like [`apply_format_spec`], but lets the caller name the type reported in the
/// "unsupported format string passed to <type>.__format__" `TypeError`.  When
/// `owner` is `Some`, that name is used verbatim (the actual subclass, e.g. `B`
/// for `class B(bytes)`); when `None`, the value's own builtin type name is
/// used.  CPython names the *actual* type the spec was passed to, not the
/// backing primitive a subclass wraps (issue #2212).
pub(crate) fn apply_format_spec_named(
    value: &Value,
    spec: &str,
    owner: Option<&str>,
) -> Result<Value> {
    if spec.is_empty() {
        // gh-95778: an empty spec renders an int in base 10 — enforce the
        // int_max_str_digits limit.  The `value_may_exceed_int_str_limit` tag
        // test keeps the hot `f"{int}"` / `format(int)` path free of the error
        // branch entirely; only BigInt/containers enter the checked path.
        if pyrust_core::value_may_exceed_int_str_limit(value) {
            pyrust_core::check_int_str_conversion(value)?;
        }
        return Ok(Value::string(value.to_py_str()));
    }

    // Types that inherit the default `object.__format__` (None, list, tuple,
    // dict, set, bytes, function, module, …) reject any non-empty format spec
    // with a TypeError, mirroring CPython 3.12.  Only the value kinds that
    // provide a real `__format__` (str / int / bool / float / complex) accept
    // a spec; everything else is rejected here.
    if !value_has_real_format(value) {
        let type_name = owner
            .map(std::borrow::Cow::Borrowed)
            .unwrap_or_else(|| pyrust_core::builtin_type_name(value));
        return Err(pyrust_core::type_err!(
            "unsupported format string passed to {}.__format__",
            type_name
        ));
    }

    // The type name CPython reports in format-spec ValueErrors ("Invalid
    // format specifier '…' for object of type '<type>'", "Unknown format code
    // '…' for object of type '<type>'", …) is the *actual* type the spec was
    // passed to: the subclass for a built-in subclass instance (`owner`), the
    // value's own builtin name otherwise.  Compute it once and thread it through
    // the parse + render so every spec error names the same type.
    let type_name = owner
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_else(|| pyrust_core::builtin_type_name(value));
    let parsed = parse_format_spec(spec, &type_name)?;
    let formatted = render_format_spec(value, &parsed, &type_name)?;
    Ok(Value::string(formatted))
}

/// Per-instruction cache for a constant f-string format spec
/// (`FormatValueSpec` opcode, issue #2357 / #2372).
///
/// A constant spec such as `".2f"` in `f"{x:.2f}"` is parsed once and cached
/// here, keyed by the *instruction pc*, so the per-iteration `parse_format_spec`
/// scan is eliminated from spec-heavy hot loops.  The slot is validated against
/// the spec string's backing pointer on every read: a constant spec loads the
/// same interned const-pool string each iteration (pointer hit), while a dynamic
/// spec (`f"{x:{w}f}"`) allocates a fresh string each time (pointer miss → the
/// cache is bypassed and the spec is parsed normally, never cached).
///
/// The cache lives in a side table indexed by pc, so it is immune to the const
/// remapping `pass_compact_consts` performs (the landmine called out in #2372).
#[derive(Clone, Debug)]
pub(crate) enum FmtSpecCacheEntry {
    Empty,
    Cached {
        /// Byte length of the spec string that produced `parsed` — a cheap
        /// fast-reject before the content compare.
        spec_len: usize,
        /// The parsed spec.  Parsing is value-independent (it depends only on
        /// the spec text), so the same parse is reused for every value rendered
        /// through this site; only `render_format_spec` re-runs per value.
        parsed: Rc<FormatSpec>,
        /// The spec string that produced `parsed`.  The cache hit is decided by
        /// comparing its *content* against the current spec — NOT by backing
        /// pointer: a small (inline / SSO) spec string carries its bytes in the
        /// NaN-box, so `as_ptr()` points at the transient stack/register slot
        /// the value currently occupies, which is reused across iterations and
        /// would false-hit when the spec changes (e.g. `.1f` → `.2f`, #2832).
        spec: Value,
    },
}

/// Typed identity of a callable handled by the registered-builtin call cache.
///
/// Registry names identify `ValueKind::BuiltinFunction`; weak class identities
/// identify every exact singleton in the primitive-constructor dispatch map.
/// Keeping the namespaces distinct prevents a same-named user class from
/// reusing a builtin-function entry.
#[derive(Clone, Debug)]
pub(crate) enum CallBuiltinCacheKey {
    RegistryName(&'static str),
    PrimitiveClass(
        /// A weak allocation identity prevents ABA reuse while adding no
        /// Python-visible owner and requiring no upgrade on a warm hit.
        std::rc::Weak<std::cell::RefCell<pyrust_core::PyClass>>,
    ),
}

/// Per-call-site inline cache for a registered built-in callee, so hot calls
/// such as `len(x)`, `math.sqrt(x)`, and `zip(a, b)` skip generic call routing
/// and the registry binary search on every iteration. Cacheability follows
/// immutable registry membership or a canonical built-in-class identity.
/// `Clone` lets the `Vec` initialise with `vec![Empty; n]`; class entries are
/// deliberately not `Copy` because their weak allocation identity owns a
/// control-block reference.
#[derive(Clone, Debug)]
pub(crate) enum CallBuiltinCacheEntry {
    Empty,
    Cached {
        /// Immutable identity that resolved to `dispatch`; polymorphic sites
        /// re-resolve and overwrite the entry when this key changes.
        key: CallBuiltinCacheKey,
        dispatch: crate::builtin_registry::BuiltinDispatchFn,
        /// Optional "vectorcall" fast entry + inclusive positional-arity bounds
        /// `(min, max)`.  When present and the call's `argc` is in range, the VM
        /// passes the argument values as a register subslice and skips the
        /// `ExpandedCallArg` buffer + kwarg/arity validation entirely.
        fast: Option<(crate::builtin_registry::BuiltinFastDispatchFn, u8, u8)>,
    },
    /// Exact class identity already proved absent from
    /// `PRIMITIVE_CLASS_DISPATCH`. A warm hit skips only that identity-map
    /// lookup and still enters the shared typing/adapter/generic-class tail.
    ClassAfterPrimitiveMiss(
        /// Weak identity prevents both ownership extension and allocator ABA.
        std::rc::Weak<std::cell::RefCell<pyrust_core::PyClass>>,
    ),
}

/// Apply a (usually constant) f-string format spec to `value`, consulting a
/// per-pc parse cache so a constant spec is parsed only once.  Mirrors
/// [`apply_format_spec`] for non-`PyInstance` values; `PyInstance` values must
/// still go through `dispatch_dunder_format` (user `__format__`) and never reach
/// here.  `cache` is the caller's `FnCode::fmt_spec_cache`; `idx` is the `pc` of
/// the executing `FormatValueSpec` instruction.
pub(crate) fn apply_format_spec_cached(
    value: &Value,
    spec_val: &Value,
    cache: &RefCell<Vec<FmtSpecCacheEntry>>,
    idx: usize,
) -> Result<Value> {
    let spec = spec_val.as_str().unwrap_or("");
    if spec.is_empty() {
        if pyrust_core::value_may_exceed_int_str_limit(value) {
            pyrust_core::check_int_str_conversion(value)?;
        }
        return Ok(Value::string(value.to_py_str()));
    }

    if !value_has_real_format(value) {
        let type_name = pyrust_core::builtin_type_name(value);
        return Err(pyrust_core::type_err!(
            "unsupported format string passed to {}.__format__",
            type_name
        ));
    }

    let type_name = pyrust_core::builtin_type_name(value);

    // Fast path: the spec string has the same *content* we last parsed at this
    // pc.  `spec_len` fast-rejects most misses without a byte compare; only a
    // same-length spec pays the (short) content comparison.  Clone the parsed
    // `Rc` out under a short borrow so the cache `RefCell` is released before
    // `render_format_spec` runs.
    let spec_len = spec.len();
    let cached_parsed = match &cache.borrow()[idx] {
        FmtSpecCacheEntry::Cached {
            spec_len: cl,
            parsed,
            spec: cached_spec,
        } if *cl == spec_len && cached_spec.as_str() == Some(spec) => Some(Rc::clone(parsed)),
        _ => None,
    };
    if let Some(parsed) = cached_parsed {
        let formatted = render_format_spec(value, &parsed, &type_name)?;
        return Ok(Value::string(formatted));
    }

    // Miss: parse once, render, and cache against this spec's content.
    let parsed = Rc::new(parse_format_spec(spec, &type_name)?);
    let formatted = render_format_spec(value, &parsed, &type_name)?;
    cache.borrow_mut()[idx] = FmtSpecCacheEntry::Cached {
        spec_len,
        parsed,
        spec: spec_val.clone(),
    };
    Ok(Value::string(formatted))
}

/// Render a format-spec character (a presentation-type code) the way CPython
/// embeds it in an "Unknown format code" error message.  CPython emits the
/// code point literally for the ASCII range `0x20..=0x7f` (note: DEL, 0x7f, is
/// kept raw); a control character (`< 0x20`) or any non-ASCII / astral code
/// point (`>= 0x80`) is escaped as `\xHEX` with lowercase, non-zero-padded hex
/// (e.g. U+1D11E -> `\x1d11e`, U+00E9 -> `\xe9`).
fn format_code_repr(c: char) -> String {
    let cp = c as u32;
    if (0x20..=0x7f).contains(&cp) {
        c.to_string()
    } else {
        format!("\\x{cp:x}")
    }
}

/// True when `value`'s type provides a real `__format__` that honours a format
/// spec (`str`, `int`/`bool`/`BigInt`, `float`, `complex`).  Every other type
/// inherits the default `object.__format__`, which rejects non-empty specs.
fn value_has_real_format(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::Str(_)
            | ValueKind::Int(_)
            | ValueKind::BigInt(_)
            | ValueKind::Bool(_)
            | ValueKind::Float(_)
            | ValueKind::Complex(_, _)
    )
}

/// Validate the positional args of a builtin `obj.__format__(spec)` call and
/// return the borrowed spec string.  Mirrors CPython 3.12's error wording:
/// a non-`str` spec → `__format__() argument must be str, not <type>`; more
/// than one argument → `<owner>.__format__() takes exactly one argument (N
/// given)`, where `<owner>` is the receiver's own type when it defines a real
/// `__format__` (int/float/str/bool/complex) and `object` otherwise (the
/// inherited `object.__format__`).  Shared by the two `__format__` method-call
/// dispatch sites (bound-method wrapper + tagged-container opcode), #2191.
pub(super) fn format_dunder_spec_arg<'a>(receiver: &Value, pos: &'a [Value]) -> Result<&'a str> {
    if pos.len() > 1 {
        return Err(pyrust_core::type_err!(
            "{}.__format__() takes exactly one argument ({} given)",
            format_dunder_owner(receiver),
            pos.len()
        ));
    }
    match pos.first() {
        None => Ok(""),
        Some(v) => v.as_str().ok_or_else(|| {
            pyrust_core::type_err!(
                "__format__() argument must be str, not {}",
                pyrust_core::builtin_type_name(v)
            )
        }),
    }
}

/// The owner type name CPython 3.12 names in a builtin `__format__` arg error:
/// the receiver's own type when it defines a real `__format__`
/// (int/float/str/bool/complex), `object` otherwise (the inherited
/// `object.__format__`).  Shared by the spec-arg and keyword-arg validators so
/// both `__format__` method-call dispatch sites use the same wording.
pub(super) fn format_dunder_owner(receiver: &Value) -> std::borrow::Cow<'static, str> {
    // `bool` inherits `int.__format__`; CPython names the type that *defines*
    // `__format__` in the MRO, so `True.__format__(...)` errors name `int`.
    if matches!(receiver.kind(), ValueKind::Bool(_)) {
        return std::borrow::Cow::Borrowed("int");
    }
    // A built-in subclass instance (`class I(int)`) without a `__format__`
    // override resolves the method to the backing type's `__format__` in
    // CPython, so an arg error names that backing type (`int`), not `object`
    // (issue #2214).
    if let Some(backing) = builtin_data_backing(receiver) {
        return format_dunder_owner(&backing);
    }
    if value_has_real_format(receiver) {
        pyrust_core::builtin_type_name(receiver)
    } else {
        std::borrow::Cow::Borrowed("object")
    }
}
