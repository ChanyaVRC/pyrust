"""
Data Classes (pyrust port of a minimal subset of CPython 3.12's
``dataclasses`` module).

Provides the ``@dataclass`` decorator and the ``field`` / ``fields`` /
``asdict`` / ``astuple`` / ``replace`` / ``is_dataclass`` helpers.  The
decorator reads the class ``__annotations__`` to discover fields (in order,
honouring inheritance), then generates ``__init__``, ``__repr__``, ``__eq__``
and — when ``frozen=True`` — ``__setattr__`` / ``__delattr__`` that raise
``FrozenInstanceError``.

This source is exec'd once into a throwaway namespace at first
``import dataclasses``; the public names are copied onto the module by
``dataclasses.rs::inject_python_members`` (mirrors ``operator`` / ``string``).

Reference: <https://docs.python.org/3/library/dataclasses.html>
"""


# Sentinel for "no default supplied" — distinct from None, which is a valid
# default value.
class _MISSING_TYPE:
    def __repr__(self):
        return "MISSING"


MISSING = _MISSING_TYPE()


class FrozenInstanceError(AttributeError):
    """Raised when assigning to a field of a frozen dataclass instance."""


class Field:
    """Describes a single dataclass field.

    Instances are created by the module-level ``field()`` function and, for
    plain annotated attributes, synthesised by the ``@dataclass`` decorator.
    """
    __slots__ = ("name", "type", "default", "default_factory", "init", "repr",
                 "compare", "hash")

    def __init__(self, default, default_factory, init, repr, compare, hash):
        self.name = None
        self.type = None
        self.default = default
        self.default_factory = default_factory
        self.init = init
        self.repr = repr
        self.compare = compare
        self.hash = hash

    def __repr__(self):
        return ("Field(name=%r,type=%r,default=%r,default_factory=%r,"
                "init=%r,repr=%r,compare=%r,hash=%r)" % (
                    self.name, self.type, self.default, self.default_factory,
                    self.init, self.repr, self.compare, self.hash))


def field(*, default=MISSING, default_factory=MISSING, init=True, repr=True,
          compare=True, hash=None):
    """Return an object to identify dataclass fields.

    ``default`` and ``default_factory`` are mutually exclusive.  ``hash``
    defaults to ``None``, meaning "use the value of ``compare``"; set it
    explicitly to include/exclude a field from the generated ``__hash__``
    independently of ``__eq__`` (mirrors CPython's ``field(hash=...)``).
    """
    if default is not MISSING and default_factory is not MISSING:
        raise ValueError("cannot specify both default and default_factory")
    return Field(default, default_factory, init, repr, compare, hash)


# Name under which the per-class tuple of Field objects is stashed.
_FIELDS = "__dataclass_fields__"


def _collect_fields(cls):
    """Build the ordered list of Field objects for ``cls``.

    Walks base classes first (reverse MRO, excluding ``object`` and ``cls``)
    so inherited fields precede this class's own, matching CPython's ordering.
    Later definitions override earlier ones by name.
    """
    fields = {}
    # Inherited fields, base-first.
    for base in cls.__mro__[-1:0:-1]:
        base_fields = getattr(base, _FIELDS, None)
        if base_fields:
            for f in base_fields:
                fields[f.name] = f
    # This class's own annotations.
    cls_annotations = cls.__dict__.get("__annotations__", {})
    for name, atype in cls_annotations.items():
        default = getattr(cls, name, MISSING)
        if isinstance(default, Field):
            f = default
        else:
            f = Field(default, MISSING, True, True, True, None)
        f.name = name
        f.type = atype
        fields[name] = f
    return list(fields.values())


def _set_new_attribute(cls, name, value):
    # Only set if not already defined directly on the class body.
    if name in cls.__dict__:
        return True
    setattr(cls, name, value)
    return False


def _process_class(cls, init, repr, eq, frozen, unsafe_hash):
    flds = _collect_fields(cls)
    setattr(cls, _FIELDS, flds)

    if init:
        _set_new_attribute(cls, "__init__", _make_init(flds, frozen))
    if repr:
        _set_new_attribute(cls, "__repr__", _make_repr(cls, flds))
    if eq:
        _set_new_attribute(cls, "__eq__", _make_eq(cls, flds))
    if frozen:
        # Always install the frozen guards (overriding any inherited setters).
        setattr(cls, "__setattr__", _frozen_setattr)
        setattr(cls, "__delattr__", _frozen_delattr)

    _set_hash(cls, flds, eq, frozen, unsafe_hash)

    return cls


# Decisions for the ``__hash__`` slot, keyed by
# ``(unsafe_hash, eq, frozen, has_explicit_hash)``.  Mirrors CPython 3.12's
# ``dataclasses._hash_action`` table exactly; the value is one of:
#   "add"   — generate a value-based ``__hash__`` over the hash fields,
#   "none"  — set ``__hash__ = None`` (unhashable),
#   "raise" — raise ``TypeError`` (can't overwrite an explicit ``__hash__``),
#   None    — do nothing (keep whatever ``__hash__`` is inherited/defined).
_HASH_ACTION = {
    (False, False, False, False): None,
    (False, False, False, True): None,
    (False, False, True, False): None,
    (False, False, True, True): None,
    (False, True, False, False): "none",
    (False, True, False, True): None,
    (False, True, True, False): "add",
    (False, True, True, True): None,
    (True, False, False, False): "add",
    (True, False, False, True): "raise",
    (True, False, True, False): "add",
    (True, False, True, True): "raise",
    (True, True, False, False): "add",
    (True, True, False, True): "raise",
    (True, True, True, False): "add",
    (True, True, True, True): "raise",
}


def _set_hash(cls, flds, eq, frozen, unsafe_hash):
    """Install ``__hash__`` following CPython 3.12's ``_hash_action`` table.

    The lookup is keyed on ``(unsafe_hash, eq, frozen, has_explicit_hash)``.
    ``has_explicit_hash`` uses CPython's heuristic: a ``__hash__`` of ``None``
    that was auto-installed by Python because the class body defines ``__eq__``
    is *not* treated as explicit (so the table can still set it to ``None``).
    """
    # CPython evaluates this *after* __eq__ generation; at that point a
    # class-body __eq__ has caused Python to auto-set __hash__ = None.
    class_hash = cls.__dict__.get("__hash__", MISSING)
    has_explicit_hash = not (
        class_hash is MISSING
        or (class_hash is None and "__eq__" in cls.__dict__)
    )
    action = _HASH_ACTION[(bool(unsafe_hash), bool(eq), bool(frozen),
                           has_explicit_hash)]
    if action == "add":
        setattr(cls, "__hash__", _make_hash(flds))
    elif action == "none":
        setattr(cls, "__hash__", None)
    elif action == "raise":
        raise TypeError("Cannot overwrite attribute __hash__ in class %s"
                        % cls.__name__)
    # action is None → leave __hash__ untouched.


def _make_hash(flds):
    # A field participates in __hash__ when its `hash` is True, or — when
    # `hash` is left at the default None — when it participates in compare.
    names = [f.name for f in flds
             if (f.hash if f.hash is not None else f.compare)]
    self_tuple = "(" + ", ".join("self.%s" % n for n in names)
    self_tuple += "," if len(names) == 1 else ""
    self_tuple += ")"
    body = ["return hash(%s)" % self_tuple]
    return _create_fn("__hash__", ["self"], body, {})


def _make_init(flds, frozen):
    # Build a parameter list with defaults, then the body assigning each field.
    # Fields without defaults must precede those with defaults.
    params = ["self"]
    body = []
    seen_default = False
    locals_ns = {"MISSING": MISSING}
    for f in flds:
        if not f.init:
            # Not an __init__ parameter; assign from default/factory directly.
            if f.default_factory is not MISSING:
                locals_ns["__df_" + f.name] = f.default_factory
                body.append(_assign(f.name, "__df_%s()" % f.name, frozen))
            elif f.default is not MISSING:
                locals_ns["__def_" + f.name] = f.default
                body.append(_assign(f.name, "__def_%s" % f.name, frozen))
            continue
        if f.default_factory is not MISSING:
            locals_ns["__df_" + f.name] = f.default_factory
            params.append("%s=MISSING" % f.name)
            seen_default = True
            body.append("if %s is MISSING: %s = __df_%s()"
                        % (f.name, f.name, f.name))
            body.append(_assign(f.name, f.name, frozen))
        elif f.default is not MISSING:
            locals_ns["__def_" + f.name] = f.default
            params.append("%s=__def_%s" % (f.name, f.name))
            seen_default = True
            body.append(_assign(f.name, f.name, frozen))
        else:
            if seen_default:
                raise TypeError(
                    "non-default argument %r follows default argument"
                    % f.name)
            params.append(f.name)
            body.append(_assign(f.name, f.name, frozen))
    if not body:
        body = ["pass"]
    return _create_fn("__init__", params, body, locals_ns)


def _assign(name, value_expr, frozen):
    if frozen:
        # Bypass the frozen __setattr__ guard during construction.
        return ("object.__setattr__(self, %r, %s)" % (name, value_expr))
    return "self.%s = %s" % (name, value_expr)


def _make_repr(cls, flds):
    parts = ", ".join("%s={self.%s!r}" % (f.name, f.name)
                      for f in flds if f.repr)
    body = ['return f"{self.__class__.__qualname__}(%s)"' % parts]
    return _create_fn("__repr__", ["self"], body, {})


def _make_eq(cls, flds):
    names = [f.name for f in flds if f.compare]
    self_tuple = "(" + ", ".join("self.%s" % n for n in names)
    self_tuple += "," if len(names) == 1 else ""
    self_tuple += ")"
    other_tuple = "(" + ", ".join("other.%s" % n for n in names)
    other_tuple += "," if len(names) == 1 else ""
    other_tuple += ")"
    body = [
        "if other.__class__ is self.__class__:",
        "    return %s == %s" % (self_tuple, other_tuple),
        "return NotImplemented",
    ]
    return _create_fn("__eq__", ["self", "other"], body, {})


def _frozen_setattr(self, name, value):
    raise FrozenInstanceError("cannot assign to field %r" % name)


def _frozen_delattr(self, name):
    raise FrozenInstanceError("cannot delete field %r" % name)


def _create_fn(name, params, body, locals_ns):
    """Compile a function from source text and return the function object."""
    args = ", ".join(params)
    body_text = "\n".join("    " + line for line in body)
    txt = "def %s(%s):\n%s" % (name, args, body_text)
    ns = dict(locals_ns)
    exec(txt, ns)
    return ns[name]


def dataclass(cls=None, /, *, init=True, repr=True, eq=True, frozen=False,
              unsafe_hash=False):
    """Add generated special methods to a class.

    Usable as ``@dataclass`` or ``@dataclass(frozen=True, ...)``.
    """
    def wrap(klass):
        return _process_class(klass, init, repr, eq, frozen, unsafe_hash)

    # Called as @dataclass without parentheses.
    if cls is None:
        return wrap
    return wrap(cls)


def fields(class_or_instance):
    """Return a tuple of Field objects for a dataclass or its instance."""
    try:
        flds = getattr(class_or_instance, _FIELDS)
    except AttributeError:
        raise TypeError("fields() should be called with a dataclass type or "
                        "instance")
    return tuple(flds)


def is_dataclass(obj):
    """Return True if ``obj`` is a dataclass or an instance of one."""
    cls = obj if isinstance(obj, type) else type(obj)
    return hasattr(cls, _FIELDS)


def asdict(obj):
    """Recursively convert a dataclass instance to a dict of field values."""
    if not is_dataclass(obj) or isinstance(obj, type):
        raise TypeError("asdict() should be called on dataclass instances")
    return _asdict_inner(obj)


def _asdict_inner(obj):
    if is_dataclass(obj) and not isinstance(obj, type):
        result = {}
        for f in fields(obj):
            result[f.name] = _asdict_inner(getattr(obj, f.name))
        return result
    elif isinstance(obj, (list, tuple)):
        return type(obj)(_asdict_inner(v) for v in obj)
    elif isinstance(obj, dict):
        return {_asdict_inner(k): _asdict_inner(v) for k, v in obj.items()}
    else:
        return obj


def astuple(obj):
    """Recursively convert a dataclass instance to a tuple of field values."""
    if not is_dataclass(obj) or isinstance(obj, type):
        raise TypeError("astuple() should be called on dataclass instances")
    return _astuple_inner(obj)


def _astuple_inner(obj):
    if is_dataclass(obj) and not isinstance(obj, type):
        return tuple(_astuple_inner(getattr(obj, f.name)) for f in fields(obj))
    elif isinstance(obj, (list, tuple)):
        return type(obj)(_astuple_inner(v) for v in obj)
    elif isinstance(obj, dict):
        return {_astuple_inner(k): _astuple_inner(v) for k, v in obj.items()}
    else:
        return obj


def replace(obj, /, **changes):
    """Return a new object of the same type, replacing the given fields."""
    if isinstance(obj, type) or not is_dataclass(obj):
        raise TypeError("replace() should be called on dataclass instances")
    kwargs = {}
    for f in fields(obj):
        if not f.init:
            continue
        if f.name in changes:
            kwargs[f.name] = changes[f.name]
        else:
            kwargs[f.name] = getattr(obj, f.name)
    return obj.__class__(**kwargs)
