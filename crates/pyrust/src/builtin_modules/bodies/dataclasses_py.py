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
                 "compare", "kw_only")

    def __init__(self, default, default_factory, init, repr, compare,
                 kw_only=MISSING):
        self.name = None
        self.type = None
        self.default = default
        self.default_factory = default_factory
        self.init = init
        self.repr = repr
        self.compare = compare
        self.kw_only = kw_only

    def __repr__(self):
        return ("Field(name=%r,type=%r,default=%r,default_factory=%r,"
                "init=%r,repr=%r,compare=%r,kw_only=%r)" % (
                    self.name, self.type, self.default, self.default_factory,
                    self.init, self.repr, self.compare, self.kw_only))


def field(*, default=MISSING, default_factory=MISSING, init=True, repr=True,
          compare=True, kw_only=MISSING):
    """Return an object to identify dataclass fields.

    ``default`` and ``default_factory`` are mutually exclusive.
    """
    if default is not MISSING and default_factory is not MISSING:
        raise ValueError("cannot specify both default and default_factory")
    return Field(default, default_factory, init, repr, compare, kw_only)


# Name under which the per-class tuple of Field objects is stashed.
_FIELDS = "__dataclass_fields__"


def _collect_fields(cls, cls_kw_only):
    """Build the ordered list of Field objects for ``cls``.

    Walks base classes first (reverse MRO, excluding ``object`` and ``cls``)
    so inherited fields precede this class's own, matching CPython's ordering.
    Later definitions override earlier ones by name.

    ``cls_kw_only`` is the class-level ``kw_only`` flag; a field whose own
    ``kw_only`` is left at ``MISSING`` inherits it.
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
            f = Field(default, MISSING, True, True, True)
        f.name = name
        f.type = atype
        if f.kw_only is MISSING:
            f.kw_only = cls_kw_only
        fields[name] = f
    return list(fields.values())


def _set_new_attribute(cls, name, value):
    # Only set if not already defined directly on the class body.
    if name in cls.__dict__:
        return True
    setattr(cls, name, value)
    return False


def _process_class(cls, init, repr, eq, order, unsafe_hash, frozen, match_args,
                   kw_only, slots):
    if order and not eq:
        raise ValueError("eq must be true if order is true")

    flds = _collect_fields(cls, kw_only)
    setattr(cls, _FIELDS, flds)

    if init:
        _set_new_attribute(cls, "__init__", _make_init(flds, frozen))
    if repr:
        _set_new_attribute(cls, "__repr__", _make_repr(cls, flds))
    if eq:
        _set_new_attribute(cls, "__eq__", _make_eq(cls, flds))
    if order:
        for name, op in (("__lt__", "<"), ("__le__", "<="),
                         ("__gt__", ">"), ("__ge__", ">=")):
            if _set_new_attribute(cls, name, _make_cmp(name, op, flds)):
                raise TypeError(
                    "Cannot overwrite attribute %s in class %s. Consider "
                    "using functools.total_ordering"
                    % (name, cls.__name__))
    if frozen:
        # Always install the frozen guards (overriding any inherited setters).
        setattr(cls, "__setattr__", _frozen_setattr)
        setattr(cls, "__delattr__", _frozen_delattr)

    # __hash__: mirror CPython's (eq, frozen) decision table, with
    # unsafe_hash as an override that always synthesises a hash.
    _apply_hash(cls, flds, eq, frozen, unsafe_hash)

    if match_args:
        # __match_args__ lists the non-kw-only init fields, in order.
        _set_new_attribute(
            cls, "__match_args__",
            tuple(f.name for f in flds if f.init and not f.kw_only))

    if slots:
        cls = _add_slots(cls, flds)

    return cls


def _init_param(f, locals_ns):
    """Render the ``__init__`` parameter spelling for an init field."""
    if f.default_factory is not MISSING:
        locals_ns["__df_" + f.name] = f.default_factory
        return "%s=MISSING" % f.name
    if f.default is not MISSING:
        locals_ns["__def_" + f.name] = f.default
        return "%s=__def_%s" % (f.name, f.name)
    return f.name


def _init_assign(f, body, frozen):
    """Append the body statements assigning an init field from its param."""
    if f.default_factory is not MISSING:
        body.append("if %s is MISSING: %s = __df_%s()"
                    % (f.name, f.name, f.name))
    body.append(_assign(f.name, f.name, frozen))


def _make_init(flds, frozen):
    # Build a parameter list with defaults, then the body assigning each field.
    # Positional fields without defaults must precede those with defaults;
    # keyword-only fields go after a ``*`` separator and have no such rule.
    pos_params = []
    kw_params = []
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
        if f.kw_only:
            kw_params.append(_init_param(f, locals_ns))
            _init_assign(f, body, frozen)
            continue
        has_default = (f.default is not MISSING
                       or f.default_factory is not MISSING)
        if not has_default and seen_default:
            raise TypeError(
                "non-default argument %r follows default argument" % f.name)
        if has_default:
            seen_default = True
        pos_params.append(_init_param(f, locals_ns))
        _init_assign(f, body, frozen)
    params = ["self"] + pos_params
    if kw_params:
        params.append("*")
        params.extend(kw_params)
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


def _cmp_tuple(receiver, names):
    inner = ", ".join("%s.%s" % (receiver, n) for n in names)
    if len(names) == 1:
        inner += ","
    return "(" + inner + ")"


def _make_cmp(name, op, flds):
    names = [f.name for f in flds if f.compare]
    body = [
        "if other.__class__ is self.__class__:",
        "    return %s %s %s" % (_cmp_tuple("self", names), op,
                                 _cmp_tuple("other", names)),
        "return NotImplemented",
    ]
    return _create_fn(name, ["self", "other"], body, {})


def _make_hash(flds):
    names = [f.name for f in flds if f.compare]
    body = ["return hash(%s)" % _cmp_tuple("self", names)]
    return _create_fn("__hash__", ["self"], body, {})


def _apply_hash(cls, flds, eq, frozen, unsafe_hash):
    """Set ``__hash__`` per CPython 3.12's (eq, frozen, unsafe_hash) table.

    - unsafe_hash=True   -> always synthesise a field-based hash.
    - eq and frozen      -> synthesise a field-based hash.
    - eq and not frozen  -> set __hash__ = None (unhashable).
    - not eq             -> leave whatever was inherited untouched.
    """
    if unsafe_hash:
        setattr(cls, "__hash__", _make_hash(flds))
        return
    if eq and frozen:
        _set_new_attribute(cls, "__hash__", _make_hash(flds))
    elif eq:
        setattr(cls, "__hash__", None)


def _add_slots(cls, flds):
    """Recreate ``cls`` with ``__slots__`` set to the field names.

    ``__slots__`` cannot be added to an existing class, so build a fresh one
    from the same bases / namespace, dropping the per-field class attributes
    that would otherwise shadow the slot descriptors.
    """
    field_names = tuple(f.name for f in flds)
    cls_dict = dict(cls.__dict__)
    cls_dict["__slots__"] = field_names
    for name in field_names:
        cls_dict.pop(name, None)
    # __dict__ / __weakref__ descriptors are recreated by the new class.
    cls_dict.pop("__dict__", None)
    cls_dict.pop("__weakref__", None)
    qualname = getattr(cls, "__qualname__", cls.__name__)
    new_cls = type(cls)(cls.__name__, cls.__bases__, cls_dict)
    new_cls.__qualname__ = qualname
    return new_cls


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


def dataclass(cls=None, /, *, init=True, repr=True, eq=True, order=False,
              unsafe_hash=False, frozen=False, match_args=True, kw_only=False,
              slots=False, weakref_slot=False):
    """Add generated special methods to a class.

    Usable as ``@dataclass`` or ``@dataclass(frozen=True, ...)``.
    """
    def wrap(klass):
        return _process_class(klass, init, repr, eq, order, unsafe_hash,
                              frozen, match_args, kw_only, slots)

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
