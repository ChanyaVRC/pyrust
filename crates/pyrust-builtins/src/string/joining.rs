/// Return the CPython type name for a `PyKey` variant — used in join's
/// "sequence item N: expected str instance, X found" error messages.
/// `PyKey::Object` stores the original `Value`, so we derive the name
/// via `builtin_type_name` rather than hardcoding "object" (#576 Copilot
/// review: use the runtime class name, e.g. "MyKey").
fn pykey_type_name(k: &PyKey) -> std::borrow::Cow<'static, str> {
    match k {
        PyKey::Int(_) | PyKey::BigInt(_) => std::borrow::Cow::Borrowed("int"),
        PyKey::Float(_) => std::borrow::Cow::Borrowed("float"),
        PyKey::Bool(_) => std::borrow::Cow::Borrowed("bool"),
        PyKey::Str(_) => std::borrow::Cow::Borrowed("str"),
        PyKey::None => std::borrow::Cow::Borrowed("NoneType"),
        PyKey::Ellipsis => std::borrow::Cow::Borrowed("ellipsis"),
        PyKey::FrozenSet(_) => std::borrow::Cow::Borrowed("frozenset"),
        PyKey::Tuple(_) => std::borrow::Cow::Borrowed("tuple"),
        PyKey::Bytes(_) => std::borrow::Cow::Borrowed("bytes"),
        PyKey::Complex(_, _) => std::borrow::Cow::Borrowed("complex"),
        PyKey::Object { value, .. } => builtin_type_name(value),
    }
}

/// Build the joined string from an iterator that yields each element as a
/// validated `&str`. The single pass validates every element (so a non-str
/// element raises before any work) and stashes the borrowed slices, summing
/// their byte lengths. The result is then filled directly into the string's
/// backing buffer via `string_from_fill` — no intermediate `String`, so the
/// joined bytes are touched exactly once (the copy) instead of three times
/// (push into a `String`, an `is_ascii` rescan, then the final memcpy).
fn join_borrowed<'a, I>(sep: &str, parts: I) -> Result<Value>
where
    I: ExactSizeIterator<Item = Result<&'a str>>,
{
    let n = parts.len();
    if n == 0 {
        return Ok(Value::string(String::new()));
    }
    // Validate every element up front, stashing the borrowed slice and summing
    // byte lengths.  Borrowing keeps the build pass infallible; a SmallVec keeps
    // the common small join off the heap (no allocation for up to 16 parts).
    let mut slices: smallvec::SmallVec<[&str; 16]> = smallvec::SmallVec::with_capacity(n);
    let mut body_len = 0usize;
    for part in parts {
        let s = part?;
        body_len += s.len();
        slices.push(s);
    }
    let total = body_len + sep.len() * (n - 1);
    // For short results the ASCII scan is cache-hot and benefits later index /
    // find / slice ops, so compute it eagerly during the copy.  For large
    // results that scan would roughly double the bytes touched, so leave the
    // flag uncomputed (`None`) and let `str_is_ascii` resolve it lazily if ever
    // queried.  256 bytes keeps the common small join eager.
    let eager_ascii = total <= 256;
    let sep_ascii = sep.is_ascii();
    // SAFETY: every slice is a validated `&str` and `sep` is a `&str`, so the
    // bytes written are valid UTF-8, and we write exactly `total` bytes.
    Ok(unsafe {
        Value::string_from_fill(total, |buf| {
            let mut off = 0usize;
            let mut all_ascii = sep_ascii;
            for (i, s) in slices.iter().enumerate() {
                if i != 0 {
                    buf[off..off + sep.len()].copy_from_slice(sep.as_bytes());
                    off += sep.len();
                }
                let b = s.as_bytes();
                buf[off..off + b.len()].copy_from_slice(b);
                off += b.len();
                if eager_ascii {
                    all_ascii &= s.is_ascii();
                }
            }
            eager_ascii.then_some(all_ascii)
        })
    })
}

fn join(sep: &str, args: &[Value]) -> Result<Value> {
    let iterable = args
        .first()
        .ok_or_else(|| PyError::Runtime("str.join() requires 1 argument".to_string()))?;
    // Borrow each element as &str (no owned String per element); the result
    // string is allocated exactly once. The borrow guard from `kind()` must
    // stay alive across the build, so do it inside each arm.
    match iterable.kind() {
        ValueKind::List(items) => join_borrowed(
            sep,
            items.iter().enumerate().map(|(i, v)| match v.kind() {
                ValueKind::Str(s) => Ok(s),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected str instance, {} found",
                        builtin_type_name(v),
                    ),
                )),
            }),
        ),
        ValueKind::Tuple(items) => join_borrowed(
            sep,
            items.iter().enumerate().map(|(i, v)| match v.kind() {
                ValueKind::Str(s) => Ok(s),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected str instance, {} found",
                        builtin_type_name(v),
                    ),
                )),
            }),
        ),
        ValueKind::Dict(d) => join_borrowed(
            sep,
            d.keys().enumerate().map(|(i, k)| match k {
                PyKey::Str(s) => Ok(s.as_str().unwrap_or("")),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected str instance, {} found",
                        pykey_type_name(k),
                    ),
                )),
            }),
        ),
        ValueKind::Str(s) => {
            // Iterating a str yields single chars; join them with `sep`
            // between each, allocating the result once.
            let n = s.chars().count();
            if n == 0 {
                return Ok(Value::string(String::new()));
            }
            let total = s.len() + sep.len() * (n - 1);
            let mut out = String::with_capacity(total);
            for (i, c) in s.chars().enumerate() {
                if i != 0 {
                    out.push_str(sep);
                }
                out.push(c);
            }
            Ok(Value::string(out))
        }
        _ => Err(PyError::named(
            "TypeError",
            "can only join an iterable".to_string(),
        )),
    }
}
