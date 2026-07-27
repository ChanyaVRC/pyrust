// ─── PyList / PyTuple / PyDict / PySet ────────────────────────────────────────
//
// These wrap the Value itself so the body can borrow the underlying slice /
// map / set with one method call.  No copy at construction time.
//
// `#[allow(dead_code)]` on the struct + the `as_slice` / `as_map` / `as_set`
// methods: none of the migrated `pyrust_module!` builtins consume these
// shapes yet (the typed-signature dialect — issue #400 — has so far only
// flipped over `PyInt` / `PyBool` / `PyStr` / `PyFloat` / `PyBytes` /
// `PyValue` call sites).  The wrappers stay as ready-to-use infrastructure
// for the remaining #400 migrations; deleting them now would just force
// the next migration to redefine the exact same shapes.

/// `list` argument.  Use [`PyList::as_slice`] to read elements.
#[allow(dead_code)] // #400 typed-signature dialect stub
#[derive(Debug, Clone)]
pub(crate) struct PyList(pub Value);

#[allow(dead_code)] // #400 stub
impl PyList {
    pub(crate) fn as_slice(&self) -> std::cell::Ref<'_, Vec<Value>> {
        // SAFETY (no unsafe used): `try_from_value` verified is_list(),
        // and `Value::clone` preserves that.  `as_list()` is then infallible.
        self.0.as_list().expect("PyList wraps a list")
    }
}

impl<'a> FromValue<'a> for PyList {
    const PY_TYPE_NAME: &'static str = "list";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PyList(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "list", value))
        }
    }

    fn matches(value: &'a Value) -> bool {
        value.is_list()
    }
}

/// `tuple` argument.  Use [`PyTuple::as_slice`].
#[allow(dead_code)] // #400 typed-signature dialect stub
#[derive(Debug, Clone)]
pub(crate) struct PyTuple(pub Value);

#[allow(dead_code)] // #400 stub
impl PyTuple {
    pub(crate) fn as_slice(&self) -> &[Value] {
        self.0.as_tuple().expect("PyTuple wraps a tuple")
    }
}

impl<'a> FromValue<'a> for PyTuple {
    const PY_TYPE_NAME: &'static str = "tuple";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PyTuple(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "tuple", value))
        }
    }

    fn matches(value: &'a Value) -> bool {
        value.is_tuple()
    }
}

/// `dict` argument.  Use [`PyDict::as_map`].
#[allow(dead_code)] // #400 typed-signature dialect stub
#[derive(Debug, Clone)]
pub(crate) struct PyDict(pub Value);

#[allow(dead_code)] // #400 stub
impl PyDict {
    pub(crate) fn as_map(&self) -> std::cell::Ref<'_, pyrust_core::PyDict> {
        self.0.as_dict().expect("PyDict wraps a dict")
    }
}

impl<'a> FromValue<'a> for PyDict {
    const PY_TYPE_NAME: &'static str = "dict";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PyDict(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "dict", value))
        }
    }

    fn matches(value: &'a Value) -> bool {
        matches!(value.kind(), ValueKind::Dict(_))
    }
}

/// `set` argument.  Use [`PySet::as_set`].
#[allow(dead_code)] // #400 typed-signature dialect stub
#[derive(Debug, Clone)]
pub(crate) struct PySet(pub Value);

#[allow(dead_code)] // #400 stub
impl PySet {
    /// Returns the underlying `IndexSet`.  Panics if the wrapper somehow
    /// doesn't wrap a `Set` — impossible by construction (`try_from_value`
    /// checks `is_set` and `Value::clone` preserves the kind), but the
    /// `expect` style matches the sibling wrappers' panic-message wording.
    /// Run `f` against the underlying `IndexSet` view.  Returns the
    /// closure's result.  Post-#450 the `IndexSet` is reached via a
    /// scoped `Ref` borrow from `ValueKind::Set`, so the API now
    /// passes a `&PySet` into the closure rather than
    /// handing one back (which the borrow lifetimes can't express).
    pub(crate) fn as_set<R>(&self, f: impl FnOnce(&pyrust_core::PySet) -> R) -> R {
        match self.0.kind() {
            ValueKind::Set(s) => f(&s),
            _ => panic!("PySet wraps a set"),
        }
    }
}

impl<'a> FromValue<'a> for PySet {
    const PY_TYPE_NAME: &'static str = "set";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PySet(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "set", value))
        }
    }

    fn matches(value: &'a Value) -> bool {
        matches!(value.kind(), ValueKind::Set(_))
    }
}
