/// Returns `true` when `name` is a sequence, mapping, or container protocol
/// dunder managed by [`builtin_protocol_dunders`].
///
/// This is the shared routing gate for built-in containers: supported names
/// enter the protocol dispatcher, while names that belong to another built-in
/// type raise `AttributeError`. Object-level dunders such as `__repr__` and
/// `__eq__` remain on their own dispatch path.
fn is_container_protocol_dunder_name(name: &str) -> bool {
    matches!(
        name,
        "__len__"
            | "__getitem__"
            | "__setitem__"
            | "__delitem__"
            | "__contains__"
            | "__add__"
            | "__mul__"
            // In-place sequence dunders (#2119): list/bytearray.
            | "__iadd__"
            | "__imul__"
            // set/frozenset/dict algebra & merge dunders (#2122), including
            // reflected (`__rOP__`) and in-place (`__iOP__`) forms.
            | "__or__"
            | "__ror__"
            | "__ior__"
            | "__and__"
            | "__rand__"
            | "__iand__"
            | "__sub__"
            | "__rsub__"
            | "__isub__"
            | "__xor__"
            | "__rxor__"
            | "__ixor__"
            // Rich-comparison / `__hash__` / `__str__` / `__repr__` exposed on
            // every primitive type (issue #2070).  Routed through the protocol
            // dispatcher so `[1].__lt__([2])`, `'a'.__eq__('a')`,
            // `{1}.__hash__()` … work via the method-call opcode.  The
            // per-receiver `builtin_protocol_dunders` membership check at the
            // call site still raises `AttributeError` for a name a given type
            // does not expose.
            | "__eq__"
            | "__ne__"
            | "__lt__"
            | "__le__"
            | "__gt__"
            | "__ge__"
            | "__hash__"
            | "__str__"
            | "__repr__"
            // `dict.__reversed__()` — reversible by insertion order (#2093);
            // the per-receiver membership check restricts it to `dict`/`list`.
            | "__reversed__"
            // Issue #2387: `__iter__` (every iterable) and `__mod__`/`__rmod__`
            // (the `%` slots on str/bytes/bytearray), routed through the protocol
            // dispatcher when called via the method-call opcode.  The
            // per-receiver `builtin_protocol_dunders` membership check restricts
            // each to the types that actually expose it.
            | "__iter__"
            | "__mod__"
            | "__rmod__"
    )
}

/// Return the interned `"<type>.<cmp_dunder>"` sentinel for a primitive's
/// rich-comparison slot. A literal match keeps the result `'static` without a
/// per-access allocation.
fn primitive_cmp_dunder_sentinel(kind: PrimitiveClassKind, dunder: &str) -> Option<&'static str> {
    macro_rules! arms {
        ($(($kind:path, $type_name:literal)),+ $(,)?) => {
            match (kind, dunder) {
                $(
                    ($kind, "__eq__") => Some(concat!($type_name, ".__eq__")),
                    ($kind, "__ne__") => Some(concat!($type_name, ".__ne__")),
                    ($kind, "__lt__") => Some(concat!($type_name, ".__lt__")),
                    ($kind, "__le__") => Some(concat!($type_name, ".__le__")),
                    ($kind, "__gt__") => Some(concat!($type_name, ".__gt__")),
                    ($kind, "__ge__") => Some(concat!($type_name, ".__ge__")),
                )+
                _ => None,
            }
        };
    }
    arms!(
        (PrimitiveClassKind::Int, "int"),
        (PrimitiveClassKind::Float, "float"),
        (PrimitiveClassKind::Complex, "complex"),
        (PrimitiveClassKind::Str, "str"),
        (PrimitiveClassKind::Bytes, "bytes"),
        (PrimitiveClassKind::Bytearray, "bytearray"),
        (PrimitiveClassKind::Tuple, "tuple"),
        (PrimitiveClassKind::Frozenset, "frozenset"),
        (PrimitiveClassKind::List, "list"),
        (PrimitiveClassKind::Dict, "dict"),
        (PrimitiveClassKind::Set, "set"),
    )
}

/// Exact primitive-slot ownership table.
///
/// This concrete Python API policy belongs to `builtin_methods`; generic
/// attribute lookup only consumes the typed result. `kind` comes from
/// singleton identity rather than mutable `PyClass::name`, so a user class
/// named `list` cannot acquire list's slots and an internally renamed builtin
/// keeps its canonical owner.
fn primitive_owned_dunder_sentinel(kind: PrimitiveClassKind, dunder: &str) -> Option<&'static str> {
    if matches!(
        dunder,
        "__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__"
    ) {
        // bool inherits all six comparisons from int; every other supported
        // scalar/container primitive owns them in CPython 3.12.
        return primitive_cmp_dunder_sentinel(kind, dunder);
    }

    Some(match (kind, dunder) {
        (PrimitiveClassKind::Bool, "__repr__") => "bool.__repr__",
        (PrimitiveClassKind::Int, "__hash__") => "int.__hash__",
        (PrimitiveClassKind::Int, "__repr__") => "int.__repr__",
        (PrimitiveClassKind::Int, "__format__") => "int.__format__",
        (PrimitiveClassKind::Float, "__hash__") => "float.__hash__",
        (PrimitiveClassKind::Float, "__repr__") => "float.__repr__",
        (PrimitiveClassKind::Float, "__format__") => "float.__format__",
        (PrimitiveClassKind::Complex, "__hash__") => "complex.__hash__",
        (PrimitiveClassKind::Complex, "__repr__") => "complex.__repr__",
        (PrimitiveClassKind::Complex, "__format__") => "complex.__format__",
        (PrimitiveClassKind::Str, "__hash__") => "str.__hash__",
        (PrimitiveClassKind::Str, "__repr__") => "str.__repr__",
        (PrimitiveClassKind::Str, "__str__") => "str.__str__",
        (PrimitiveClassKind::Str, "__format__") => "str.__format__",
        (PrimitiveClassKind::Bytes, "__hash__") => "bytes.__hash__",
        (PrimitiveClassKind::Bytes, "__repr__") => "bytes.__repr__",
        (PrimitiveClassKind::Bytes, "__str__") => "bytes.__str__",
        (PrimitiveClassKind::Bytearray, "__repr__") => "bytearray.__repr__",
        (PrimitiveClassKind::Bytearray, "__str__") => "bytearray.__str__",
        (PrimitiveClassKind::Tuple, "__hash__") => "tuple.__hash__",
        (PrimitiveClassKind::Tuple, "__repr__") => "tuple.__repr__",
        (PrimitiveClassKind::Frozenset, "__hash__") => "frozenset.__hash__",
        (PrimitiveClassKind::Frozenset, "__repr__") => "frozenset.__repr__",
        (PrimitiveClassKind::List, "__repr__") => "list.__repr__",
        (PrimitiveClassKind::Dict, "__repr__") => "dict.__repr__",
        (PrimitiveClassKind::Set, "__repr__") => "set.__repr__",
        _ => return None,
    })
}

/// Resolve the primitive-owned descriptor sentinel for `class`.
///
/// C3 order preserves normal builtin subclass inheritance (`class L(list)`),
/// while [`primitive_class_kind`] admits only the genuine singleton base. The
/// first primitive that actually owns `dunder` wins, which is significant for
/// bool: it owns `__repr__`, but inherits comparisons/hash/format from int.
pub(crate) fn primitive_owned_object_dunder(
    class: &Rc<RefCell<PyClass>>,
    dunder: &str,
) -> Option<&'static str> {
    c3_linearize_classes(class)
        .into_iter()
        .filter_map(|candidate| primitive_class_kind(&candidate))
        .find_map(|kind| primitive_owned_dunder_sentinel(kind, dunder))
}

/// `Some((type_name, dunder))` when `qualified` is one of the type-qualified
/// object-level dunder sentinels synthesised above (`"str.__hash__"`,
/// `"complex.__format__"`, …). Rich comparisons are dispatched by the generic
/// protocol arm and therefore deliberately excluded here.
pub(super) fn primitive_object_dunder_owner(qualified: &str) -> Option<(&str, &str)> {
    let (type_name, dunder) = qualified.split_once('.')?;
    if !matches!(dunder, "__hash__" | "__repr__" | "__str__" | "__format__") {
        return None;
    }
    let kind = PrimitiveClassKind::from_primitive_name(type_name)?;
    primitive_owned_dunder_sentinel(kind, dunder).map(|_| (type_name, dunder))
}

/// Numeric-tower rank used by the scalar forward dunders (#2070):
/// `int`/`bool` = 0, `float` = 1, `complex` = 2.  Returns `None` for
/// non-numeric values.  A forward slot on a receiver of rank R accepts an
/// operand of rank ≤ R (`float.__add__(int)` works, `int.__add__(float)` →
/// `NotImplemented`), mirroring CPython's `nb_*` slot coercion direction.
fn numeric_tower_rank(v: &Value) -> Option<u8> {
    match v.kind() {
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => Some(0),
        ValueKind::Float(_) => Some(1),
        ValueKind::Complex(..) => Some(2),
        _ => None,
    }
}

/// Whether a scalar numeric/bitwise forward dunder on `recv` accepts `operand`
/// (else the slot returns `NotImplemented`).  See [`numeric_tower_rank`].
fn numeric_operand_accepted(recv: &Value, operand: &Value) -> bool {
    match (numeric_tower_rank(recv), numeric_tower_rank(operand)) {
        (Some(r), Some(o)) => o <= r,
        _ => false,
    }
}

/// Whether a rich-comparison forward dunder on `recv` accepts `operand` (else
/// it returns `NotImplemented`).  `is_equality` distinguishes `__eq__`/`__ne__`
/// (defined for `complex` and `dict`) from the ordering slots (which `complex`
/// and `dict` do not define).  Numeric receivers follow the same tower-rank
/// acceptance as arithmetic; non-numeric receivers accept only operands of a
/// compatible type group (str/str, bytes/bytes, list/list, tuple/tuple,
/// dict/dict, and set↔frozenset interchangeably).
fn richcmp_operand_accepted(recv: &Value, operand: &Value, is_equality: bool) -> bool {
    // Numeric receivers: ordering is undefined for `complex`, so a complex
    // receiver does not accept an ordering comparison from any operand.
    if let Some(r) = numeric_tower_rank(recv) {
        if !is_equality && r == 2 {
            return false;
        }
        return numeric_operand_accepted(recv, operand);
    }
    let rt = pyrust_core::builtin_type_name(recv);
    let ot = pyrust_core::builtin_type_name(operand);
    match &*rt {
        // dict has equality but no ordering.
        "dict" => is_equality && ot == "dict",
        // set and frozenset are interchangeable for both equality and the
        // subset/superset ordering comparisons.
        "set" | "frozenset" => matches!(&*ot, "set" | "frozenset"),
        // Every other primitive sequence/string compares only with its own
        // exact type (`'a'.__eq__(b'a')` → NotImplemented).
        _ => rt == ot,
    }
}

/// Issue #2291 / #2398: whether a container protocol dunder is a *named*
/// method-wrapper in CPython 3.12 (its error messages read `{type}.{method}()`)
/// versus an anonymous slot wrapper (whose messages read `wrapper {method}()`).
/// The named set is exactly the slots CPython implements as `method_descriptor`
/// rather than `wrapper_descriptor`: `mp_subscript` (`list`/`dict`
/// `__getitem__`), `sq_contains` (`dict`/`set`/`frozenset` `__contains__`), and
/// `__reversed__` (`list`/`dict`).  Every other protocol dunder is an anonymous
/// slot wrapper.  This is the same partition that drives the unbound
/// `wrapper_descriptor` vs `method_descriptor` `repr`/type-name in
/// the typed callable-presentation provider. Verified against `python3.12`.
pub(super) fn is_named_protocol_wrapper(method: &str, type_name: &str) -> bool {
    matches!(
        (method, type_name),
        ("__getitem__", "list" | "dict")
            | ("__contains__", "dict" | "set" | "frozenset")
            | ("__reversed__", "list" | "dict")
            // Issue #2297/#2481: `int`/`float`.`__round__`/`__trunc__`/`__floor__`/
            // `__ceil__` are `method_descriptor`s (named keyword-rejection
            // wording).  `int.__index__` stays an anonymous slot wrapper.
            | ("__round__" | "__trunc__" | "__floor__" | "__ceil__", "int" | "bool" | "float")
    )
}

pub(super) const SLOT_ATTR: u8 = 1;
const SLOT_PROTOCOL: u8 = 2;

/// Per-type slot-dunder table — the SINGLE source for both the protocol
/// dispatch check (`builtin_protocol_dunders`) and the primitive-singleton
/// type-attr registration consumed by primitive-class bootstrap (issue #2406:
/// the previous pair of comment-synced hand lists is the same drift pattern
/// that caused #2324).
///
/// Flags: `SLOT_PROTOCOL` = dispatchable through
/// `dispatch_builtin_protocol_dunder` (rich comparisons / `__hash__` /
/// `__str__` / `__repr__` are exposed as bound method-wrappers per #2070, the
/// container/numeric slots per #1909/#2215/#2387).  `SLOT_ATTR` = registered
/// as a type-level attribute on the primitive class singleton (drives
/// `list.__iter__`, `hasattr`, `dir`, and subclass MRO resolution of the
/// unbound form; names without it resolve through other paths).  A few
/// entries are attr-only (`float.__trunc__`/`__floor__`/`__ceil__` have
/// registry bodies, not protocol dispatch).
///
/// list / dict / set / bytearray are unhashable, so `__hash__` is *not*
/// listed (CPython sets `list.__hash__ = None`; the None-attr path handles
/// `[1].__hash__()`).
pub(super) fn slot_dunder_table(type_name: &str) -> &'static [(&'static str, u8)] {
    const PA: u8 = SLOT_ATTR | SLOT_PROTOCOL;
    const P: u8 = SLOT_PROTOCOL;
    match type_name {
        "list" => &[
            ("__len__", PA),
            ("__getitem__", PA),
            ("__setitem__", PA),
            ("__delitem__", PA),
            ("__contains__", PA),
            ("__add__", PA),
            ("__mul__", PA),
            ("__iadd__", PA),
            ("__imul__", PA),
            ("__iter__", PA),
            ("__reversed__", PA),
            ("__eq__", P),
            ("__ne__", P),
            ("__lt__", P),
            ("__le__", P),
            ("__gt__", P),
            ("__ge__", P),
            ("__str__", P),
            ("__repr__", P),
        ],
        "str" => &[
            ("__len__", PA),
            ("__getitem__", PA),
            ("__contains__", PA),
            ("__add__", PA),
            ("__mul__", PA),
            ("__mod__", PA),
            ("__rmod__", PA),
            ("__iter__", PA),
            ("__eq__", PA),
            ("__ne__", PA),
            ("__lt__", PA),
            ("__le__", PA),
            ("__gt__", PA),
            ("__ge__", PA),
            ("__hash__", P),
            ("__str__", P),
            ("__repr__", P),
        ],
        "bytes" => &[
            ("__len__", PA),
            ("__getitem__", PA),
            ("__contains__", PA),
            ("__add__", PA),
            ("__mul__", PA),
            ("__mod__", PA),
            ("__rmod__", PA),
            ("__iter__", PA),
            ("__eq__", P),
            ("__ne__", P),
            ("__lt__", P),
            ("__le__", P),
            ("__gt__", P),
            ("__ge__", P),
            ("__hash__", P),
            ("__str__", P),
            ("__repr__", P),
        ],
        "tuple" => &[
            ("__len__", PA),
            ("__getitem__", PA),
            ("__contains__", PA),
            ("__add__", PA),
            ("__mul__", PA),
            ("__iter__", PA),
            ("__eq__", P),
            ("__ne__", P),
            ("__lt__", P),
            ("__le__", P),
            ("__gt__", P),
            ("__ge__", P),
            ("__hash__", P),
            ("__str__", P),
            ("__repr__", P),
        ],
        "bytearray" => &[
            ("__len__", PA),
            ("__getitem__", PA),
            ("__setitem__", PA),
            ("__delitem__", PA),
            ("__contains__", PA),
            ("__add__", PA),
            ("__mul__", PA),
            ("__iadd__", PA),
            ("__imul__", PA),
            ("__mod__", PA),
            ("__rmod__", PA),
            ("__iter__", PA),
            ("__eq__", P),
            ("__ne__", P),
            ("__lt__", P),
            ("__le__", P),
            ("__gt__", P),
            ("__ge__", P),
            ("__str__", P),
            ("__repr__", P),
        ],
        "dict" => &[
            ("__len__", PA),
            ("__getitem__", PA),
            ("__setitem__", PA),
            ("__delitem__", PA),
            ("__contains__", PA),
            ("__or__", PA),
            ("__ror__", PA),
            ("__ior__", PA),
            ("__iter__", PA),
            ("__reversed__", PA),
            ("__eq__", P),
            ("__ne__", P),
            ("__lt__", P),
            ("__le__", P),
            ("__gt__", P),
            ("__ge__", P),
            ("__str__", P),
            ("__repr__", P),
        ],
        "set" => &[
            ("__len__", PA),
            ("__contains__", PA),
            ("__or__", PA),
            ("__ror__", PA),
            ("__and__", PA),
            ("__rand__", PA),
            ("__sub__", PA),
            ("__rsub__", PA),
            ("__xor__", PA),
            ("__rxor__", PA),
            ("__ior__", PA),
            ("__iand__", PA),
            ("__isub__", PA),
            ("__ixor__", PA),
            ("__iter__", PA),
            ("__eq__", P),
            ("__ne__", P),
            ("__lt__", P),
            ("__le__", P),
            ("__gt__", P),
            ("__ge__", P),
            ("__str__", P),
            ("__repr__", P),
        ],
        "frozenset" => &[
            ("__len__", PA),
            ("__contains__", PA),
            ("__or__", PA),
            ("__ror__", PA),
            ("__and__", PA),
            ("__rand__", PA),
            ("__sub__", PA),
            ("__rsub__", PA),
            ("__xor__", PA),
            ("__rxor__", PA),
            ("__iter__", PA),
            ("__eq__", P),
            ("__ne__", P),
            ("__lt__", P),
            ("__le__", P),
            ("__gt__", P),
            ("__ge__", P),
            ("__hash__", P),
            ("__str__", P),
            ("__repr__", P),
        ],
        "dict_keys" | "dict_items" => &[
            ("__getattribute__", PA),
            ("__len__", PA),
            ("__contains__", PA),
            ("__or__", PA),
            ("__ror__", PA),
            ("__and__", PA),
            ("__rand__", PA),
            ("__sub__", PA),
            ("__rsub__", PA),
            ("__xor__", PA),
            ("__rxor__", PA),
            ("__iter__", PA),
            ("__reversed__", PA),
            ("__eq__", PA),
            ("__ne__", PA),
            ("__lt__", PA),
            ("__le__", PA),
            ("__gt__", PA),
            ("__ge__", PA),
            ("__repr__", PA),
            ("__str__", P),
        ],
        "dict_values" => &[
            ("__getattribute__", PA),
            ("__iter__", PA),
            ("__reversed__", PA),
            ("__len__", PA),
            // Values views inherit comparisons from object rather than owning
            // them, but the bound wrappers must remain directly callable.
            ("__eq__", P),
            ("__ne__", P),
            ("__lt__", P),
            ("__le__", P),
            ("__gt__", P),
            ("__ge__", P),
            ("__hash__", P),
            ("__repr__", PA),
            ("__str__", P),
        ],
        "odict_keys" | "odict_items" | "odict_values" => &[("__iter__", PA), ("__reversed__", PA)],
        "int" | "bool" => &[
            ("__add__", PA),
            ("__sub__", PA),
            ("__mul__", PA),
            ("__truediv__", PA),
            ("__floordiv__", PA),
            ("__mod__", PA),
            ("__pow__", PA),
            ("__and__", PA),
            ("__or__", PA),
            ("__xor__", PA),
            ("__lshift__", PA),
            ("__rshift__", PA),
            ("__eq__", PA),
            ("__ne__", PA),
            ("__lt__", PA),
            ("__le__", PA),
            ("__gt__", PA),
            ("__ge__", PA),
            ("__hash__", P),
            ("__str__", P),
            ("__repr__", P),
            // Issue #2433: `int.__bool__`/`__float__`/`__int__` are int-owned slot
            // wrappers in CPython (`<slot wrapper '__bool__' of 'int' objects>`);
            // expose them unbound so `int.__bool__`/`(5).__float__()` resolve.
            ("__bool__", PA),
            ("__float__", PA),
            ("__int__", PA),
            ("__divmod__", P),
            ("__neg__", P),
            ("__pos__", P),
            ("__abs__", P),
            ("__invert__", P),
            // Issue #2297: `int.__round__`/`__index__`/`__trunc__`/`__floor__`/
            // `__ceil__` are int-owned descriptors (`__index__` a slot wrapper,
            // the rest method_descriptors) — exposed unbound so `int.__round__`
            // resolves, and dispatchable bound so `(5).__round__()`/
            // `(5).__floor__()` compute.
            ("__round__", PA),
            ("__index__", PA),
            ("__trunc__", PA),
            ("__floor__", PA),
            ("__ceil__", PA),
            ("__radd__", P),
            ("__rsub__", P),
            ("__rmul__", P),
            ("__rtruediv__", P),
            ("__rfloordiv__", P),
            ("__rmod__", P),
            ("__rpow__", P),
            ("__rdivmod__", P),
            ("__rand__", P),
            ("__ror__", P),
            ("__rxor__", P),
            ("__rlshift__", P),
            ("__rrshift__", P),
        ],
        "float" => &[
            ("__eq__", P),
            ("__ne__", P),
            ("__lt__", P),
            ("__le__", P),
            ("__gt__", P),
            ("__ge__", P),
            ("__hash__", P),
            ("__str__", P),
            ("__repr__", P),
            ("__bool__", P),
            ("__add__", P),
            ("__sub__", P),
            ("__mul__", P),
            ("__truediv__", P),
            ("__floordiv__", P),
            ("__mod__", P),
            ("__pow__", P),
            ("__divmod__", P),
            ("__neg__", P),
            ("__pos__", P),
            ("__abs__", P),
            ("__radd__", P),
            ("__rsub__", P),
            ("__rmul__", P),
            ("__rtruediv__", P),
            ("__rfloordiv__", P),
            ("__rmod__", P),
            ("__rpow__", P),
            ("__rdivmod__", P),
            // Issue #2481: `float.__round__`/`__trunc__`/`__floor__`/`__ceil__`
            // are float-owned method_descriptors — exposed unbound so
            // `float.__trunc__` resolves, and dispatchable bound so
            // `(1.7).__trunc__()`/`(1.7).__round__()` compute.
            ("__round__", PA),
            ("__trunc__", PA),
            ("__floor__", PA),
            ("__ceil__", PA),
        ],
        // Issue #2536: every dunder CPython 3.12 carries in `complex.__dict__`
        // is a complex-owned slot wrapper, so expose them unbound (`PA`) — the
        // same treatment `int` (#2480) and `float` (#2488) received.  Owned set
        // (`[c for c in complex.__mro__ if d in c.__dict__][0] is complex`):
        // the six rich-comparison slots (present in `__dict__` even though
        // `<`/`<=`/… raise TypeError at call time), `__hash__`/`__repr__`/
        // `__bool__`, the forward + reflected arithmetic slots, and
        // `__neg__`/`__pos__`/`__abs__`.  `__str__` is inherited from `object`
        // (not in `complex.__dict__`), so it stays `P` (dispatchable bound but
        // no complex-owned type attr — `complex.__str__` must resolve to
        // `<slot wrapper '__str__' of 'object' objects>`).
        "complex" => &[
            ("__eq__", PA),
            ("__ne__", PA),
            ("__lt__", PA),
            ("__le__", PA),
            ("__gt__", PA),
            ("__ge__", PA),
            ("__hash__", PA),
            ("__repr__", PA),
            ("__bool__", PA),
            ("__add__", PA),
            ("__sub__", PA),
            ("__mul__", PA),
            ("__truediv__", PA),
            ("__pow__", PA),
            ("__neg__", PA),
            ("__pos__", PA),
            ("__abs__", PA),
            ("__radd__", PA),
            ("__rsub__", PA),
            ("__rmul__", PA),
            ("__rtruediv__", PA),
            ("__rpow__", PA),
            ("__str__", P),
        ],
        // Issue #2399: `range` is a `ValueKind::Range`/`BigRange`, not a
        // primitive `PyClass` populated by `build_primitive_classes`, so its
        // slot dunders were neither dispatchable as method-wrappers nor exposed
        // as attributes.  The protocol dunders route through the shared
        // dispatcher exactly as the other sequences do (`__iter__`/`__reversed__`
        // → the `iter`/`reversed` builtins; `__len__`/`__getitem__`/`__contains__`
        // → `len`/`eval_index`/`eval_in`; `__eq__`/`__ne__`/`__hash__`/`__bool__`/
        // `__repr__` → the shared scalar/object arms).  The SLOT_ATTR entries are
        // registered onto the `range` class singleton in helpers.rs's RANGE_CLASS
        // initialiser (not the `build_primitive_classes` loop, which range is not
        // part of).  The SLOT_ATTR (`PA`) names are exactly the slots `range`
        // *owns* in CPython (`[c for c in range.__mro__ if '<dunder>' in
        // c.__dict__][0] is range`): range overrides
        // `__eq__`/`__ne__`/`__hash__`/`__repr__`.  `__str__` is `P`
        // (protocol-only, like list/tuple): range inherits `object.__str__`, so
        // it must stay *dispatchable* (`range(3).__str__()` → `range(0, 3)`) but
        // must NOT register a range-owned type attr (so `range.__str__` resolves
        // to the inherited `<slot wrapper '__str__' of 'object' objects>`).
        // range is hashable and defines equality but NOT ordering (`<`/`<=`/…
        // inherit object's identity slots), so the ordering dunders are omitted.
        "range" => &[
            ("__iter__", PA),
            ("__reversed__", PA),
            ("__len__", PA),
            ("__getitem__", PA),
            ("__contains__", PA),
            ("__eq__", PA),
            ("__ne__", PA),
            ("__hash__", PA),
            ("__bool__", PA),
            ("__repr__", PA),
            ("__str__", P),
        ],
        _ => &[],
    }
}

/// Membership test for `SLOT_PROTOCOL` names.  A direct scan of the static
/// flagged table: same rodata locality as the historical hand-written
/// slices — an OnceLock/Box variant of this check cost ~8% on `s.upper()`
/// loops because it sits on the plain builtin-method dispatch path.
#[inline]
pub(super) fn is_protocol_dunder(type_name: &str, method: &str) -> bool {
    slot_dunder_table(type_name)
        .iter()
        .any(|&(n, f)| f & SLOT_PROTOCOL != 0 && n == method)
}

/// Iterate the `SLOT_PROTOCOL` names of a type (the `dir()` merge consumer).
fn protocol_dunder_names(type_name: &str) -> impl Iterator<Item = &'static str> {
    slot_dunder_table(type_name)
        .iter()
        .filter(|&&(_, f)| f & SLOT_PROTOCOL != 0)
        .map(|&(n, _)| n)
}
