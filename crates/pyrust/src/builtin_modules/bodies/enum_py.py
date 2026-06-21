# Python-level implementation of the `enum` module, injected onto the native
# `enum` module by `enum_mod.rs::inject_python_members` (mirrors the
# `operator` / `string` modules).
#
# A close port of the core of CPython 3.12's `Lib/enum.py`, restricted to the
# minimum viable surface: `Enum`, `IntEnum`, `Flag`, `IntFlag`, `StrEnum`,
# `EnumMeta` (aka `EnumType`), and `auto`.  `_generate_next_value_` overrides,
# functional construction, pickling support, and `_missing_` are intentionally
# omitted.
#
# Known limitation: a *named* flag composite that references other members
# inside the class body via ``auto()`` (``WHITE = RED | GREEN`` where the
# operands are ``auto()`` members) is not supported, because the interpreter
# flushes ``_EnumDict.__setitem__`` after the class body runs rather than
# interleaving it (so the operands are still unresolved sentinels when the
# expression evaluates).  Spell such composites with explicit integer values
# (``WHITE = 7``) instead; bitwise composition of *finished* members
# (``Color.RED | Color.GREEN``) works as expected.
#
# Reference: <https://docs.python.org/3/library/enum.html>


class auto:
    """Placeholder for a value auto-assigned by ``EnumMeta`` (1, 2, 3, ...).

    For a plain ``Enum`` the metaclass fills in 1, 2, 3, ...; for a ``Flag`` /
    ``IntFlag`` subclass it fills in successive powers of two (1, 2, 4, ...).
    """

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
        # `Flag` / `IntFlag` subclasses generate powers of two (1, 2, 4, ...)
        # for ``auto()`` instead of the sequential 1, 2, 3, ... of a plain
        # ``Enum``; the OR of every value seen so far drives the next bit.
        self._is_flag = is_flag

    def __setitem__(self, key, value):
        if not _is_sunder(key) and not _is_dunder(key) and not _is_descriptor(value):
            # A genuine enum member.
            if isinstance(value, auto):
                value = self._generate_next_value()
            if self._is_flag:
                # Track the OR of every flag value so the next ``auto()``
                # picks the next free bit (mirrors CPython's accumulator).
                self._last_value |= value
            else:
                self._last_value = value
            self._member_names.append(key)
        super().__setitem__(key, value)

    def _generate_next_value(self):
        if not self._is_flag:
            return self._last_value + 1
        # Next power of two strictly above the highest bit seen so far.
        if self._last_value <= 0:
            return 1
        high_bit = self._last_value.bit_length() - 1
        return 2 ** (high_bit + 1)


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
        # ``Flag`` is defined later in this module; during its own creation it
        # is not yet bound, so fall back to ``False`` (a bare ``Flag`` has no
        # members anyway).
        flag_base = globals().get("Flag")
        is_flag = flag_base is not None and any(
            isinstance(base, type) and issubclass(base, flag_base) for base in bases
        )
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

        flag_base = globals().get("Flag")
        is_flag = flag_base is not None and issubclass(enum_class, flag_base)

        for name in member_names:
            value = classdict[name]
            # First-seen value wins as the canonical member; a later member with
            # an existing value is an *alias*: it reuses the canonical member,
            # is accessible by name (`Shape.DIAMOND`) and via `_member_map_`,
            # but is not iterated and does not appear in `_member_names_` (it is
            # `Shape.SQUARE`).  Matches CPython's alias semantics.
            #
            # For `Flag`, a value that is not a single set bit -- a named
            # composite (`WHITE = RED | GREEN | BLUE`) or the all-zero flag
            # (`NONE = 0`) -- is accessible by name but is not a canonical
            # member: it is not iterated and does not occupy `_member_names_`,
            # matching CPython's flag aliasing.
            is_non_canonical_flag = (
                is_flag and isinstance(value, int) and (value == 0 or (value & (value - 1)) != 0)
            )
            if value in enum_class._value2member_map_:
                member = enum_class._value2member_map_[value]
            elif is_non_canonical_flag:
                member = enum_class._new_member_(name, value)
                enum_class._value2member_map_[value] = member
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
            pass
        # ``Flag`` accepts composite integer values (the OR of one or more
        # members), synthesising a pseudo-member on demand.
        flag_base = globals().get("Flag")
        if flag_base is not None and issubclass(cls, flag_base):
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


class StrEnum(str, Enum):
    """Enum where members are also (and comparable to) ``str``.

    Like ``IntEnum`` but for strings: ``str(Direction.NORTH) == 'north'`` and
    ``Direction.NORTH == 'north'``.
    """

    @classmethod
    def _new_member_(cls, name, value):
        if not isinstance(value, str):
            raise TypeError("%r is not a string" % (value,))
        member = str.__new__(cls, value)
        member._name_ = name
        member._value_ = value
        return member

    def __str__(self):
        # The member *is* its string value; format that directly (`Enum`
        # overrides `__repr__`, so deferring to it would print the repr form).
        return self._value_

    def __format__(self, format_spec):
        return format(self._value_, format_spec)


class Flag(Enum):
    """Base class for creating enumerated bit-flag constants.

    Members are powers of two and compose through the bitwise operators
    (``|``, ``&``, ``^``, ``~``); an empty flag (value ``0``) is falsy.
    """

    @classmethod
    def _new_member_(cls, name, value):
        member = object.__new__(cls)
        member._name_ = name
        member._value_ = value
        return member

    @classmethod
    def _missing_(cls, value):
        # Resolve / synthesise the member for an arbitrary composite value.
        if not isinstance(value, int):
            raise ValueError("%r is not a valid %s" % (value, cls.__name__))
        member = cls._value2member_map_.get(value)
        if member is not None:
            return member
        all_bits = 0
        for m in cls._member_map_.values():
            all_bits |= m._value_
        # CPython 3.12 accepts negatives in [-(all_bits+1), -1] and masks them
        # (two's complement): Color(-1) with all_bits=7 yields Color(7).
        if value < 0 and value >= -(all_bits + 1):
            value = value & all_bits
            cached = cls._value2member_map_.get(value)
            if cached is not None:
                return cached
        elif (value & ~all_bits) != 0 or value < 0:
            raise ValueError("%r is not a valid %s" % (value, cls.__name__))
        # Build an unnamed composite pseudo-member and cache it.
        pseudo = object.__new__(cls)
        pseudo._name_ = None
        pseudo._value_ = value
        cls._value2member_map_[value] = pseudo
        return pseudo

    @property
    def name(self):
        # A canonical member carries its own name; an unnamed composite reports
        # the joined names of its set bits (`Color.RED|GREEN`.name == 'RED|GREEN'),
        # matching CPython 3.12.  The empty flag (no set bits) stays nameless.
        if self._name_ is not None:
            return self._name_
        names = self._decompose_()
        if names:
            return "|".join(names)
        return None

    def _decompose_(self):
        # Names of the canonical single-bit members contained in ``self``, in
        # definition order.  Used by ``__str__`` / ``__repr__``.
        members = []
        for name in self.__class__._member_names_:
            member = self.__class__._member_map_[name]
            v = member._value_
            # Single-bit members whose bit is set.
            if v and (v & (v - 1)) == 0 and (self._value_ & v) == v:
                members.append(name)
        return members

    def __iter__(self):
        for name in self._decompose_():
            yield self.__class__._member_map_[name]

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
        all_bits = 0
        for member in self.__class__._member_map_.values():
            all_bits |= member._value_
        return self.__class__(all_bits & ~self._value_)

    def __contains__(self, other):
        if not isinstance(other, self.__class__):
            raise TypeError(
                "unsupported operand type(s) for 'in': %r and %r"
                % (type(other).__name__, self.__class__.__name__)
            )
        if other._value_ == 0:
            return self._value_ == 0
        return (self._value_ & other._value_) == other._value_

    def __hash__(self):
        return hash(self._value_)

    def __eq__(self, other):
        if isinstance(other, self.__class__):
            return self._value_ == other._value_
        return NotImplemented

    def __repr__(self):
        cls_name = self.__class__.__name__
        if self._name_ is not None:
            return "<%s.%s: %r>" % (cls_name, self._name_, self._value_)
        names = self._decompose_()
        if names:
            return "<%s.%s: %r>" % (cls_name, "|".join(names), self._value_)
        return "<%s: %r>" % (cls_name, self._value_)

    def __str__(self):
        cls_name = self.__class__.__name__
        if self._name_ is not None:
            return "%s.%s" % (cls_name, self._name_)
        names = self._decompose_()
        if names:
            return "%s.%s" % (cls_name, "|".join(names))
        return "%s(%r)" % (cls_name, self._value_)


class IntFlag(int, Flag):
    """``Flag`` whose members are also (and comparable to) ints.

    Bitwise operations with plain ``int`` operands are supported and the
    ``str`` / ``format`` of a member is its underlying integer value.
    """

    @classmethod
    def _new_member_(cls, name, value):
        member = int.__new__(cls, value)
        member._name_ = name
        member._value_ = value
        return member

    @classmethod
    def _missing_(cls, value):
        if not isinstance(value, int):
            raise ValueError("%r is not a valid %s" % (value, cls.__name__))
        member = cls._value2member_map_.get(value)
        if member is not None:
            return member
        pseudo = int.__new__(cls, value)
        pseudo._name_ = None
        pseudo._value_ = value
        cls._value2member_map_[value] = pseudo
        return pseudo

    def __or__(self, other):
        return self.__class__(int(self) | int(other))

    def __and__(self, other):
        return self.__class__(int(self) & int(other))

    def __xor__(self, other):
        return self.__class__(int(self) ^ int(other))

    def __ror__(self, other):
        return self.__class__(int(self) | int(other))

    def __rand__(self, other):
        return self.__class__(int(self) & int(other))

    def __rxor__(self, other):
        return self.__class__(int(self) ^ int(other))

    def __invert__(self):
        all_bits = 0
        for member in self.__class__._member_map_.values():
            all_bits |= member._value_
        return self.__class__(all_bits & ~int(self))

    def __hash__(self):
        return hash(self._value_)

    def __str__(self):
        return "%d" % int(self)

    def __format__(self, format_spec):
        return format(int(self), format_spec)
