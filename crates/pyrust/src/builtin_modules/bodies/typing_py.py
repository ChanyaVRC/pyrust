# Python-source members of the `typing` module (issue #2516).
#
# These names are most naturally expressed in Python and are exec'd once into
# a throwaway namespace at first import of `typing`; the resulting public names
# are copied onto the module by `inject_python_members`.  The native special
# forms (`List`, `Optional`, `Union`, `Generic`, `Protocol`, `TypeVar`, …) are
# already present on the module before this source runs, so the helpers below
# may reference them via the names the exec namespace is pre-seeded with.

import collections as _collections
import sys as _sys


def get_origin(tp):
    """Return the unsubscripted origin of a typing/PEP 585 generic.

    `get_origin(List[int])` is `list`, `get_origin(Optional[str])` is `Union`,
    `get_origin(int)` is `None`.  Mirrors CPython's `typing.get_origin`.
    """
    # `Annotated[X, ...]` reports the `Annotated` marker as its origin, even
    # though it stores `X` in `__origin__` (PEP 593 / CPython `get_origin`).
    if isinstance(tp, _AnnotatedAlias):
        return Annotated
    origin = getattr(tp, "__origin__", None)
    if origin is None:
        return None
    # The native special-form aliases carry the sentinel class as their
    # `__origin__`.  Normalise to match CPython's observable origins.
    if origin is Optional or origin is Union:
        return Union
    # The deprecated `List`/`Dict`/`Type`/… aliases carry the alias class as
    # their `__origin__`; CPython reports the underlying builtin (`list`,
    # `type`, …), which is stashed on the alias as `__pyrust_legacy_alias_of__`.
    builtin = getattr(origin, "__pyrust_legacy_alias_of__", None)
    if builtin is not None:
        return builtin
    return origin


def get_args(tp):
    """Return the type arguments of a typing/PEP 585 generic.

    `get_args(List[int])` is `(int,)`, `get_args(Optional[str])` is
    `(str, NoneType)`, `get_args(int)` is `()`.
    """
    # `Annotated[X, *meta]` reports `(X, *meta)`, even though it stores only
    # `(X,)` in `__args__` (PEP 593 / CPython `get_args`).
    if isinstance(tp, _AnnotatedAlias):
        return (tp.__origin__,) + tp.__metadata__
    args = getattr(tp, "__args__", None)
    if args is None:
        return ()
    origin = getattr(tp, "__origin__", None)
    # `Optional[X]` is `Union[X, None]` in CPython, so its args include
    # `NoneType`.  pyrust's native `Optional[X]` carries just `(X,)`.
    if origin is Optional and type(None) not in args:
        return tuple(args) + (type(None),)
    return tuple(args)


def _resolve(value, globalns):
    """Resolve a single annotation: eval string forward refs, else identity."""
    if isinstance(value, str):
        try:
            return eval(value, globalns)
        except Exception:
            return value
    return value


def _strip_annotated(value):
    """Strip an `Annotated[X, ...]` alias down to its underlying type `X`.

    Used by `get_type_hints` when `include_extras=False`, mirroring CPython,
    which discards the metadata and keeps only the annotated type.
    """
    if isinstance(value, _AnnotatedAlias):
        return value.__origin__
    return value


def get_type_hints(obj, globalns=None, localns=None, include_extras=False):
    """Return a dict of type hints for a function, class, or module.

    String (forward-reference) annotations are evaluated against the object's
    globals.  For classes, annotations are collected across the MRO with base
    classes contributing first (subclass annotations win on conflict).  Unless
    `include_extras` is set, `Annotated[X, ...]` hints are stripped to `X`.
    """
    if isinstance(obj, type):
        hints = {}
        for base in reversed(obj.__mro__):
            base_globals = globalns
            if base_globals is None:
                module = getattr(base, "__module__", None)
                mod = _sys.modules.get(module) if module else None
                base_globals = getattr(mod, "__dict__", {}) if mod else {}
            ann = base.__dict__.get("__annotations__", {})
            for name, value in ann.items():
                hints[name] = _resolve(value, base_globals)
    else:
        ann = getattr(obj, "__annotations__", None)
        if ann is None:
            return {}
        if globalns is None:
            globalns = getattr(obj, "__globals__", {})
        hints = {name: _resolve(value, globalns) for name, value in ann.items()}

    if not include_extras:
        hints = {name: _strip_annotated(value) for name, value in hints.items()}
    return hints


def _namedtuple_functional(typename, fields=None, /, **kwargs):
    """Functional form of `typing.NamedTuple`.

    `NamedTuple('Point', [('x', int), ('y', int)])` and the keyword form
    `NamedTuple('Point', x=int, y=int)` both build a `collections.namedtuple`.
    The class form `class Point(NamedTuple): x: int` is handled natively.
    This is invoked from the native `NamedTuple` marker's call path.
    """
    if fields is None:
        fields = list(kwargs.items())
    elif kwargs:
        raise TypeError(
            "Either list of fields or keywords can be provided to NamedTuple, not both"
        )
    names = [n for (n, _t) in fields]
    return _collections.namedtuple(typename, names)


def _build_namedtuple_class(typename, fields, defaults, namespace):
    """Build a `NamedTuple` subclass from a `class` statement (issue #2516).

    Called natively from class creation when a class inherits from the
    `NamedTuple` marker.  `fields` is the ordered list of annotated field
    names, `defaults` maps a subset of them to default values, and `namespace`
    holds any extra members defined in the class body (methods, docstring).
    """
    # Defaults must occupy a trailing run of fields, matching CPython's
    # NamedTupleMeta (a non-default field may not follow a default one).
    seen_default = None
    defs = []
    for name in fields:
        if name in defaults:
            seen_default = name
            defs.append(defaults[name])
        elif seen_default is not None:
            raise TypeError(
                "Non-default namedtuple field " + name +
                " cannot follow default field " + seen_default
            )
    cls = _collections.namedtuple(typename, fields, defaults=defs)
    # Class-body bookkeeping attrs that namedtuple already manages or that are
    # read-only on a type object; never copy these over.
    skip = {"__dict__", "__weakref__", "__annotations__", "__new__", "__slots__"}
    for key, value in namespace.items():
        if key in skip or key in fields:
            continue
        try:
            setattr(cls, key, value)
        except (AttributeError, TypeError):
            pass
    return cls


# Names that CPython's `_get_protocol_attrs` never treats as protocol members:
# typing/Protocol bookkeeping attrs and `object` infrastructure, not
# user-declared requirements (issue #2526).
_PROTOCOL_EXCLUDED_ATTRS = frozenset({
    "__init__", "__new__", "__init_subclass__", "__subclasshook__",
    "__class_getitem__", "__doc__", "__dict__", "__weakref__",
    "__abstractmethods__", "__protocol_attrs__",
    "__protocol_runtime_checkable__", "__non_callable_proto_members__",
    "__module__", "__qualname__",
    "__slots__", "__parameters__", "__orig_bases__", "__annotations__",
})


def _collect_protocol_attrs(cls):
    """Collect the member names a `@runtime_checkable` Protocol requires.

    Mirrors CPython 3.12's `typing._get_protocol_attrs`: the union of names
    declared in the protocol body and in any protocol *bases* (but not on
    `object` / `Protocol` / `Generic`), minus typing bookkeeping names.  For a
    simple protocol this is just the method/attribute names the user wrote,
    plus any annotation-only data members (`name: str`).
    """
    attrs = set()
    for base in getattr(cls, "__mro__", (cls,)):
        if getattr(base, "__name__", "") in ("Protocol", "Generic", "object"):
            continue
        for key in vars(base):
            if key not in _PROTOCOL_EXCLUDED_ATTRS:
                attrs.add(key)
        ann = getattr(base, "__annotations__", None)
        if isinstance(ann, dict):
            for key in ann:
                if key not in _PROTOCOL_EXCLUDED_ATTRS:
                    attrs.add(key)
    return attrs


def runtime_checkable(cls):
    """Mark a `Protocol` as runtime-checkable and record its member names.

    Sets `__protocol_runtime_checkable__` plus `__protocol_attrs__` — the set
    of attribute names a subject must have for a structural `isinstance` check
    to succeed (issue #2526) — and `__non_callable_proto_members__`, the subset
    of those attrs whose class value is not callable (data members).  The
    structural check itself lives in the `isinstance` builtin; it mirrors
    CPython 3.12, which treats a member resolved to `None` as absent unless the
    member is a declared non-callable.
    """
    cls.__protocol_runtime_checkable__ = True
    attrs = _collect_protocol_attrs(cls)
    cls.__protocol_attrs__ = attrs
    # Mirror CPython's `runtime_checkable`: a protocol attr whose class value is
    # not callable is a data member.  `isinstance` allows such a member to hold
    # `None` on the subject, but a callable (method) member resolved to `None`
    # is treated as absent.
    non_callable = set()
    for attr in attrs:
        if not callable(getattr(cls, attr, None)):
            non_callable.add(attr)
    cls.__non_callable_proto_members__ = non_callable
    return cls


def final(f):
    """`@final` decorator — runtime no-op marker."""
    try:
        f.__final__ = True
    except (AttributeError, TypeError):
        pass
    return f


def no_type_check(arg):
    """`@no_type_check` decorator — runtime no-op marker."""
    return arg


def reveal_type(obj):
    """Stub for static checkers: prints the runtime type and returns `obj`."""
    print(f"Runtime type is {type(obj).__name__!r}", file=_sys.stderr)
    return obj


def assert_never(arg):
    """`assert_never` — raises at runtime if ever reached."""
    raise AssertionError("Expected code to be unreachable, but got: " + repr(arg))


def assert_type(val, typ, /):
    """`assert_type` — runtime no-op that returns its first argument."""
    return val


def dataclass_transform(*args, **kwargs):
    """`@dataclass_transform()` decorator factory — runtime no-op marker."""

    def decorator(cls_or_fn):
        try:
            cls_or_fn.__dataclass_transform__ = {}
        except (AttributeError, TypeError):
            pass
        return cls_or_fn

    return decorator


def get_overloads(func):
    """Return registered `@overload` definitions (none, since pyrust drops them)."""
    return []


def clear_overloads():
    """Clear the overload registry — no-op in pyrust."""
    return None


class _SpecialMarker:
    """Lightweight stand-in for special forms that are only subscripted or
    used as annotations (`Self`, `Never`, `LiteralString`, `Annotated`, …)."""

    def __init__(self, name):
        self._name = name

    def __repr__(self):
        return "typing." + self._name

    def __getitem__(self, item):
        return self

    def __call__(self, *args, **kwargs):
        return self


def _type_repr(obj):
    """Render a type/metadata value as CPython's `typing._type_repr` would.

    Plain classes show as their (qualified) name rather than `<class '...'>`;
    everything else uses `repr`.  Used by `_AnnotatedAlias.__repr__`.
    """
    if isinstance(obj, type):
        module = getattr(obj, "__module__", None)
        qualname = getattr(obj, "__qualname__", obj.__name__)
        if module in (None, "builtins"):
            return qualname
        return module + "." + qualname
    return repr(obj)


class _AnnotatedAlias:
    """Runtime form of `Annotated[X, m1, m2, ...]` (PEP 593).

    Carries the annotated type as `__origin__`, the metadata tuple as
    `__metadata__`, and `(__origin__,)` as `__args__` (the metadata is *not*
    in `__args__`, matching CPython 3.12; `get_args` re-appends it) so that
    `get_origin`, `get_args`, and `repr` match CPython 3.12.
    """

    def __init__(self, origin, metadata):
        self.__origin__ = origin
        self.__metadata__ = tuple(metadata)
        self.__args__ = (origin,)

    def __repr__(self):
        meta = ", ".join(_type_repr(m) for m in self.__metadata__)
        return "typing.Annotated[" + _type_repr(self.__origin__) + ", " + meta + "]"

    def __eq__(self, other):
        if not isinstance(other, _AnnotatedAlias):
            return NotImplemented
        return (
            self.__origin__ == other.__origin__
            and self.__metadata__ == other.__metadata__
        )

    def __hash__(self):
        return hash((self.__origin__, self.__metadata__))


class _AnnotatedMarker:
    """Special form for `Annotated` (PEP 593).

    `Annotated[X, m1, ...]` builds an `_AnnotatedAlias`; it requires at least a
    type plus one metadata element, matching CPython's `TypeError`.  A nested
    `Annotated[Annotated[X, a], b]` is flattened to `Annotated[X, a, b]` at
    construction, mirroring CPython 3.12.
    """

    def __repr__(self):
        return "typing.Annotated"

    def __getitem__(self, params):
        if not isinstance(params, tuple) or len(params) < 2:
            raise TypeError(
                "Annotated[...] should be used with at least two arguments "
                "(a type and an annotation)."
            )
        origin = params[0]
        metadata = params[1:]
        # Flatten nested aliases: the underlying type collapses to its own
        # origin and its metadata is prepended (CPython `Annotated.__class_getitem__`).
        if isinstance(origin, _AnnotatedAlias):
            metadata = origin.__metadata__ + metadata
            origin = origin.__origin__
        return _AnnotatedAlias(origin, metadata)


Self = _SpecialMarker("Self")
Never = _SpecialMarker("Never")
LiteralString = _SpecialMarker("LiteralString")
Annotated = _AnnotatedMarker()
TypeAlias = _SpecialMarker("TypeAlias")
Concatenate = _SpecialMarker("Concatenate")
Unpack = _SpecialMarker("Unpack")
Required = _SpecialMarker("Required")
NotRequired = _SpecialMarker("NotRequired")
TypeGuard = _SpecialMarker("TypeGuard")


class ParamSpec:
    """Minimal `ParamSpec` stub: stores its name and exposes `args`/`kwargs`."""

    def __init__(self, name, *, bound=None, covariant=False, contravariant=False):
        self.__name__ = name
        self.__bound__ = bound

    @property
    def args(self):
        return self

    @property
    def kwargs(self):
        return self

    def __repr__(self):
        return "~" + self.__name__


class TypeVarTuple:
    """Minimal `TypeVarTuple` stub: stores its name."""

    def __init__(self, name):
        self.__name__ = name

    def __iter__(self):
        return iter((Unpack[self],))

    def __repr__(self):
        return self.__name__
