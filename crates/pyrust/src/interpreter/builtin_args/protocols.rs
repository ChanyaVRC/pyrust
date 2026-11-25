// ─── Protocol-driven FromValue wrappers (#3268) ──────────────────────────────
//
// - `PyBytesLike<'a>`: read-only byte buffer from `bytes`, `bytearray`,
//   `memoryview`, their subclasses, or PEP 688 `__buffer__` exporters.
// - `PyPath`: filesystem path from `str`, `bytes`, or `os.PathLike` via
//   `__fspath__()`.
// - `PyIndex`: integer resolved via Python's `__index__` protocol.
// - `PyNumber`: real number (`f64`) via `float`/`int`/`bool`/`__float__`/`__index__`.
// - `PyCallable`: validated callable `Value`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

// ─── PyBytesLike ─────────────────────────────────────────────────────────────

/// Read-only bytes-like argument (`bytes`, `bytearray`, `memoryview`,
/// subclasses, or PEP 688 `__buffer__` objects).
///
/// Zero-copy (`Cow::Borrowed`) when passed a plain `bytes` value; materialises
/// an owned `Vec<u8>` for mutable/view/PEP-688 buffers (always releasing
/// acquired PEP 688 views via `__release_buffer__`).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct PyBytesLike<'a>(pub Cow<'a, [u8]>);

#[allow(dead_code)]
impl<'a> PyBytesLike<'a> {
    #[inline]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[inline]
    pub(crate) fn into_vec(self) -> Vec<u8> {
        self.0.into_owned()
    }
}

impl<'a> Deref for PyBytesLike<'a> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl<'a> AsRef<[u8]> for PyBytesLike<'a> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<&'static [u8]> for PyBytesLike<'static> {
    fn from(b: &'static [u8]) -> Self {
        PyBytesLike(Cow::Borrowed(b))
    }
}

impl From<Vec<u8>> for PyBytesLike<'static> {
    fn from(b: Vec<u8>) -> Self {
        PyBytesLike(Cow::Owned(b))
    }
}

#[allow(dead_code)]
fn try_extract_native_bytes_like<'a>(value: &'a Value) -> Result<Option<Cow<'a, [u8]>>> {
    if let ValueKind::Bytes(rc) = value.kind() {
        return Ok(Some(Cow::Borrowed(rc.as_slice())));
    }
    if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(value) {
        return Ok(Some(Cow::Owned(snap)));
    }
    if pyrust_builtins::memoryview::is_memoryview(value) {
        let bytes = pyrust_builtins::memoryview::to_bytes_checked(value)?;
        return Ok(Some(Cow::Owned(bytes)));
    }
    if let Some(backing) = crate::interpreter::builtin_data_backing(value) {
        if let ValueKind::Bytes(rc) = backing.kind() {
            return Ok(Some(Cow::Owned(rc.as_ref().clone())));
        }
        if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
            return Ok(Some(Cow::Owned(snap)));
        }
    }
    Ok(None)
}

impl<'a> FromValue<'a> for PyBytesLike<'a> {
    const PY_TYPE_NAME: &'static str = "a bytes-like object";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if let Some(buf) = try_extract_native_bytes_like(value)? {
            return Ok(PyBytesLike(buf));
        }
        Err(must_be_error(fn_name, arg_name, Self::PY_TYPE_NAME, value))
    }

    fn try_from_value_with_interp(
        interp: &mut crate::interpreter::Interpreter,
        value: &'a Value,
        fn_name: &str,
        arg_name: &str,
    ) -> Result<Self> {
        // Fast native paths first (zero-copy for `bytes`).
        if let ValueKind::Bytes(rc) = value.kind() {
            return Ok(PyBytesLike(Cow::Borrowed(rc.as_slice())));
        }
        if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(value) {
            return Ok(PyBytesLike(Cow::Owned(snap)));
        }
        if pyrust_builtins::memoryview::is_memoryview(value) {
            let bytes = pyrust_builtins::memoryview::to_bytes_checked(value)?;
            return Ok(PyBytesLike(Cow::Owned(bytes)));
        }
        // PEP 688 `__buffer__` exporters (including `array.array` and user
        // classes) take precedence over `__builtin_data__` fallback, matching
        // `acquire_buffer_view`.
        if matches!(value.kind(), ValueKind::PyInstance(_)) {
            if let Some(buffer_method) =
                crate::interpreter::lookup_value_special_method(value, "__buffer__").transpose()?
            {
                let is_inherited_builtin = matches!(
                    buffer_method.kind(),
                    ValueKind::BuiltinFunction("bytes.__buffer__" | "bytearray.__buffer__")
                );
                if !is_inherited_builtin {
                    let mv = crate::interpreter::acquire_buffer_view(
                        interp,
                        value,
                        crate::interpreter::BUF_FULL_RO,
                    )?;
                    let extract_res = pyrust_builtins::memoryview::to_bytes_checked(&mv);
                    let release_res =
                        crate::interpreter::release_memoryview_with_interp(interp, &mv);
                    let bytes = extract_res?;
                    release_res?;
                    return Ok(PyBytesLike(Cow::Owned(bytes)));
                }
            }
            if let Some(backing) = crate::interpreter::builtin_data_backing(value) {
                if let ValueKind::Bytes(rc) = backing.kind() {
                    return Ok(PyBytesLike(Cow::Owned(rc.as_ref().clone())));
                }
                if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
                    return Ok(PyBytesLike(Cow::Owned(snap)));
                }
            }
        }
        Err(must_be_error(fn_name, arg_name, Self::PY_TYPE_NAME, value))
    }

    fn matches(value: &'a Value) -> bool {
        match value.kind() {
            ValueKind::Bytes(_) => true,
            ValueKind::BuiltinObject { ops, .. } => {
                ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray)
                    || pyrust_builtins::memoryview::is_memoryview_ops(ops)
            }
            ValueKind::PyInstance(inst) => {
                let class = Rc::clone(&inst.borrow().class);
                if crate::interpreter::lookup_class_attr(&class, "__buffer__").is_some() {
                    return true;
                }
                crate::interpreter::builtin_data_backing(value).is_some_and(|b| {
                    matches!(b.kind(), ValueKind::Bytes(_))
                        || pyrust_builtins::bytearray::as_bytearray_snapshot(&b).is_some()
                })
            }
            _ => false,
        }
    }
}

// ─── PyPath ──────────────────────────────────────────────────────────────────

/// Filesystem path argument extracted from `str`, `bytes`, their subclasses,
/// or `os.PathLike` objects via `__fspath__()`.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PyPath {
    path: PathBuf,
    from_bytes: bool,
}

#[allow(dead_code)]
impl PyPath {
    #[inline]
    pub(crate) fn as_path(&self) -> &Path {
        &self.path
    }

    #[inline]
    pub(crate) fn into_path_buf(self) -> PathBuf {
        self.path
    }

    #[inline]
    pub(crate) fn as_os_str(&self) -> &OsStr {
        self.path.as_os_str()
    }

    #[inline]
    pub(crate) fn into_os_string(self) -> OsString {
        self.path.into_os_string()
    }

    #[inline]
    pub(crate) fn is_bytes(&self) -> bool {
        self.from_bytes
    }
}

impl Deref for PyPath {
    type Target = Path;

    #[inline]
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for PyPath {
    #[inline]
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<OsStr> for PyPath {
    #[inline]
    fn as_ref(&self) -> &OsStr {
        self.path.as_os_str()
    }
}

impl From<&str> for PyPath {
    fn from(s: &str) -> Self {
        PyPath {
            path: PathBuf::from(s),
            from_bytes: false,
        }
    }
}

impl From<String> for PyPath {
    fn from(s: String) -> Self {
        PyPath {
            path: PathBuf::from(s),
            from_bytes: false,
        }
    }
}

impl From<PathBuf> for PyPath {
    fn from(path: PathBuf) -> Self {
        PyPath {
            path,
            from_bytes: false,
        }
    }
}

#[allow(dead_code)]
fn bytes_to_path_buf(bytes: &[u8]) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
    }
    #[cfg(not(unix))]
    {
        let s = std::str::from_utf8(bytes).map_err(|_| {
            PyError::named(
                "UnicodeDecodeError",
                "'utf-8' codec can't decode bytes in path".to_string(),
            )
        })?;
        Ok(PathBuf::from(s))
    }
}

#[allow(dead_code)]
fn extract_str_or_bytes_path(value: &Value) -> Result<Option<PyPath>> {
    if let Some(s) = value.as_str() {
        return Ok(Some(PyPath {
            path: PathBuf::from(s),
            from_bytes: false,
        }));
    }
    if let ValueKind::Bytes(rc) = value.kind() {
        return Ok(Some(PyPath {
            path: bytes_to_path_buf(rc.as_slice())?,
            from_bytes: true,
        }));
    }
    if let Some(backing) = crate::interpreter::builtin_data_backing(value) {
        if let Some(s) = backing.as_str() {
            return Ok(Some(PyPath {
                path: PathBuf::from(s),
                from_bytes: false,
            }));
        }
        if let ValueKind::Bytes(rc) = backing.kind() {
            return Ok(Some(PyPath {
                path: bytes_to_path_buf(rc.as_slice())?,
                from_bytes: true,
            }));
        }
    }
    Ok(None)
}

impl<'a> FromValue<'a> for PyPath {
    const PY_TYPE_NAME: &'static str = "str, bytes or os.PathLike object";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if let Some(path) = extract_str_or_bytes_path(value)? {
            return Ok(path);
        }
        Err(must_be_error(fn_name, arg_name, Self::PY_TYPE_NAME, value))
    }

    fn try_from_value_with_interp(
        interp: &mut crate::interpreter::Interpreter,
        value: &'a Value,
        fn_name: &str,
        arg_name: &str,
    ) -> Result<Self> {
        if let Some(path) = extract_str_or_bytes_path(value)? {
            return Ok(path);
        }
        if let Some(method) =
            crate::interpreter::lookup_value_special_method(value, "__fspath__").transpose()?
        {
            let resolved =
                crate::interpreter::invoke_class_method(interp, method, value.clone(), &[])?;
            if let Some(path) = extract_str_or_bytes_path(&resolved)? {
                return Ok(path);
            }
            return Err(type_error(format!(
                "expected {}.__fspath__() to return str or bytes, not {}",
                crate::interpreter::value_type_name_str(value),
                crate::interpreter::value_type_name_str(&resolved),
            )));
        }
        // Also support instance-level `__fspath__` attribute (matching `os.fspath`).
        if matches!(value.kind(), ValueKind::PyInstance(_))
            && let Ok(method) = interp.get_attr(value, "__fspath__")
        {
            let resolved = interp.call_function_expanded(method, &[])?;
            if let Some(path) = extract_str_or_bytes_path(&resolved)? {
                return Ok(path);
            }
            return Err(type_error(format!(
                "expected {}.__fspath__() to return str or bytes, not {}",
                crate::interpreter::value_type_name_str(value),
                crate::interpreter::value_type_name_str(&resolved),
            )));
        }
        Err(must_be_error(fn_name, arg_name, Self::PY_TYPE_NAME, value))
    }

    fn matches(value: &'a Value) -> bool {
        if value.is_str() || matches!(value.kind(), ValueKind::Bytes(_)) {
            return true;
        }
        if let ValueKind::PyInstance(inst) = value.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if crate::interpreter::lookup_class_attr(&class, "__fspath__").is_some()
                || inst.borrow().attrs.contains_key("__fspath__")
            {
                return true;
            }
            if let Some(backing) = crate::interpreter::builtin_data_backing(value) {
                return backing.is_str() || matches!(backing.kind(), ValueKind::Bytes(_));
            }
        }
        false
    }
}

// ─── PyIndex ─────────────────────────────────────────────────────────────────

/// Integer argument resolved via Python's `__index__` protocol (`int`, `bool`,
/// `int` subclasses, or objects implementing `__index__`).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PyIndex(PyIndexRepr);

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum PyIndexRepr {
    Small(i64),
    Big(PyBigInt),
}

#[allow(dead_code)]
impl PyIndex {
    pub(crate) fn as_i64(&self) -> Option<i64> {
        match &self.0 {
            PyIndexRepr::Small(n) => Some(*n),
            PyIndexRepr::Big(b) => b.to_i64(),
        }
    }

    pub(crate) fn expect_i64(&self, fn_name: &str, arg_name: &str) -> Result<i64> {
        self.as_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                format!("{fn_name}() argument '{arg_name}' too large to fit in i64"),
            )
        })
    }

    pub(crate) fn as_isize(&self) -> Option<isize> {
        self.as_i64().and_then(|n| isize::try_from(n).ok())
    }

    pub(crate) fn expect_isize(&self, fn_name: &str, arg_name: &str) -> Result<isize> {
        self.as_isize().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                format!("{fn_name}() argument '{arg_name}' too large to fit in isize"),
            )
        })
    }

    pub(crate) fn to_bigint(&self) -> PyBigInt {
        match &self.0 {
            PyIndexRepr::Small(n) => PyBigInt::from(*n),
            PyIndexRepr::Big(b) => b.clone(),
        }
    }
}

impl From<i64> for PyIndex {
    fn from(n: i64) -> Self {
        PyIndex(PyIndexRepr::Small(n))
    }
}

impl From<isize> for PyIndex {
    fn from(n: isize) -> Self {
        PyIndex(PyIndexRepr::Small(n as i64))
    }
}

#[allow(dead_code)]
fn index_from_int_value(value: &Value) -> Option<PyIndex> {
    match value.kind() {
        ValueKind::Int(n) => Some(PyIndex(PyIndexRepr::Small(n))),
        ValueKind::Bool(b) => Some(PyIndex(PyIndexRepr::Small(i64::from(b)))),
        ValueKind::BigInt(b) => Some(PyIndex(PyIndexRepr::Big(b.clone()))),
        _ => None,
    }
}

impl<'a> FromValue<'a> for PyIndex {
    const PY_TYPE_NAME: &'static str = "integer";

    fn try_from_value(value: &'a Value, _fn_name: &str, _arg_name: &str) -> Result<Self> {
        if let Some(idx) = index_from_int_value(value) {
            return Ok(idx);
        }
        if let Some(backing) = crate::interpreter::coerce_subclass_backing(value, &[])
            && let Some(idx) = index_from_int_value(&backing)
        {
            return Ok(idx);
        }
        Err(type_error(format!(
            "'{}' object cannot be interpreted as an integer",
            pyrust_core::builtin_type_name(value),
        )))
    }

    fn try_from_value_with_interp(
        interp: &mut crate::interpreter::Interpreter,
        value: &'a Value,
        _fn_name: &str,
        _arg_name: &str,
    ) -> Result<Self> {
        let Some(resolved) = interp.try_value_to_index(value)? else {
            return Err(type_error(format!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(value),
            )));
        };
        Ok(index_from_int_value(&resolved).expect("try_value_to_index guarantees an integer"))
    }

    fn matches(value: &'a Value) -> bool {
        match value.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => true,
            ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
                if let Some(backing) = crate::interpreter::coerce_subclass_backing(value, &[])
                    && matches!(
                        backing.kind(),
                        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                    )
                {
                    return true;
                }
                crate::interpreter::lookup_value_special_method(value, "__index__")
                    .transpose()
                    .ok()
                    .flatten()
                    .is_some()
            }
            _ => false,
        }
    }
}

// ─── PyNumber ────────────────────────────────────────────────────────────────

/// Real numeric argument coerced to `f64` via `float`, `int`, `bool`,
/// `__float__`, or `__index__` (matching CPython's `PyFloat_AsDouble`).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct PyNumber(pub f64);

#[allow(dead_code)]
impl PyNumber {
    #[inline]
    pub(crate) fn as_f64(&self) -> f64 {
        self.0
    }
}

impl Deref for PyNumber {
    type Target = f64;

    #[inline]
    fn deref(&self) -> &f64 {
        &self.0
    }
}

impl From<f64> for PyNumber {
    fn from(f: f64) -> Self {
        PyNumber(f)
    }
}

impl From<i64> for PyNumber {
    fn from(n: i64) -> Self {
        PyNumber(n as f64)
    }
}

#[allow(dead_code)]
fn primitive_number_to_f64(value: &Value) -> Result<Option<f64>> {
    match value.kind() {
        ValueKind::Float(f) => Ok(Some(f)),
        ValueKind::Int(i) => Ok(Some(i as f64)),
        ValueKind::Bool(b) => Ok(Some(if b { 1.0 } else { 0.0 })),
        ValueKind::BigInt(b) => {
            let f = b.to_f64().unwrap_or(f64::INFINITY);
            if f.is_finite() {
                Ok(Some(f))
            } else {
                Err(PyError::named(
                    "OverflowError",
                    "int too large to convert to float".to_string(),
                ))
            }
        }
        _ => Ok(None),
    }
}

impl<'a> FromValue<'a> for PyNumber {
    const PY_TYPE_NAME: &'static str = "real number";

    fn try_from_value(value: &'a Value, _fn_name: &str, _arg_name: &str) -> Result<Self> {
        if let Some(f) = primitive_number_to_f64(value)? {
            return Ok(PyNumber(f));
        }
        if let Some(backing) = crate::interpreter::coerce_subclass_backing(value, &[])
            && let Some(f) = primitive_number_to_f64(&backing)?
        {
            return Ok(PyNumber(f));
        }
        Err(type_error(format!(
            "must be real number, not {}",
            pyrust_core::builtin_type_name(value),
        )))
    }

    fn try_from_value_with_interp(
        interp: &mut crate::interpreter::Interpreter,
        value: &'a Value,
        _fn_name: &str,
        _arg_name: &str,
    ) -> Result<Self> {
        if let Some(f) = primitive_number_to_f64(value)? {
            return Ok(PyNumber(f));
        }
        if matches!(
            value.kind(),
            ValueKind::PyInstance(_) | ValueKind::PyClass(_)
        ) {
            if let Some(method) =
                crate::interpreter::lookup_value_special_method(value, "__float__").transpose()?
            {
                let res =
                    crate::interpreter::invoke_class_method(interp, method, value.clone(), &[])?;
                if let ValueKind::Float(f) = res.kind() {
                    return Ok(PyNumber(f));
                }
                if let Some(backing) = crate::interpreter::coerce_subclass_backing(&res, &[])
                    && let ValueKind::Float(f) = backing.kind()
                {
                    return Ok(PyNumber(f));
                }
                return Err(type_error(format!(
                    "{}.__float__ returned non-float (type {})",
                    crate::interpreter::value_type_name_str(value),
                    crate::interpreter::value_type_name_str(&res),
                )));
            }
            if let Some(backing) = crate::interpreter::coerce_subclass_backing(value, &[])
                && let Some(f) = primitive_number_to_f64(&backing)?
            {
                return Ok(PyNumber(f));
            }
            if let Some(idx_val) = interp.try_value_to_index(value)?
                && let Some(f) = primitive_number_to_f64(&idx_val)?
            {
                return Ok(PyNumber(f));
            }
        }
        Err(type_error(format!(
            "must be real number, not {}",
            pyrust_core::builtin_type_name(value),
        )))
    }

    fn matches(value: &'a Value) -> bool {
        match value.kind() {
            ValueKind::Float(_) | ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
                true
            }
            ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
                if let Some(backing) = crate::interpreter::coerce_subclass_backing(value, &[])
                    && matches!(
                        backing.kind(),
                        ValueKind::Float(_)
                            | ValueKind::Int(_)
                            | ValueKind::Bool(_)
                            | ValueKind::BigInt(_)
                    )
                {
                    return true;
                }
                crate::interpreter::lookup_value_special_method(value, "__float__")
                    .transpose()
                    .ok()
                    .flatten()
                    .is_some()
                    || crate::interpreter::lookup_value_special_method(value, "__index__")
                        .transpose()
                        .ok()
                        .flatten()
                        .is_some()
            }
            _ => false,
        }
    }
}

// ─── PyCallable ──────────────────────────────────────────────────────────────

/// Validated callable argument (`callable(v) == True`).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct PyCallable(pub Value);

#[allow(dead_code)]
impl PyCallable {
    #[inline]
    pub(crate) fn as_value(&self) -> &Value {
        &self.0
    }

    #[inline]
    pub(crate) fn into_value(self) -> Value {
        self.0
    }
}

impl Deref for PyCallable {
    type Target = Value;

    #[inline]
    fn deref(&self) -> &Value {
        &self.0
    }
}

impl<'a> FromValue<'a> for PyCallable {
    const PY_TYPE_NAME: &'static str = "callable";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PyCallable(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, Self::PY_TYPE_NAME, value))
        }
    }

    fn matches(value: &'a Value) -> bool {
        crate::interpreter::slot_is_callable(value)
    }
}
