"""
Data Classes (pyrust port targeting CPython 3.12's ``dataclasses`` module).

Provides the ``@dataclass`` decorator, ``InitVar``, ``KW_ONLY``, ``MISSING``,
``Field``, ``FrozenInstanceError``, and the ``field`` / ``fields`` /
``asdict`` / ``astuple`` / ``replace`` / ``is_dataclass`` / ``make_dataclass``
helpers.

This source is exec'd once into a throwaway namespace at first
``import dataclasses``; the public names are copied onto the module by
``dataclasses.rs::inject_python_members`` (mirrors ``operator`` / ``string``).

Reference: <https://docs.python.org/3/library/dataclasses.html>
"""

import copy as _copy
import sys as _sys
import types as _types


# Sentinel for "no default supplied" — distinct from None, which is a valid
# default value.
class _MISSING_TYPE:
    def __repr__(self):
        return "MISSING"


MISSING = _MISSING_TYPE()


# Sentinel used as a *type annotation* in a class body: a bare ``_: KW_ONLY``
# marks every field defined after it as keyword-only (CPython 3.10+).
class _KW_ONLY_TYPE:
    pass


KW_ONLY = _KW_ONLY_TYPE()


# Field kind sentinels (mirrors CPython's _FIELD_BASE).
class _FIELD_BASE:
    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return self.name


_FIELD = _FIELD_BASE("_FIELD")
_FIELD_CLASSVAR = _FIELD_BASE("_FIELD_CLASSVAR")
_FIELD_INITVAR = _FIELD_BASE("_FIELD_INITVAR")

_EMPTY_METADATA = _types.MappingProxyType({})

# Python 3.12 hard keywords for make_dataclass identifier validation.
_KEYWORDS = frozenset((
    "False", "None", "True", "and", "as", "assert", "async", "await",
    "break", "class", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try",
    "while", "with", "yield",
))


class InitVar:
    """Marker for init-only dataclass fields passed to ``__post_init__``."""

    __slots__ = ("type",)

    def __init__(self, type):
        self.type = type

    def __repr__(self):
        if isinstance(self.type, type):
            type_name = self.type.__name__
        else:
            type_name = repr(self.type)
        return "dataclasses.InitVar[%s]" % type_name

    def __class_getitem__(cls, type):
        return InitVar(type)


class FrozenInstanceError(AttributeError):
    """Raised when assigning to a field of a frozen dataclass instance."""


class Field:
    """Describes a single dataclass field.

    Instances are created by the module-level ``field()`` function and, for
    plain annotated attributes, synthesised by the ``@dataclass`` decorator.
    """
    __slots__ = ("name", "type", "default", "default_factory", "init", "repr",
                 "compare", "hash", "metadata", "kw_only", "_field_type")

    def __init__(self, default, default_factory, init, repr, hash, compare,
                 metadata=None, kw_only=MISSING):
        self.name = None
        self.type = None
        self.default = default
        self.default_factory = default_factory
        self.init = init
        self.repr = repr
        self.hash = hash
        self.compare = compare
        self.metadata = (_EMPTY_METADATA
                         if metadata is None
                         else _types.MappingProxyType(metadata))
        self.kw_only = kw_only
        self._field_type = None

    def __repr__(self):
        return ("Field(name=%r,type=%r,default=%r,default_factory=%r,"
                "init=%r,repr=%r,hash=%r,compare=%r,metadata=%r,"
                "kw_only=%r,_field_type=%r)" % (
                    self.name, self.type, self.default, self.default_factory,
                    self.init, self.repr, self.hash, self.compare,
                    self.metadata, self.kw_only, self._field_type))


def field(*, default=MISSING, default_factory=MISSING, init=True, repr=True,
          hash=None, compare=True, metadata=None, kw_only=MISSING):
    """Return an object to identify dataclass fields.

    ``default`` and ``default_factory`` are mutually exclusive.  ``hash``
    defaults to ``None``, meaning "use the value of ``compare``"; set it
    explicitly to include/exclude a field from the generated ``__hash__``
    independently of ``__eq__`` (mirrors CPython's ``field(hash=...)``).
    """
    if default is not MISSING and default_factory is not MISSING:
        raise ValueError("cannot specify both default and default_factory")
    return Field(default, default_factory, init, repr, hash, compare,
                 metadata, kw_only)


# Name under which the per-class dict of Field objects is stashed.
_FIELDS = "__dataclass_fields__"


def _is_string_marker(a_type, mod_name, marker_name):
    """Best-effort check for stringified annotations like ``ClassVar[int]`` or
    ``typing.ClassVar[int]`` (when ``from __future__ import annotations`` is
    active)."""
    s = a_type.strip()
    if s == marker_name or s.startswith(marker_name + "["):
        return True
    qualified = mod_name + "." + marker_name
    if s == qualified or s.startswith(qualified + "["):
        return True
    return False


def _is_classvar(a_type, typing_mod):
    if typing_mod is not None:
        cv = getattr(typing_mod, "ClassVar", None)
        if cv is not None:
            if a_type is cv or getattr(a_type, "__origin__", None) is cv:
                return True
    if isinstance(a_type, str):
        return _is_string_marker(a_type, "typing", "ClassVar")
    # Handle ClassVar even if `typing` was imported under another generation
    # or before `sys.modules["typing"]` was consulted.
    origin = getattr(a_type, "__origin__", None)
    if (origin is not None
            and getattr(origin, "__module__", None) == "typing"
            and getattr(origin, "__name__", None) == "ClassVar"):
        return True
    if (isinstance(a_type, type)
            and getattr(a_type, "__module__", None) == "typing"
            and getattr(a_type, "__name__", None) == "ClassVar"):
        return True
    return False


def _is_initvar(a_type):
    if a_type is InitVar or isinstance(a_type, InitVar):
        return True
    if isinstance(a_type, str):
        return _is_string_marker(a_type, "dataclasses", "InitVar")
    return False


def _is_kw_only(a_type):
    if a_type is KW_ONLY or isinstance(a_type, _KW_ONLY_TYPE):
        return True
    if isinstance(a_type, str):
        return _is_string_marker(a_type, "dataclasses", "KW_ONLY")
    return False


def _collect_fields(cls, cls_kw_only):
    """Build the ordered dict of Field objects for ``cls``.

    Walks base classes first (reverse MRO, excluding ``object`` and ``cls``)
    so inherited fields precede this class's own, matching CPython's ordering.
    Later definitions override earlier ones by name.

    ``cls_kw_only`` is the class-level ``kw_only`` flag; a field whose own
    ``kw_only`` is left at ``MISSING`` inherits it.
    """
    typing_mod = _sys.modules.get("typing")
    all_fields = {}
    # Inherited fields, base-first.
    for base in cls.__mro__[-1:0:-1]:
        base_fields = getattr(base, _FIELDS, None)
        if base_fields is not None:
            if isinstance(base_fields, dict):
                items = base_fields.values()
            else:
                items = base_fields
            for f in items:
                all_fields[f.name] = f
    # This class's own annotations.  A bare ``_: KW_ONLY`` pseudo-field flips
    # every subsequent field in this class body to keyword-only.
    cls_annotations = cls.__dict__.get("__annotations__", {})
    kw_only_seen = False
    for name, atype in cls_annotations.items():
        if _is_kw_only(atype):
            if kw_only_seen:
                raise TypeError(
                    "%r is KW_ONLY, but KW_ONLY has already been specified"
                    % name)
            kw_only_seen = True
            cls_kw_only = True
            continue
        default = cls.__dict__.get(name, MISSING)
        if isinstance(default, Field):
            f = default
        else:
            f = Field(default, MISSING, True, True, None, True)
        f.name = name
        f.type = atype
        if _is_classvar(atype, typing_mod):
            f._field_type = _FIELD_CLASSVAR
        elif _is_initvar(atype):
            f._field_type = _FIELD_INITVAR
            if f.default_factory is not MISSING:
                raise TypeError("field %s cannot have a default factory" % name)
            if f.kw_only is MISSING:
                f.kw_only = cls_kw_only
            # Replace Field sentinel on class body with default (or delete).
            if isinstance(default, Field):
                if f.default is MISSING:
                    delattr(cls, name)
                else:
                    setattr(cls, name, f.default)
        else:
            f._field_type = _FIELD
            if f.kw_only is MISSING:
                f.kw_only = cls_kw_only
            if isinstance(default, Field):
                if f.default is MISSING:
                    delattr(cls, name)
                else:
                    setattr(cls, name, f.default)
        all_fields[name] = f
    return all_fields


def _set_new_attribute(cls, name, value):
    # Only set if not already defined directly on the class body.
    if name in cls.__dict__:
        return True
    setattr(cls, name, value)
    return False


def _process_class(cls, init, repr, eq, order, unsafe_hash, frozen, match_args,
                   kw_only, slots, weakref_slot):
    if order and not eq:
        raise ValueError("eq must be true if order is true")
    if weakref_slot and not slots:
        raise TypeError("weakref_slot is True but slots is False")

    all_fields = _collect_fields(cls, kw_only)
    setattr(cls, _FIELDS, all_fields)

    regular_fields = [f for f in all_fields.values() if f._field_type is _FIELD]
    init_fields = [f for f in all_fields.values()
                   if f._field_type in (_FIELD, _FIELD_INITVAR)]

    if init:
        _set_new_attribute(cls, "__init__", _make_init(cls, init_fields, frozen))
    if repr:
        _set_new_attribute(cls, "__repr__", _make_repr(cls, regular_fields))
    if eq:
        _set_new_attribute(cls, "__eq__", _make_eq(cls, regular_fields))
    if order:
        for name, op in (("__lt__", "<"), ("__le__", "<="),
                         ("__gt__", ">"), ("__ge__", ">=")):
            if _set_new_attribute(cls, name, _make_cmp(name, op, regular_fields)):
                raise TypeError(
                    "Cannot overwrite attribute %s in class %s. Consider "
                    "using functools.total_ordering"
                    % (name, cls.__name__))
    if frozen:
        # Always install the frozen guards (overriding any inherited setters).
        setattr(cls, "__setattr__", _frozen_setattr)
        setattr(cls, "__delattr__", _frozen_delattr)

    _set_hash(cls, regular_fields, eq, frozen, unsafe_hash)

    if match_args:
        # __match_args__ lists the non-kw-only init fields (including InitVar),
        # in order, matching CPython 3.12.
        _set_new_attribute(
            cls, "__match_args__",
            tuple(f.name for f in init_fields if f.init and not f.kw_only))

    if slots:
        cls = _add_slots(cls, regular_fields, weakref_slot)

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
    """Install ``__hash__`` following CPython 3.12's ``_hash_action`` table."""
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


def _make_hash(flds):
    names = [f.name for f in flds
             if (f.hash if f.hash is not None else f.compare)]
    body = ["return hash(%s)" % _cmp_tuple("self", names)]
    return _create_fn("__hash__", ["self"], body, {})


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
    """Append the body statements assigning a regular init field from its param."""
    if f.default_factory is not MISSING:
        body.append("if %s is MISSING: %s = __df_%s()"
                    % (f.name, f.name, f.name))
    body.append(_assign(f.name, f.name, frozen))


def _make_init(cls, flds, frozen):
    # Build a parameter list with defaults, then the body assigning each regular
    # field and forwarding InitVar arguments to ``__post_init__``.
    pos_params = []
    kw_params = []
    init_vars = []
    body = []
    seen_default = False
    locals_ns = {"MISSING": MISSING}
    for f in flds:
        is_initvar = f._field_type is _FIELD_INITVAR
        if is_initvar:
            init_vars.append(f.name)
        if not f.init:
            if not is_initvar:
                if f.default_factory is not MISSING:
                    locals_ns["__df_" + f.name] = f.default_factory
                    body.append(_assign(f.name, "__df_%s()" % f.name, frozen))
                elif f.default is not MISSING:
                    locals_ns["__def_" + f.name] = f.default
                    body.append(_assign(f.name, "__def_%s" % f.name, frozen))
            continue
        if f.kw_only:
            kw_params.append(_init_param(f, locals_ns))
            if not is_initvar:
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
        if not is_initvar:
            _init_assign(f, body, frozen)
    params = ["self"] + pos_params
    if kw_params:
        params.append("*")
        params.extend(kw_params)
    if hasattr(cls, "__post_init__"):
        body.append("self.__post_init__(%s)" % ", ".join(init_vars))
    if not body:
        body = ["pass"]
    return _create_fn("__init__", params, body, locals_ns)


def _assign(name, value_expr, frozen):
    if frozen:
        return "object.__setattr__(self, %r, %s)" % (name, value_expr)
    return "self.%s = %s" % (name, value_expr)


def _make_repr(cls, flds):
    parts = ", ".join("%s={self.%s!r}" % (f.name, f.name)
                      for f in flds if f.repr)
    body = ['return f"{self.__class__.__qualname__}(%s)"' % parts]
    return _create_fn("__repr__", ["self"], body, {})


def _cmp_tuple(receiver, names):
    inner = ", ".join("%s.%s" % (receiver, n) for n in names)
    if len(names) == 1:
        inner += ","
    return "(" + inner + ")"


def _make_eq(cls, flds):
    names = [f.name for f in flds if f.compare]
    body = [
        "if other.__class__ is self.__class__:",
        "    return %s == %s" % (_cmp_tuple("self", names),
                                 _cmp_tuple("other", names)),
        "return NotImplemented",
    ]
    return _create_fn("__eq__", ["self", "other"], body, {})


def _make_cmp(name, op, flds):
    names = [f.name for f in flds if f.compare]
    body = [
        "if other.__class__ is self.__class__:",
        "    return %s %s %s" % (_cmp_tuple("self", names), op,
                                 _cmp_tuple("other", names)),
        "return NotImplemented",
    ]
    return _create_fn(name, ["self", "other"], body, {})


def _add_slots(cls, flds, weakref_slot=False):
    """Recreate ``cls`` with ``__slots__`` set to the regular field names."""
    field_names = tuple(f.name for f in flds)
    if weakref_slot:
        field_names = field_names + ("__weakref__",)
    cls_dict = dict(cls.__dict__)
    cls_dict["__slots__"] = field_names
    for name in field_names:
        if name != "__weakref__":
            cls_dict.pop(name, None)
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
                              frozen, match_args, kw_only, slots, weakref_slot)

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
    if isinstance(flds, dict):
        return tuple(f for f in flds.values() if f._field_type is _FIELD)
    return tuple(f for f in flds if f._field_type is _FIELD)


def is_dataclass(obj):
    """Return True if ``obj`` is a dataclass or an instance of one."""
    cls = obj if isinstance(obj, type) else type(obj)
    return hasattr(cls, _FIELDS)


def _is_dataclass_instance(obj):
    return hasattr(type(obj), _FIELDS)


def asdict(obj, *, dict_factory=dict):
    """Recursively convert a dataclass instance to a dict of field values."""
    if not _is_dataclass_instance(obj):
        raise TypeError("asdict() should be called on dataclass instances")
    return _asdict_inner(obj, dict_factory)


def _asdict_inner(obj, dict_factory):
    if _is_dataclass_instance(obj):
        if dict_factory is dict:
            return {
                f.name: _asdict_inner(getattr(obj, f.name), dict)
                for f in fields(obj)
            }
        result = []
        for f in fields(obj):
            value = _asdict_inner(getattr(obj, f.name), dict_factory)
            result.append((f.name, value))
        return dict_factory(result)
    elif isinstance(obj, tuple) and hasattr(obj, "_fields"):
        return type(obj)(*[_asdict_inner(v, dict_factory) for v in obj])
    elif isinstance(obj, (list, tuple)):
        return type(obj)(_asdict_inner(v, dict_factory) for v in obj)
    elif isinstance(obj, dict):
        return type(obj)((_asdict_inner(k, dict_factory),
                          _asdict_inner(v, dict_factory))
                         for k, v in obj.items())
    else:
        return _copy.deepcopy(obj)


def astuple(obj, *, tuple_factory=tuple):
    """Recursively convert a dataclass instance to a tuple of field values."""
    if not _is_dataclass_instance(obj):
        raise TypeError("astuple() should be called on dataclass instances")
    return _astuple_inner(obj, tuple_factory)


def _astuple_inner(obj, tuple_factory):
    if _is_dataclass_instance(obj):
        result = []
        for f in fields(obj):
            value = _astuple_inner(getattr(obj, f.name), tuple_factory)
            result.append(value)
        return tuple_factory(result)
    elif isinstance(obj, tuple) and hasattr(obj, "_fields"):
        return type(obj)(*[_astuple_inner(v, tuple_factory) for v in obj])
    elif isinstance(obj, (list, tuple)):
        return type(obj)(_astuple_inner(v, tuple_factory) for v in obj)
    elif isinstance(obj, dict):
        return type(obj)((_astuple_inner(k, tuple_factory),
                          _astuple_inner(v, tuple_factory))
                         for k, v in obj.items())
    else:
        return _copy.deepcopy(obj)


def replace(obj, /, **changes):
    """Return a new object of the same type, replacing the given fields."""
    if not _is_dataclass_instance(obj):
        raise TypeError("replace() should be called on dataclass instances")
    all_fields = getattr(obj, _FIELDS)
    items = all_fields.values() if isinstance(all_fields, dict) else all_fields
    for f in items:
        if f._field_type is _FIELD_CLASSVAR:
            continue
        if not f.init:
            if f.name in changes:
                raise ValueError(
                    "field %s is declared with init=False, "
                    "it cannot be specified with replace()" % f.name)
            continue
        if f.name not in changes:
            if f._field_type is _FIELD_INITVAR and f.default is MISSING:
                raise ValueError(
                    "InitVar %r must be specified with replace()" % f.name)
            if f._field_type is _FIELD:
                changes[f.name] = getattr(obj, f.name)
    return obj.__class__(**changes)


def make_dataclass(cls_name, fields, *, bases=(), namespace=None, init=True,
                   repr=True, eq=True, order=False, unsafe_hash=False,
                   frozen=False, match_args=True, kw_only=False, slots=False,
                   weakref_slot=False, module=None):
    """Return a new dynamically created dataclass."""
    if namespace is None:
        namespace = {}
    else:
        namespace = dict(namespace)

    seen = set()
    annotations = {}
    defaults = {}
    typing_mod = None
    for item in fields:
        if isinstance(item, str):
            name = item
            if typing_mod is None:
                import typing as typing_mod
            tp = typing_mod.Any
        elif isinstance(item, tuple) and len(item) == 2:
            name, tp = item
        elif isinstance(item, tuple) and len(item) == 3:
            name, tp, spec = item
            defaults[name] = spec
        else:
            raise TypeError("Invalid field: %r" % (item,))

        if not isinstance(name, str) or not name.isidentifier():
            raise TypeError("Field names must be valid identifiers: %r" % (name,))
        if name in _KEYWORDS:
            raise TypeError("Field names must not be keywords: %r" % (name,))
        if name in seen:
            raise TypeError("Field name duplicated: %r" % (name,))
        seen.add(name)
        annotations[name] = tp

    namespace["__annotations__"] = annotations
    for name, value in defaults.items():
        namespace[name] = value

    if module is None:
        try:
            module = _sys._getframe(1).f_globals.get("__name__", "__main__")
        except (AttributeError, ValueError):
            module = "__main__"
    if module is not None:
        namespace["__module__"] = module

    cls = type(cls_name, bases, namespace)
    return dataclass(
        cls,
        init=init,
        repr=repr,
        eq=eq,
        order=order,
        unsafe_hash=unsafe_hash,
        frozen=frozen,
        match_args=match_args,
        kw_only=kw_only,
        slots=slots,
        weakref_slot=weakref_slot,
    )
