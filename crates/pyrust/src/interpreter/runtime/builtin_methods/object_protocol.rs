/// The set of names that CPython's `object` exposes via `dir(object)`.
/// These are included in `dir(instance)` for every user-defined class
/// instance because every class implicitly inherits from `object` (#1225).
static OBJECT_DUNDER_NAMES: &[&str] = &[
    "__class__",
    "__delattr__",
    "__dir__",
    "__doc__",
    "__eq__",
    "__format__",
    "__ge__",
    "__getattribute__",
    "__getstate__",
    "__gt__",
    "__hash__",
    "__init__",
    "__init_subclass__",
    "__le__",
    "__lt__",
    "__ne__",
    "__new__",
    "__reduce__",
    "__reduce_ex__",
    "__repr__",
    "__setattr__",
    "__sizeof__",
    "__str__",
    "__subclasshook__",
];

/// Pre-allocated `Vec<String>` of `OBJECT_DUNDER_NAMES` so that each
/// `dir()` call can clone the cached vec rather than allocating 24
/// `String`s from scratch.
static OBJECT_DUNDER_NAMES_OWNED: std::sync::LazyLock<Vec<String>> =
    std::sync::LazyLock::new(|| {
        OBJECT_DUNDER_NAMES
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    });

/// `true` if `name` is an object-protocol method that every built-in data
/// value exposes (#2151). `__bool__` is only on `None` (other primitives use
/// truthiness via `__len__`/value, not an inherited `object.__bool__`).
///
/// This exact built-in API table belongs beside its dispatch implementation,
/// not in generic attribute lookup.
pub(crate) fn is_object_protocol_method(target: &Value, name: &str) -> bool {
    match name {
        "__sizeof__" | "__dir__" | "__reduce__" | "__reduce_ex__" | "__getstate__" => true,
        "__bool__" => matches!(target.kind(), ValueKind::None),
        _ => false,
    }
}

/// Append the universal `object` dunder names (#2151) to a built-in value's
/// method list, so `dir(x)` advertises exactly the names `hasattr(x, …)` /
/// `getattr(x, …)` resolve through the value's `object`-rooted class MRO.
/// The caller's dedup pass removes any duplicates from type-specific overrides.
fn with_object_dunders(mut names: Vec<String>) -> Vec<String> {
    names.extend_from_slice(&OBJECT_DUNDER_NAMES_OWNED);
    names
}

/// Returns the list of attribute/method names that `dir(obj)` should report.
pub(crate) fn dir_names(value: &Value) -> Vec<String> {
    /// Recursively collect all attribute names from a class and its entire
    /// MRO (primary base then extra_bases, depth-first).
    ///
    /// When the chain terminates (base == None and the class is not the
    /// object singleton itself), append the standard object dunder names
    /// so that inherited names from `object` appear in `dir()` output,
    /// matching CPython's behaviour (#1225).
    fn collect_class_names(class: &Rc<RefCell<PyClass>>, names: &mut Vec<String>) {
        let (own_keys, base, extra_bases): (Vec<String>, _, _) = {
            let borrowed = class.borrow();
            (
                borrowed.attrs.keys().cloned().collect(),
                borrowed.base.clone(),
                borrowed.extra_bases.clone(),
            )
        };
        names.extend(own_keys);
        if let Some(b) = base {
            collect_class_names(&b, names);
        } else {
            // Reached the top of the MRO chain.  Append the names that
            // CPython's `object` exposes; the caller's dedup pass removes
            // any that were already collected from a subclass override.
            // Clone from the pre-allocated static vec to avoid 24 per-call
            // String allocations.
            names.extend_from_slice(&OBJECT_DUNDER_NAMES_OWNED);
        }
        for eb in &extra_bases {
            collect_class_names(eb, names);
        }
    }
    match value.kind() {
        ValueKind::PyInstance(inst) => {
            // `items_snapshot` routes through the live `__dict__` for dict-backed
            // instances (#1981), so `dir()` lists attributes set via the dict.
            let mut names: Vec<String> = inst
                .borrow()
                .attrs
                .items_snapshot()
                .into_iter()
                .map(|(k, _)| k.to_string())
                .collect();
            let class = Rc::clone(&inst.borrow().class);
            collect_class_names(&class, &mut names);
            names
        }
        ValueKind::PyClass(class) => {
            let mut names: Vec<String> = Vec::new();
            collect_class_names(class, &mut names);
            names
        }
        ValueKind::PyModule(module) => {
            let module = module.borrow();
            let is_filesystem = module.filesystem_namespace().is_some();
            let mut names: Vec<String> = module.attrs_snapshot().into_keys().collect();
            drop(module);
            // Append the synthetic dunder attributes that are returned by
            // get_attr for built-in modules. Filesystem modules store their
            // real dunders in the shared namespace and must not resurrect a
            // deleted one here.
            if !is_filesystem {
                for dunder in &[
                    "__name__",
                    "__package__",
                    "__loader__",
                    "__spec__",
                    "__doc__",
                ] {
                    if !names.iter().any(|n| n == dunder) {
                        names.push(dunder.to_string());
                    }
                }
            }
            names
        }
        // Built-in data values (int/str/list/.../None) now chain to `object`
        // in their class MRO (#2151), so `dir(x)` includes the universal object
        // dunders (`__class__`, `__doc__`, `__eq__`, `__sizeof__`, `__dir__`,
        // `__reduce__`, …) alongside the type-specific methods, matching
        // `dir(x)` under CPython.  `with_object_dunders` appends them; the
        // caller's dedup pass removes overlaps.
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => {
            with_object_dunders(builtin_method_names("int"))
        }
        ValueKind::Bytes(_) => with_object_dunders(builtin_method_names("bytes")),
        ValueKind::Str(_) => with_object_dunders(builtin_method_names("str")),
        ValueKind::List(_) => with_object_dunders(builtin_method_names("list")),
        ValueKind::Tuple(_) => with_object_dunders(builtin_method_names("tuple")),
        ValueKind::Dict(_) => with_object_dunders(builtin_method_names("dict")),
        ValueKind::Set(_) => with_object_dunders(builtin_method_names("set")),
        // Issue #2490: `float`/`complex` route through `builtin_method_names`
        // like every other primitive so their slot dunders (`__add__`,
        // `__trunc__`, `__neg__`, …) and instance methods (`conjugate`,
        // `hex`, …) appear in `dir(1.7)` / `dir(1j)` — matching `hasattr`
        // and CPython.
        ValueKind::Float(_) => with_object_dunders(builtin_method_names("float")),
        ValueKind::Complex(_, _) => with_object_dunders(builtin_method_names("complex")),
        ValueKind::None
        | ValueKind::NotImplemented
        | ValueKind::Ellipsis
        | ValueKind::Range { .. } => with_object_dunders(Vec::new()),
        ValueKind::BuiltinObject { ops, .. } => {
            with_object_dunders(builtin_object_method_names(ops))
        }
        ValueKind::Generator(cell) => {
            // CPython exposes a type-specific introspection surface per
            // generator kind (issue #2302): a plain generator advertises the
            // synchronous iteration protocol plus `gi_*`; an async generator
            // advertises the asynchronous protocol plus `ag_*`; a coroutine
            // advertises `send`/`throw`/`close` plus `cr_*` (but NOT
            // `__iter__`/`__next__`).
            // Read from the immutable kind tag: a `dir()` taken from inside
            // the running body must still describe the right surface, and the
            // state cell is checked out for the whole of a resume (#2978).
            let kind = cell.kind();
            let is_async_gen = kind == GeneratorKind::AsyncGenerator;
            let is_coroutine = matches!(
                kind,
                GeneratorKind::Coroutine | GeneratorKind::AsyncGenerator
            );
            let mut names = vec![
                "__class__".to_string(),
                "__name__".to_string(),
                "__qualname__".to_string(),
            ];
            if is_async_gen {
                names.extend(
                    [
                        "__aiter__",
                        "__anext__",
                        "asend",
                        "athrow",
                        "aclose",
                        "ag_code",
                        "ag_frame",
                        "ag_running",
                        "ag_await",
                    ]
                    .iter()
                    .map(|s| s.to_string()),
                );
            } else if is_coroutine {
                names.extend(
                    [
                        "send",
                        "throw",
                        "close",
                        "cr_code",
                        "cr_frame",
                        "cr_running",
                        "cr_await",
                    ]
                    .iter()
                    .map(|s| s.to_string()),
                );
            } else {
                names.extend(
                    [
                        "__iter__",
                        "__next__",
                        "send",
                        "throw",
                        "close",
                        "gi_code",
                        "gi_frame",
                        "gi_running",
                        "gi_yieldfrom",
                    ]
                    .iter()
                    .map(|s| s.to_string()),
                );
                // Only the concrete built-in iterators carry a remaining-count
                // slot (issue #2920); a real generator does not.
                if cell
                    .try_borrow()
                    .is_ok_and(|state| builtin_iterator_has_length_hint(&**state))
                {
                    names.push("__length_hint__".to_string());
                }
            }
            names
        }
        _ => Vec::new(),
    }
}

/// Public method names per built-in type for `dir()`.
///
/// Derives the list from the provider's complete primitive-class metadata, so
/// instance/class/static methods, constructor sentinels, class subscripting,
/// and owned slots share their inventory with class bootstrap.  The remaining
/// protocol dunders CPython exposes (`__len__`, `__getitem__`, `__contains__`,
/// `__add__`, …) come from `builtin_protocol_dunders` (issue #1909).
fn builtin_method_names(type_name: &str) -> Vec<String> {
    let attrs = pyrust_builtins::primitive_class_attrs::lookup(type_name);
    let mut out: Vec<String> = match attrs {
        Some(attrs) => attrs.iter().map(|attr| attr.name.to_string()).collect(),
        None => match type_name {
            // `slice` is represented by a separate BuiltinObject class rather
            // than one of the primitive class singletons.
            "slice" => pyrust_builtins::slice::METHODS
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            _ => Vec::new(),
        },
    };
    for d in protocol_dunder_names(type_name) {
        out.push(d.to_string());
    }
    // Issue #2490: float/complex expose `real`/`imag` read-only properties
    // (intercepted in `get_attr`). Class/static methods already came from the
    // provider metadata above. CPython also advertises
    // `__int__`/`__float__`/`__getformat__` (float) and `__complex__`
    // (complex), but those slots are not yet resolvable on pyrust instances —
    // omitting them keeps `dir()` in lock-step with `hasattr`.
    if type_name == "float" {
        for n in ["real", "imag"] {
            out.push(n.to_string());
        }
    }
    if type_name == "complex" {
        for n in ["real", "imag"] {
            out.push(n.to_string());
        }
    }
    out
}

/// Public method names for the canonical families represented through
/// `BuiltinObject`.
///
/// This adapter deliberately classifies the operations table by concrete
/// identity/tag rather than decoding `BuiltinTypeOps::type_name()`. The
/// strings passed to `builtin_method_names` below are Python API metadata
/// owned by this builtin-method domain, not runtime RTTI.
fn builtin_object_method_names(ops: &dyn pyrust_core::BuiltinTypeOps) -> Vec<String> {
    match ops.canonical_class_tag() {
        Some(pyrust_core::CanonicalClassTag::Bytearray) => builtin_method_names("bytearray"),
        Some(pyrust_core::CanonicalClassTag::Dict) => builtin_method_names("dict"),
        Some(pyrust_core::CanonicalClassTag::Frozenset) => builtin_method_names("frozenset"),
        _ if pyrust_builtins::slice::is_slice_ops(ops) => builtin_method_names("slice"),
        _ => Vec::new(),
    }
}
