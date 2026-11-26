# Python-source members of the `typing` module (issue #2516, #3231).
#
# These names are most naturally expressed in Python and are exec'd once into
# a throwaway namespace at first import of `typing`; the resulting public names
# are copied onto the module by `inject_python_members`.  The native special
# forms (`List`, `Optional`, `Union`, `Generic`, `Protocol`, `TypeVar`, …) are
# already present on the module before this source runs, so the helpers below
# may reference them via the names the exec namespace is pre-seeded with
# (`TYPING_PY_SEEDS` in `typing/python_members.rs`).  That includes the native
# `_SpecialGenericAlias` class used to build `Iterable`, `Mapping`, … below
# (issue #3326).

import collections as _collections
import collections.abc as _collections_abc
import contextlib as _contextlib
import re as _stdlib_re
import sys as _sys
import types as _types
from abc import abstractmethod as _abstractmethod

# Please keep __all__ alphabetized within each category (CPython 3.12.3).
__all__ = [
    # Super-special typing primitives.
    'Annotated',
    'Any',
    'Callable',
    'ClassVar',
    'Concatenate',
    'Final',
    'ForwardRef',
    'Generic',
    'Literal',
    'Optional',
    'ParamSpec',
    'Protocol',
    'Tuple',
    'Type',
    'TypeVar',
    'TypeVarTuple',
    'Union',

    # ABCs (from collections.abc).
    'AbstractSet',  # collections.abc.Set.
    'ByteString',
    'Container',
    'ContextManager',
    'Hashable',
    'ItemsView',
    'Iterable',
    'Iterator',
    'KeysView',
    'Mapping',
    'MappingView',
    'MutableMapping',
    'MutableSequence',
    'MutableSet',
    'Sequence',
    'Sized',
    'ValuesView',
    'Awaitable',
    'AsyncIterator',
    'AsyncIterable',
    'Coroutine',
    'Collection',
    'AsyncGenerator',
    'AsyncContextManager',

    # Structural checks, a.k.a. protocols.
    'Reversible',
    'SupportsAbs',
    'SupportsBytes',
    'SupportsComplex',
    'SupportsFloat',
    'SupportsIndex',
    'SupportsInt',
    'SupportsRound',

    # Concrete collection types.
    'ChainMap',
    'Counter',
    'Deque',
    'Dict',
    'DefaultDict',
    'List',
    'OrderedDict',
    'Set',
    'FrozenSet',
    'NamedTuple',  # Not really a type.
    'TypedDict',  # Not really a type.
    'Generator',

    # Other concrete types.
    'BinaryIO',
    'IO',
    'Match',
    'Pattern',
    'TextIO',

    # One-off things.
    'AnyStr',
    'assert_type',
    'assert_never',
    'cast',
    'clear_overloads',
    'dataclass_transform',
    'final',
    'get_args',
    'get_origin',
    'get_overloads',
    'get_type_hints',
    'is_typeddict',
    'LiteralString',
    'Never',
    'NewType',
    'no_type_check',
    'no_type_check_decorator',
    'NoReturn',
    'NotRequired',
    'overload',
    'override',
    'ParamSpecArgs',
    'ParamSpecKwargs',
    'Required',
    'reveal_type',
    'runtime_checkable',
    'Self',
    'Text',
    'TYPE_CHECKING',
    'TypeAlias',
    'TypeGuard',
    'TypeAliasType',
    'Unpack',
]


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
    if isinstance(value, ForwardRef):
        try:
            return value._evaluate(globalns, localns, recursive_guard=frozenset())
        except Exception:
            return value
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
# typing/Protocol bookkeeping attrs (`_TYPING_INTERNALS`) and `object`
# infrastructure (`_SPECIAL_NAMES`), not user-declared requirements (issues
# #2526, #3326).  Names starting with `_abc_` are skipped as well.
_PROTOCOL_EXCLUDED_ATTRS = frozenset({
    "__init__", "__new__", "__init_subclass__", "__subclasshook__",
    "__class_getitem__", "__doc__", "__dict__", "__weakref__",
    "__abstractmethods__", "__protocol_attrs__",
    "__protocol_runtime_checkable__", "__non_callable_proto_members__",
    "__module__", "__qualname__",
    "__slots__", "__parameters__", "__orig_bases__", "__annotations__",
    "__orig_class__", "__type_params__", "_is_protocol", "_is_runtime_protocol",
    "_MutableMapping__marker",
})


def _collect_protocol_attrs(cls):
    """Collect the member names a `@runtime_checkable` Protocol requires."""
    attrs = set()
    for base in getattr(cls, "__mro__", (cls,)):
        if getattr(base, "__name__", "") in ("Protocol", "Generic", "object"):
            continue
        for key in vars(base):
            if key not in _PROTOCOL_EXCLUDED_ATTRS and not key.startswith("_abc_"):
                attrs.add(key)
        ann = getattr(base, "__annotations__", None)
        if isinstance(ann, dict):
            for key in ann:
                if key not in _PROTOCOL_EXCLUDED_ATTRS and not key.startswith("_abc_"):
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


class ForwardRef:
    """Internal wrapper to hold a forward reference."""

    __slots__ = ('__forward_arg__', '__forward_code__',
                 '__forward_evaluated__', '__forward_value__',
                 '__forward_is_argument__', '__forward_is_class__',
                 '__forward_module__')

    def __init__(self, arg, is_argument=True, module=None, *, is_class=False):
        if not isinstance(arg, str):
            raise TypeError(f"Forward reference must be a string -- got {arg!r}")
        # `def f(*args: *Ts)` produces `arg = '*Ts'`, which is not a valid
        # expression on its own, so unpack it manually.
        if arg[0] == '*':
            arg_to_compile = f'({arg},)[0]'
        else:
            arg_to_compile = arg
        try:
            code = compile(arg_to_compile, '<string>', 'eval')
        except SyntaxError:
            raise SyntaxError(f"Forward reference must be an expression -- got {arg!r}")

        self.__forward_arg__ = arg
        self.__forward_code__ = code
        self.__forward_evaluated__ = False
        self.__forward_value__ = None
        self.__forward_is_argument__ = is_argument
        self.__forward_is_class__ = is_class
        self.__forward_module__ = module

    def _evaluate(self, globalns, localns, type_params=None, *, recursive_guard):
        if self.__forward_arg__ in recursive_guard:
            return self
        if not self.__forward_evaluated__ or localns is not globalns:
            if globalns is None and localns is None:
                globalns = localns = {}
            elif globalns is None:
                globalns = localns
            elif localns is None:
                localns = globalns
            if self.__forward_module__ is not None:
                globalns = getattr(
                    _sys.modules.get(self.__forward_module__, None), '__dict__', globalns
                )
            value = eval(self.__forward_code__, globalns, localns)
            self.__forward_value__ = type(None) if value is None else value
            self.__forward_evaluated__ = True
        return self.__forward_value__

    def __eq__(self, other):
        if not isinstance(other, ForwardRef):
            return NotImplemented
        if self.__forward_evaluated__ and other.__forward_evaluated__:
            return (self.__forward_arg__ == other.__forward_arg__ and
                    self.__forward_value__ == other.__forward_value__)
        return (self.__forward_arg__ == other.__forward_arg__ and
                self.__forward_module__ == other.__forward_module__)

    def __hash__(self):
        return hash((self.__forward_arg__, self.__forward_module__))

    def __or__(self, other):
        return Union[self, other]

    def __ror__(self, other):
        return Union[other, self]

    def __repr__(self):
        if self.__forward_module__ is None:
            module_repr = ''
        else:
            module_repr = f', module={self.__forward_module__!r}'
        return f'ForwardRef({self.__forward_arg__!r}{module_repr})'


# A useful type variable with constraints.  This represents string types.
AnyStr = TypeVar('AnyStr', bytes, str)
AnyStr.__module__ = 'typing'

# Python-version-specific alias (Python 2: unicode; Python 3: str).
Text = str


# Various ABCs mimicking those in collections.abc (CPython 3.12.3 order).
# `_SpecialGenericAlias` is the native class from `typing/special_aliases.rs`;
# its second argument is the number of type parameters the alias accepts.
_alias = _SpecialGenericAlias

Hashable = _alias(_collections_abc.Hashable, 0)  # Not generic.
Awaitable = _alias(_collections_abc.Awaitable, 1)
Coroutine = _alias(_collections_abc.Coroutine, 3)
AsyncIterable = _alias(_collections_abc.AsyncIterable, 1)
AsyncIterator = _alias(_collections_abc.AsyncIterator, 1)
Iterable = _alias(_collections_abc.Iterable, 1)
Iterator = _alias(_collections_abc.Iterator, 1)
Reversible = _alias(_collections_abc.Reversible, 1)
Sized = _alias(_collections_abc.Sized, 0)  # Not generic.
Container = _alias(_collections_abc.Container, 1)
Collection = _alias(_collections_abc.Collection, 1)
AbstractSet = _alias(_collections_abc.Set, 1, name='AbstractSet')
MutableSet = _alias(_collections_abc.MutableSet, 1)
Mapping = _alias(_collections_abc.Mapping, 2)
MutableMapping = _alias(_collections_abc.MutableMapping, 2)
Sequence = _alias(_collections_abc.Sequence, 1)
MutableSequence = _alias(_collections_abc.MutableSequence, 1)
ByteString = _DeprecatedGenericAlias(
    _collections_abc.ByteString, 0, removal_version=(3, 14)  # Not generic.
)
Deque = _alias(_collections.deque, 1, name='Deque')
MappingView = _alias(_collections_abc.MappingView, 1)
KeysView = _alias(_collections_abc.KeysView, 1)
ItemsView = _alias(_collections_abc.ItemsView, 2)
ValuesView = _alias(_collections_abc.ValuesView, 1)
ContextManager = _alias(_contextlib.AbstractContextManager, 1, name='ContextManager')
AsyncContextManager = _alias(
    _contextlib.AbstractAsyncContextManager, 1, name='AsyncContextManager'
)
DefaultDict = _alias(_collections.defaultdict, 2, name='DefaultDict')
OrderedDict = _alias(_collections.OrderedDict, 2)
Counter = _alias(_collections.Counter, 1)
ChainMap = _alias(_collections.ChainMap, 2)
Generator = _alias(_collections_abc.Generator, 3)
AsyncGenerator = _alias(_collections_abc.AsyncGenerator, 2)
Pattern = _alias(_stdlib_re.Pattern, 1)
Match = _alias(_stdlib_re.Match, 1)

del _alias


@runtime_checkable
class SupportsInt(Protocol):
    """An ABC with one abstract method __int__."""

    __slots__ = ()

    @_abstractmethod
    def __int__(self) -> int:
        pass


@runtime_checkable
class SupportsFloat(Protocol):
    """An ABC with one abstract method __float__."""

    __slots__ = ()

    @_abstractmethod
    def __float__(self) -> float:
        pass


@runtime_checkable
class SupportsComplex(Protocol):
    """An ABC with one abstract method __complex__."""

    __slots__ = ()

    @_abstractmethod
    def __complex__(self) -> complex:
        pass


@runtime_checkable
class SupportsBytes(Protocol):
    """An ABC with one abstract method __bytes__."""

    __slots__ = ()

    @_abstractmethod
    def __bytes__(self) -> bytes:
        pass


@runtime_checkable
class SupportsIndex(Protocol):
    """An ABC with one abstract method __index__."""

    __slots__ = ()

    @_abstractmethod
    def __index__(self) -> int:
        pass


@runtime_checkable
class SupportsAbs[T](Protocol):
    """An ABC with one abstract method __abs__ that is covariant in its return type."""

    __slots__ = ()

    @_abstractmethod
    def __abs__(self) -> T:
        pass


@runtime_checkable
class SupportsRound[T](Protocol):
    """An ABC with one abstract method __round__ that is covariant in its return type."""

    __slots__ = ()

    @_abstractmethod
    def __round__(self, ndigits: int = 0) -> T:
        pass


class IO(Generic[AnyStr]):
    """Generic base class for TextIO and BinaryIO.

    This is an abstract, generic version of the return of open().

    NOTE: This does not distinguish between the different possible
    classes (text vs. binary, read vs. write vs. read/write,
    append-only, unbuffered).  The TextIO and BinaryIO subclasses
    below capture the distinctions between text vs. binary, which is
    pervasive in the interface; however we currently do not offer a
    way to track the other distinctions in the type system.
    """

    __slots__ = ()

    @property
    @_abstractmethod
    def mode(self) -> str:
        pass

    @property
    @_abstractmethod
    def name(self) -> str:
        pass

    @_abstractmethod
    def close(self) -> None:
        pass

    @property
    @_abstractmethod
    def closed(self) -> bool:
        pass

    @_abstractmethod
    def fileno(self) -> int:
        pass

    @_abstractmethod
    def flush(self) -> None:
        pass

    @_abstractmethod
    def isatty(self) -> bool:
        pass

    @_abstractmethod
    def read(self, n: int = -1) -> AnyStr:
        pass

    @_abstractmethod
    def readable(self) -> bool:
        pass

    @_abstractmethod
    def readline(self, limit: int = -1) -> AnyStr:
        pass

    @_abstractmethod
    def readlines(self, hint: int = -1) -> List[AnyStr]:
        pass

    @_abstractmethod
    def seek(self, offset: int, whence: int = 0) -> int:
        pass

    @_abstractmethod
    def seekable(self) -> bool:
        pass

    @_abstractmethod
    def tell(self) -> int:
        pass

    @_abstractmethod
    def truncate(self, size: int = None) -> int:
        pass

    @_abstractmethod
    def writable(self) -> bool:
        pass

    @_abstractmethod
    def write(self, s: AnyStr) -> int:
        pass

    @_abstractmethod
    def writelines(self, lines: List[AnyStr]) -> None:
        pass

    @_abstractmethod
    def __enter__(self) -> 'IO[AnyStr]':
        pass

    @_abstractmethod
    def __exit__(self, type, value, traceback) -> None:
        pass


class BinaryIO(IO[bytes]):
    """Typed version of the return of open() in binary mode."""

    __slots__ = ()

    @_abstractmethod
    def write(self, s: Union[bytes, bytearray]) -> int:
        pass

    @_abstractmethod
    def __enter__(self) -> 'BinaryIO':
        pass


class TextIO(IO[str]):
    """Typed version of the return of open() in text mode."""

    __slots__ = ()

    @property
    @_abstractmethod
    def buffer(self) -> BinaryIO:
        pass

    @property
    @_abstractmethod
    def encoding(self) -> str:
        pass

    @property
    @_abstractmethod
    def errors(self) -> Optional[str]:
        pass

    @property
    @_abstractmethod
    def line_buffering(self) -> bool:
        pass

    @property
    @_abstractmethod
    def newlines(self) -> Any:
        pass

    @_abstractmethod
    def __enter__(self) -> 'TextIO':
        pass
