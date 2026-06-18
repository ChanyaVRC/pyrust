# Python-level implementation of the `enum` module, injected onto the native
# `enum` module by `enum_mod.rs::inject_python_members` (mirrors the
# `operator` / `string` modules).
#
# A close port of the core of CPython 3.12's `Lib/enum.py`, restricted to the
# minimum viable surface: `Enum`, `IntEnum`, `EnumMeta` (aka `EnumType`), and
# `auto`.  `Flag`, `StrEnum`, `_generate_next_value_` overrides, functional
# construction, pickling support, and `_missing_` are intentionally omitted.
#
# Reference: <https://docs.python.org/3/library/enum.html>


class auto:
    """Placeholder for a value auto-assigned by ``EnumMeta`` (1, 2, 3, ...)."""

    def __init__(self):
        self.value = None


class _EnumDict(dict):
    """Class-body namespace returned by ``EnumMeta.__prepare__``.

    Records member assignments in definition order and resolves ``auto()``
    sentinels to the next integer as they are stored, mirroring CPython's
    ``_EnumDict``.
    """

    def __init__(self):
        super().__init__()
        self._member_names = []
        self._last_value = 0

    def __setitem__(self, key, value):
        if not _is_sunder(key) and not _is_dunder(key) and not _is_descriptor(value):
            # A genuine enum member.
            if isinstance(value, auto):
                self._last_value += 1
                value = self._last_value
            else:
                self._last_value = value
            self._member_names.append(key)
        super().__setitem__(key, value)


def _is_dunder(name):
    return (
        len(name) > 4
        and name[:2] == "__"
        and name[-2:] == "__"
        and name[2] != "_"
        and name[-3] != "_"
    )


def _is_sunder(name):
    return (
        len(name) > 2
        and name[0] == "_"
        and name[-1] == "_"
        and name[1] != "_"
        and name[-2] != "_"
    )


def _is_descriptor(obj):
    return (
        hasattr(obj, "__get__")
        or hasattr(obj, "__set__")
        or hasattr(obj, "__delete__")
    )


class EnumType(type):
    """Metaclass that turns plain class-body assignments into enum members."""

    @classmethod
    def __prepare__(mcls, cls, bases, **kwds):
        return _EnumDict()

    def __new__(mcls, cls, bases, classdict, **kwds):
        member_names = classdict._member_names
        # Strip the members out of the namespace handed to ``type.__new__``;
        # they are re-attached below as fully-formed member instances.
        ns = {}
        for key, value in classdict.items():
            if key not in member_names:
                ns[key] = value

        enum_class = super().__new__(mcls, cls, bases, ns)
        enum_class._member_names_ = []
        enum_class._member_map_ = {}
        enum_class._value2member_map_ = {}

        for name in member_names:
            value = classdict[name]
            # First-seen value wins as the canonical member; a later member with
            # an existing value is an *alias*: it reuses the canonical member,
            # is accessible by name (`Shape.DIAMOND`) and via `_member_map_`,
            # but is not iterated and does not appear in `_member_names_` (it is
            # `Shape.SQUARE`).  Matches CPython's alias semantics.
            if value in enum_class._value2member_map_:
                member = enum_class._value2member_map_[value]
            else:
                member = enum_class._new_member_(name, value)
                enum_class._member_names_.append(name)
                enum_class._value2member_map_[value] = member
            enum_class._member_map_[name] = member
            setattr(enum_class, name, member)

        return enum_class

    def __call__(cls, value):
        # ``Color(value)`` performs a value lookup, returning the existing
        # member.  Calling the bare ``Enum`` base (no members) is not supported.
        try:
            return cls._value2member_map_[value]
        except KeyError:
            raise ValueError("%r is not a valid %s" % (value, cls.__name__))

    def __getitem__(cls, name):
        return cls._member_map_[name]

    def __iter__(cls):
        return (cls._member_map_[name] for name in cls._member_names_)

    def __len__(cls):
        return len(cls._member_names_)

    def __contains__(cls, member):
        if isinstance(member, cls):
            return True
        return member in cls._value2member_map_

    def __repr__(cls):
        return "<enum %r>" % cls.__name__


# CPython 3.12 renamed the metaclass to ``EnumType`` and keeps ``EnumMeta`` as
# a backwards-compatible alias, so ``type(Color).__name__ == 'EnumType'``.
EnumMeta = EnumType


class Enum(metaclass=EnumMeta):
    """Base class for creating enumerated constants."""

    @classmethod
    def _new_member_(cls, name, value):
        member = object.__new__(cls)
        member._name_ = name
        member._value_ = value
        return member

    @property
    def name(self):
        return self._name_

    @property
    def value(self):
        return self._value_

    def __repr__(self):
        return "<%s.%s: %r>" % (self.__class__.__name__, self._name_, self._value_)

    def __str__(self):
        return "%s.%s" % (self.__class__.__name__, self._name_)

    def __hash__(self):
        return hash(self._name_)


class IntEnum(int, Enum):
    """Enum where members are also (and comparable to) ints.

    Like CPython's ``ReprEnum``-based ``IntEnum``, the ``str`` / ``format`` of a
    member is the underlying ``int``'s (``str(Status.OK) == '200'``), while
    ``repr`` keeps the ``<Status.OK: 200>`` form from ``Enum``.
    """

    @classmethod
    def _new_member_(cls, name, value):
        member = int.__new__(cls, value)
        member._name_ = name
        member._value_ = value
        return member

    def __str__(self):
        # `int.__str__` defers to `__repr__`, which `Enum` overrides; format the
        # underlying integer directly so `str(Status.OK) == '200'`.
        return "%d" % int(self)

    def __format__(self, format_spec):
        return format(int(self), format_spec)
