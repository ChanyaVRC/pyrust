# Python-source members of the `typing` module (issue #2516, #3231).
#
# These names are most naturally expressed in Python and are exec'd once into
# a throwaway namespace at first import of `typing`; the resulting public names
# are copied onto the module by `inject_python_members`.  The native special
# forms (`List`, `Optional`, `Union`, `Generic`, `Protocol`, `TypeVar`, …) are
# already present on the module before this source runs, so the helpers below
# may reference them via the names the exec namespace is pre-seeded with.

import collections as _collections
import collections.abc as _collections_abc
import sys as _sys
import types as _types


def get_origin(tp):
    """Return the unsubscripted origin of a typing/PEP 585 generic.

    `get_origin(List[int])` is `list`, `get_origin(Optional[str])` is `Union`,
    `get_origin(Callable[[int], str])` is `collections.abc.Callable`,
    `get_origin(int)` is `None`.  Mirrors CPython's `typing.get_origin`.
    """
    # `Annotated[X, ...]` reports the `Annotated` marker as its origin, even
    # though it stores `X` in `__origin__` (PEP 593 / CPython `get_origin`).
    if isinstance(tp, _AnnotatedAlias):
        return Annotated
    # PEP 604 union (`int | str`): a `types.UnionType` instance has no
    # `__origin__`, but CPython's `get_origin` reports `types.UnionType` itself.
    if isinstance(tp, _types.UnionType):
        return _types.UnionType
    origin = getattr(tp, "__origin__", None)
    if origin is None:
        return None
    # The native special-form aliases carry the sentinel class as their
    # `__origin__`.  Normalise to match CPython's observable origins.
    if origin is Optional or origin is Union:
        return Union
    if origin is Callable:
        return _collections_abc.Callable
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


def _resolve(value, globalns, localns):
    """Resolve a single annotation: eval string forward refs, normalize None."""
    if value is None:
        return type(None)
    if isinstance(value, str):
        try:
            resolved = eval(value, globalns, localns)
            return type(None) if resolved is None else resolved
        except Exception:
            return value
    return value


def _strip_extras(value):
    """Strip `Annotated[X, ...]`, `Required[X]`, and `NotRequired[X]` down to `X`.

    Used by `get_type_hints` when `include_extras=False`, mirroring CPython 3.12.
    """
    if isinstance(value, _AnnotatedAlias):
        return _strip_extras(value.__origin__)
    if isinstance(value, _SpecialFormAlias) and value.__origin__ in (Required, NotRequired):
        return _strip_extras(value.__args__[0])
    return value


def get_type_hints(obj, globalns=None, localns=None, include_extras=False):
    """Return a dict of type hints for a function, class, or module.

    String (forward-reference) annotations are evaluated against `globalns` and
    `localns`.  For classes, annotations are collected across the MRO with base
    classes contributing first (subclass annotations win on conflict).  Unless
    `include_extras` is set, `Annotated[X, ...]`, `Required[X]`, and
    `NotRequired[X]` hints are stripped to `X`.
    """
    if isinstance(obj, type):
        hints = {}
        for base in reversed(obj.__mro__):
            if globalns is None:
                module = getattr(base, "__module__", None)
                mod = _sys.modules.get(module) if module else None
                base_globals = getattr(mod, "__dict__", {}) if mod else {}
            else:
                base_globals = globalns
            base_locals = base_globals if localns is None else localns
            ann = base.__dict__.get("__annotations__", {})
            for name, value in ann.items():
                hints[name] = _resolve(value, base_globals, base_locals)
    else:
        ann = getattr(obj, "__annotations__", None)
        if ann is None:
            return {}
        if globalns is None:
            if isinstance(obj, _types.ModuleType):
                globalns = obj.__dict__
            else:
                globalns = getattr(obj, "__globals__", {})
        if localns is None:
            localns = globalns
        hints = {name: _resolve(value, globalns, localns) for name, value in ann.items()}

    if not include_extras:
        hints = {name: _strip_extras(value) for name, value in hints.items()}
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


def _typeddict_field_qualifier(tp):
    """Return `'required'`, `'optional'`, or `None` for a TypedDict field annotation."""
    while isinstance(tp, _AnnotatedAlias):
        tp = tp.__origin__
    origin = get_origin(tp)
    if origin is Required:
        return "required"
    if origin is NotRequired:
        return "optional"
    return None


class _TypedDictMeta(type):
    """Metaclass for `TypedDict` classes (PEP 589 / PEP 655, issues #2718, #3231)."""

    def __new__(mcls, name, bases, ns, total=True):
        own = dict(ns.get("__annotations__", {}))

        merged = {}
        required = set()
        optional = set()
        # Merge ancestor TypedDict fields first; own fields override on conflict.
        for base in bases:
            merged.update(getattr(base, "__annotations__", {}))
            base_required = getattr(base, "__required_keys__", ())
            required.update(base_required)
            optional.difference_update(base_required)
            base_optional = getattr(base, "__optional_keys__", ())
            required.difference_update(base_optional)
            optional.update(base_optional)

        merged.update(own)
        for key, ann in own.items():
            qual = _typeddict_field_qualifier(ann)
            if qual == "required":
                required.add(key)
                optional.discard(key)
            elif qual == "optional":
                optional.add(key)
                required.discard(key)
            elif total:
                required.add(key)
                optional.discard(key)
            else:
                optional.add(key)
                required.discard(key)

        ns["__annotations__"] = merged
        ns["__required_keys__"] = frozenset(required)
        ns["__optional_keys__"] = frozenset(optional)
        ns["__total__"] = bool(total)
        # CPython collapses the bases to `(dict,)`: a TypedDict is an annotation
        # type, so the runtime class is a plain dict subclass and the ancestor
        # TypedDicts do not appear in the MRO (their fields are merged above).
        return super().__new__(mcls, name, (dict,), ns)

    def __call__(cls, *args, **kwargs):
        return dict(*args, **kwargs)

    def __subclasscheck__(cls, other):
        # A TypedDict is an annotation type, not a runtime class; CPython's
        # `_TypedDictMeta.__subclasscheck__` rejects `issubclass()` (issue #2738).
        raise TypeError("TypedDict does not support instance and class checks")

    __instancecheck__ = __subclasscheck__


def is_typeddict(tp):
    """Check if an annotation is a TypedDict class (Python 3.10+)."""
    return _TypedDictMeta in getattr(type(tp), "__mro__", ())


def _build_typeddict_class(typename, annotations, total=True):
    """Build a TypedDict class from its annotations (issue #2718)."""
    namespace = {"__annotations__": dict(annotations)}
    return _TypedDictMeta(typename, (dict,), namespace, total=total)


def _typeddict_functional(typename, fields=None, /, *, total=True, **kwargs):
    """Functional form of `typing.TypedDict` (issue #2718)."""
    if fields is None:
        fields = kwargs
    elif kwargs:
        raise TypeError(
            "TypedDict takes either a dict or keyword arguments, but not both"
        )
    return _build_typeddict_class(typename, fields, total)


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
    """Collect the member names a `@runtime_checkable` Protocol requires."""
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
    """Mark a `Protocol` as runtime-checkable and record its member names."""
    cls.__protocol_runtime_checkable__ = True
    attrs = _collect_protocol_attrs(cls)
    cls.__protocol_attrs__ = attrs
    non_callable = set()
    for attr in attrs:
        if not callable(getattr(cls, attr, None)):
            non_callable.add(attr)
    cls.__non_callable_proto_members__ = non_callable
    return cls


def final(f):
    """`@final` decorator — sets `__final__ = True` and returns `f`."""
    try:
        f.__final__ = True
    except (AttributeError, TypeError):
        pass
    return f


def override(arg, /):
    """`@override` decorator (PEP 698, Python 3.12) — sets `__override__ = True`."""
    try:
        arg.__override__ = True
    except (AttributeError, TypeError):
        pass
    return arg


def no_type_check(arg):
    """`@no_type_check` decorator — sets `__no_type_check__ = True`."""
    if isinstance(arg, type):
        for obj in arg.__dict__.values():
            if isinstance(obj, _types.FunctionType):
                obj.__no_type_check__ = True
    else:
        try:
            arg.__no_type_check__ = True
        except (AttributeError, TypeError):
            pass
    return arg


def no_type_check_decorator(decorator):
    """Decorator to give another decorator the `no_type_check()` effect."""
    def wrapped_decorator(*args, **kwds):
        func = decorator(*args, **kwds)
        return no_type_check(func)
    return wrapped_decorator


def reveal_type(obj, /):
    """Stub for static checkers: prints the runtime type and returns `obj`."""
    print(f"Runtime type is {type(obj).__name__!r}", file=_sys.stderr)
    return obj


def assert_never(arg, /):
    """`assert_never` — raises at runtime if ever reached."""
    raise AssertionError("Expected code to be unreachable, but got: " + repr(arg))


def assert_type(val, typ, /):
    """`assert_type` — runtime no-op that returns its first argument."""
    return val


def dataclass_transform(
    *,
    eq_default=True,
    order_default=False,
    kw_only_default=False,
    frozen_default=False,
    field_specifiers=(),
    **kwargs,
):
    """`@dataclass_transform()` decorator factory (PEP 681, Python 3.11)."""

    def decorator(cls_or_fn):
        try:
            cls_or_fn.__dataclass_transform__ = {
                "eq_default": eq_default,
                "order_default": order_default,
                "kw_only_default": kw_only_default,
                "frozen_default": frozen_default,
                "field_specifiers": field_specifiers,
                "kwargs": kwargs,
            }
        except (AttributeError, TypeError):
            pass
        return cls_or_fn

    return decorator


_overload_registry = {}


def _overload_dummy(*args, **kwds):
    """Helper for `@overload` to raise when called directly."""
    raise NotImplementedError(
        "You should not call an overloaded function. "
        "A series of @overload-decorated functions "
        "outside a stub module should always be followed "
        "by an implementation that is not @overload-ed."
    )


def overload(func):
    """Decorator for overloaded functions/methods (Python 3.11+ registry)."""
    f = getattr(func, "__func__", func)
    try:
        key = (f.__module__, f.__qualname__)
        _overload_registry.setdefault(key, []).append(func)
    except AttributeError:
        pass
    return _overload_dummy


def get_overloads(func):
    """Return a sequence of `@overload`-decorated definitions for `func`."""
    f = getattr(func, "__func__", func)
    try:
        key = (f.__module__, f.__qualname__)
        return list(_overload_registry.get(key, ()))
    except AttributeError:
        return []


def clear_overloads():
    """Clear all registered overloads."""
    _overload_registry.clear()


class NewType:
    """NewType creates simple unique types with almost zero runtime overhead."""

    def __init__(self, name, tp):
        self.__qualname__ = name
        self.__name__ = name
        self.__supertype__ = tp
        try:
            frame = _sys._getframe(1)
            mod = frame.f_globals.get("__name__", "__main__")
        except (AttributeError, ValueError):
            mod = "__main__"
        self.__module__ = mod

    def __call__(self, x, /):
        return x

    def __mro_entries__(self, bases):
        raise TypeError(
            "Cannot subclass an instance of NewType. "
            "Perhaps you were looking for: https://peps.python.org/pep-0484/#newtype-helper-function"
        )

    def __repr__(self):
        return f"{self.__module__}.{self.__qualname__}"

    def __or__(self, other):
        return Union[self, other]

    def __ror__(self, other):
        return Union[other, self]


def _type_repr(obj):
    """Render a type/metadata value as CPython's `typing._type_repr` would."""
    if obj is Ellipsis:
        return "..."
    if isinstance(obj, type):
        module = getattr(obj, "__module__", None)
        qualname = getattr(obj, "__qualname__", obj.__name__)
        if module in (None, "builtins"):
            return qualname
        return module + "." + qualname
    return repr(obj)


def _collect_type_params(args):
    params = []
    for a in args:
        if isinstance(a, (TypeVar, ParamSpec, TypeVarTuple)):
            if a not in params:
                params.append(a)
        else:
            for p in getattr(a, "__parameters__", ()):
                if p not in params:
                    params.append(p)
    return tuple(params)


class _SpecialForm:
    """Non-subscriptable typing special form (`Self`, `Never`, `LiteralString`, …)."""

    def __init__(self, name):
        self._name = name

    def __repr__(self):
        return "typing." + self._name

    def __getitem__(self, item):
        raise TypeError(f"{self!r} is not subscriptable")

    def __call__(self, *args, **kwargs):
        raise TypeError(f"Cannot instantiate {self!r}")

    def __or__(self, other):
        return Union[self, other]

    def __ror__(self, other):
        return Union[other, self]


class _SpecialFormAlias:
    """Subscripted alias for `Required`, `NotRequired`, `TypeGuard`, `Concatenate`."""

    def __init__(self, origin, args):
        self.__origin__ = origin
        self.__args__ = tuple(args)
        self.__parameters__ = _collect_type_params(self.__args__)

    def __repr__(self):
        rendered = ", ".join(_type_repr(a) for a in self.__args__)
        return f"typing.{self.__origin__._name}[{rendered}]"

    def __eq__(self, other):
        if not isinstance(other, _SpecialFormAlias):
            return NotImplemented
        return (
            self.__origin__ == other.__origin__
            and self.__args__ == other.__args__
        )

    def __hash__(self):
        return hash((self.__origin__, self.__args__))

    def __or__(self, other):
        return Union[self, other]

    def __ror__(self, other):
        return Union[other, self]


class _SingleArgSpecialForm(_SpecialForm):
    """Special form accepting exactly one type argument (`Required`, `NotRequired`, `TypeGuard`)."""

    def __getitem__(self, item):
        if isinstance(item, tuple):
            raise TypeError(f"{self!r} accepts only single type.")
        if item is None:
            item = type(None)
        return _SpecialFormAlias(self, (item,))


class _ConcatenateForm(_SpecialForm):
    """Special form for `Concatenate[T1, ..., ParamSpec]` (PEP 612)."""

    def __getitem__(self, parameters):
        if not isinstance(parameters, tuple):
            parameters = (parameters,)
        if len(parameters) < 2:
            if not isinstance(parameters[-1], ParamSpec) and parameters[-1] is not Ellipsis:
                raise TypeError(
                    "The last parameter to Concatenate should be a ParamSpec variable or ellipsis."
                )
            raise TypeError("Concatenate takes at least two arguments")
        if not isinstance(parameters[-1], ParamSpec) and parameters[-1] is not Ellipsis:
            raise TypeError(
                "The last parameter to Concatenate should be a ParamSpec variable or ellipsis."
            )
        norm = tuple(type(None) if p is None else p for p in parameters)
        return _SpecialFormAlias(self, norm)


class _AnnotatedAlias:
    """Runtime form of `Annotated[X, m1, m2, ...]` (PEP 593)."""

    def __init__(self, origin, metadata):
        self.__origin__ = origin
        self.__metadata__ = tuple(metadata)
        self.__args__ = (origin,)
        self.__parameters__ = _collect_type_params((origin,))

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

    def __or__(self, other):
        return Union[self, other]

    def __ror__(self, other):
        return Union[other, self]


class _AnnotatedMarker:
    """Special form for `Annotated` (PEP 593)."""

    def __repr__(self):
        return "typing.Annotated"

    def __getitem__(self, params):
        if not isinstance(params, tuple) or len(params) < 2:
            raise TypeError(
                "Annotated[...] should be used with at least two arguments "
                "(a type and an annotation)."
            )
        origin = params[0]
        if origin is None:
            origin = type(None)
        metadata = params[1:]
        if isinstance(origin, _AnnotatedAlias):
            metadata = origin.__metadata__ + metadata
            origin = origin.__origin__
        return _AnnotatedAlias(origin, metadata)


class _UnpackGenericAlias:
    """Runtime form of `Unpack[X]` (PEP 646)."""

    def __init__(self, origin, args):
        self.__origin__ = origin
        self.__args__ = args
        self.__parameters__ = _collect_type_params(args)

    def __repr__(self):
        return "typing.Unpack[" + _type_repr(self.__args__[0]) + "]"

    def __eq__(self, other):
        if not isinstance(other, _UnpackGenericAlias):
            return NotImplemented
        return (
            self.__origin__ == other.__origin__
            and self.__args__ == other.__args__
        )

    def __hash__(self):
        return hash((self.__origin__, self.__args__))


class _UnpackMarker:
    """Special form for `Unpack` (PEP 646)."""

    def __repr__(self):
        return "typing.Unpack"

    def __getitem__(self, item):
        if isinstance(item, tuple):
            raise TypeError("Unpack[...] accepts only single type.")
        return _UnpackGenericAlias(self, (item,))


Self = _SpecialForm("Self")
Never = _SpecialForm("Never")
NoReturn = _SpecialForm("NoReturn")
LiteralString = _SpecialForm("LiteralString")
Annotated = _AnnotatedMarker()
TypeAlias = _SpecialForm("TypeAlias")
Concatenate = _ConcatenateForm("Concatenate")
Unpack = _UnpackMarker()
Required = _SingleArgSpecialForm("Required")
NotRequired = _SingleArgSpecialForm("NotRequired")
TypeGuard = _SingleArgSpecialForm("TypeGuard")
