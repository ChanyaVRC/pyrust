# Python-level implementation of the `enum` module, injected onto the native
# `enum` module by `enum_mod.rs::inject_python_members` (mirrors the
# `operator` / `string` modules).
#
# A close port of the core of CPython 3.12's `Lib/enum.py`, restricted to the
# minimum viable surface: `Enum`, `IntEnum`, `Flag`, `EnumMeta` (aka
# `EnumType`), and `auto`.  `StrEnum`, `IntFlag`, `_generate_next_value_`
# overrides, functional construction, pickling support, and `_missing_` are
# intentionally omitted.
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

    def __init__(self, is_flag=False):
        super().__init__()
        self._member_names = []
        self._last_value = 0
        # When building a ``Flag`` subclass, ``auto()`` resolves to the next
        # unused single bit rather than the next consecutive integer.
        self._is_flag = is_flag

    def __setitem__(self, key, value):
        if not _is_sunder(key) and not _is_dunder(key) and not _is_descriptor(value):
            # A genuine enum member.
            if isinstance(value, auto):
                value = self._next_value()
            self._last_value = value
            self._member_names.append(key)
        super().__setitem__(key, value)

    def _next_value(self):
        if self._is_flag:
            # Next power of two strictly above the highest bit seen so far.
            if self._last_value <= 0:
                return 1
            high_bit = 1
            while high_bit <= self._last_value:
                high_bit <<= 1
            return high_bit
        return self._last_value + 1


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


def _flag_bin(num, max_bits):
    # Mirror CPython ``enum.bin`` (non-negative branch): a leading sign group of
    # ``0`` followed by the zero-padded binary digits, e.g. ``0b0 1000``.
    ceiling = 2 ** num.bit_length()
    s = bin(num + ceiling).replace("1", "0", 1)
    sign = s[:3]
    digits = s[3:]
    if len(digits) < max_bits:
        digits = (sign[-1] * max_bits + digits)[-max_bits:]
    return "%s %s" % (sign, digits)


def _is_single_bit(value):
    # True for exactly one set bit (a canonical Flag member, e.g. 1/2/4/...).
    return isinstance(value, int) and value > 0 and (value & (value - 1)) == 0


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
        is_flag = any(getattr(base, "_is_flag_", False) for base in bases)
        return _EnumDict(is_flag=is_flag)

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

        is_flag = getattr(enum_class, "_is_flag_", False)
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
                enum_class._value2member_map_[value] = member
                # For a Flag, only single-bit values are *canonical* members
                # that participate in iteration; multi-bit (composite) and
                # zero values are named, looked-up, but never iterated.
                if not is_flag or _is_single_bit(value):
                    enum_class._member_names_.append(name)
            enum_class._member_map_[name] = member
            setattr(enum_class, name, member)

        return enum_class

    def __call__(cls, value):
        # ``Color(value)`` performs a value lookup, returning the existing
        # member.  Calling the bare ``Enum`` base (no members) is not supported.
        try:
            return cls._value2member_map_[value]
        except KeyError:
            pass
        if getattr(cls, "_is_flag_", False):
            # Flags accept any combination of the defined single-bit members
            # (a *composite* value), synthesising a pseudo-member on demand.
            return cls._missing_(value)
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
        kind = "flag" if getattr(cls, "_is_flag_", False) else "enum"
        return "<%s %r>" % (kind, cls.__name__)


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


class Flag(Enum):
    """Enum where members support the bitwise operators (``&``, ``|``, ``^``,
    ``~``).

    A *composite* value (e.g. ``Color.RED | Color.GREEN``) is a synthesised
    pseudo-member combining named single-bit members; iterating it yields those
    members in definition order, and ``len()`` counts them.
    """

    # Marker consulted by ``EnumType.__prepare__`` (``auto()`` -> powers of two)
    # and ``EnumType.__call__`` (value lookup synthesises composites).
    _is_flag_ = True

    @classmethod
    def _all_bits_(cls):
        # OR of every canonical single-bit member's value.
        result = 0
        for name in cls._member_names_:
            result |= cls._member_map_[name]._value_
        return result

    @classmethod
    def _missing_(cls, value):
        # Build (and cache) the composite pseudo-member for ``value``.
        if not isinstance(value, int):
            raise ValueError("%r is not a valid %s" % (value, cls.__name__))
        all_bits = cls._all_bits_()
        if value < 0 or (value & ~all_bits) != 0:
            max_bits = max(value.bit_length(), all_bits.bit_length())
            # ``<flag 'Name'>`` is built explicitly rather than via ``%r`` of the
            # class so the message is byte-exact even where ``repr(cls)`` does
            # not dispatch through the metaclass ``__repr__``.
            raise ValueError(
                "<flag %r> invalid value %r\n    given %s\n  allowed %s"
                % (
                    cls.__name__,
                    value,
                    _flag_bin(value, max_bits),
                    _flag_bin(all_bits, max_bits),
                )
            )
        member = object.__new__(cls)
        member._name_ = None
        member._value_ = value
        cls._value2member_map_[value] = member
        return member

    def _decompose_(self):
        # Named single-bit members contained in this value, in definition order.
        cls = self.__class__
        members = []
        for name in cls._member_names_:
            m = cls._member_map_[name]
            bit = m._value_
            if bit != 0 and (self._value_ & bit) == bit:
                members.append(m)
        return members

    def __iter__(self):
        return iter(self._decompose_())

    def __len__(self):
        return len(self._decompose_())

    def __bool__(self):
        return bool(self._value_)

    def __or__(self, other):
        if not isinstance(other, self.__class__):
            return NotImplemented
        return self.__class__(self._value_ | other._value_)

    def __and__(self, other):
        if not isinstance(other, self.__class__):
            return NotImplemented
        return self.__class__(self._value_ & other._value_)

    def __xor__(self, other):
        if not isinstance(other, self.__class__):
            return NotImplemented
        return self.__class__(self._value_ ^ other._value_)

    def __invert__(self):
        return self.__class__(self._all_bits_() & ~self._value_)

    def __contains__(self, other):
        if not isinstance(other, self.__class__):
            raise TypeError(
                "unsupported operand type(s) for 'in': %r and %r"
                % (type(other).__qualname__, self.__class__.__qualname__)
            )
        return (other._value_ & self._value_) == other._value_

    def __repr__(self):
        cls_name = self.__class__.__name__
        if self._name_ is not None:
            # A member defined in the class body (single bit or named alias).
            return "<%s.%s: %r>" % (cls_name, self._name_, self._value_)
        members = self._decompose_()
        if members:
            names = "|".join(m._name_ for m in members)
            return "<%s.%s: %r>" % (cls_name, names, self._value_)
        return "<%s: %r>" % (cls_name, self._value_)

    def __str__(self):
        cls_name = self.__class__.__name__
        if self._name_ is not None:
            return "%s.%s" % (cls_name, self._name_)
        members = self._decompose_()
        if members:
            return "%s.%s" % (cls_name, "|".join(m._name_ for m in members))
        return "%s(%r)" % (cls_name, self._value_)

    def __hash__(self):
        return hash(self._value_)
