# Python-level implementation of the `enum` module, injected onto the native
# `enum` module by `enum_mod.rs::inject_python_members` (mirrors the
# `operator` / `string` modules).
#
# A faithful port of CPython 3.12's `Lib/enum.py` covering `Enum`, `ReprEnum`,
# `IntEnum`, `StrEnum`, `Flag`, `IntFlag`, `EnumType` (`EnumMeta`), `auto`,
# `FlagBoundary` (`STRICT`/`CONFORM`/`EJECT`/`KEEP`), `EnumCheck`
# (`CONTINUOUS`/`NAMED_FLAGS`/`UNIQUE`), `verify`, `unique`, `global_enum`
# (+ `global_enum_repr`, `global_flag_repr`, `global_str`), `property`
# (`DynamicClassAttribute`), `member`, `nonmember`, and `show_flag_values`.
#
# Reference: <https://docs.python.org/3/library/enum.html>

import sys as _sys
from types import DynamicClassAttribute, FunctionType as _FunctionType

__all__ = [
    "EnumType",
    "EnumMeta",
    "Enum",
    "IntEnum",
    "StrEnum",
    "Flag",
    "IntFlag",
    "ReprEnum",
    "auto",
    "unique",
    "property",
    "verify",
    "member",
    "nonmember",
    "FlagBoundary",
    "STRICT",
    "CONFORM",
    "EJECT",
    "KEEP",
    "global_flag_repr",
    "global_enum_repr",
    "global_str",
    "global_enum",
    "EnumCheck",
    "CONTINUOUS",
    "NAMED_FLAGS",
    "UNIQUE",
    "show_flag_values",
]


class nonmember:
    """Protects item from becoming an Enum member during class creation."""

    def __init__(self, value):
        self.value = value


class member:
    """Forces item to become an Enum member during class creation."""

    def __init__(self, value):
        self.value = value


class auto:
    """Placeholder for a value auto-assigned by ``EnumType``."""

    def __init__(self):
        self.value = None


class property(DynamicClassAttribute):
    """Enum-aware property descriptor (Python 3.11+).

    Access on an enum member calls ``fget(member)``; access on the enum class
    returns the member with the same name if one exists, otherwise raises
    ``AttributeError`` (matching CPython's ``enum.property``).
    """

    def __init__(self, fget=None, fset=None, fdel=None, doc=None):
        self.fget = fget
        self.fset = fset
        self.fdel = fdel
        self.__doc__ = doc if doc is not None else (fget.__doc__ if fget is not None else None)
        self.overwrite_doc = doc is None
        self.__isabstractmethod__ = bool(getattr(fget, "__isabstractmethod__", False))
        self.name = None

    def __set_name__(self, ownerclass, name):
        self.name = name

    def __get__(self, instance, ownerclass=None):
        if instance is None:
            if ownerclass is not None and self.name is not None:
                mmap = getattr(ownerclass, "_member_map_", None)
                if mmap is not None and self.name in mmap:
                    return mmap[self.name]
            if self.__isabstractmethod__:
                return self
            cls_repr = repr(ownerclass) if ownerclass is not None else "Enum"
            raise AttributeError("%s has no attribute %r" % (cls_repr, self.name))
        if self.fget is None:
            raise AttributeError("unreadable attribute")
        return self.fget(instance)


def _iter_bits_lsb(num):
    """Yield powers of two set in ``num`` from least-significant bit up."""
    original = num
    if isinstance(num, Enum):
        num = num.value
    if num < 0:
        raise ValueError("%r is not a positive integer" % (original,))
    while num:
        b = num & (~num + 1)
        yield b
        num ^= b


def show_flag_values(value):
    """Return a list of all power-of-two integers contained in ``value``."""
    return list(_iter_bits_lsb(value))


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
    # User-defined functions are descriptors in CPython (`function.__get__`),
    # but PyRust does not expose `__get__` on them, so check explicitly.
    return (
        isinstance(obj, _FunctionType)
        or hasattr(obj, "__get__")
        or hasattr(obj, "__set__")
        or hasattr(obj, "__delete__")
    )


def _find_data_type(bases):
    """Return the first non-Enum concrete mixin base type, or ``object``."""
    enum_base = globals().get("Enum")
    for base in bases:
        if enum_base is not None and issubclass(base, enum_base):
            dt = getattr(base, "_member_type_", object)
            if dt is not object:
                return dt
        elif base is not object:
            return base
    return object


def _find_generate_next_value(bases, classdict=None):
    if classdict is not None and "_generate_next_value_" in classdict:
        fn = classdict["_generate_next_value_"]
        if isinstance(fn, staticmethod):
            return fn.__func__
        return fn
    for base in bases:
        for cls in base.__mro__:
            if "_generate_next_value_" in cls.__dict__:
                fn = cls.__dict__["_generate_next_value_"]
                if isinstance(fn, staticmethod):
                    return fn.__func__
                return fn
    return None


class _EnumDict(dict):
    """Class-body namespace returned by ``EnumType.__prepare__``."""

    def __init__(self, is_flag=False, is_str=False, gen_next=None):
        super().__init__()
        self._member_names = []
        self._last_values = []
        self._last_value = 0
        self._is_flag = is_flag
        self._is_str = is_str
        self._generate_next_value = gen_next
        self._explicit_members = set()

    def __setitem__(self, key, value):
        if key == "_generate_next_value_":
            self._generate_next_value = (
                value.__func__ if isinstance(value, staticmethod) else value
            )
            super().__setitem__(key, value)
            return
        if isinstance(value, nonmember):
            super().__setitem__(key, value.value)
            return
        if isinstance(value, member):
            value = value.value
            if isinstance(value, auto):
                value = self._resolve_auto(key)
            self._record_value(value)
            self._member_names.append(key)
            self._explicit_members.add(key)
            super().__setitem__(key, value)
            return
        if not _is_sunder(key) and not _is_dunder(key) and not _is_descriptor(value):
            if isinstance(value, auto):
                value = self._resolve_auto(key)
            self._record_value(value)
            self._member_names.append(key)
        super().__setitem__(key, value)

    def _record_value(self, value):
        self._last_values.append(value)
        if self._is_str:
            pass
        elif self._is_flag and isinstance(value, int):
            self._last_value |= value
        elif isinstance(value, int):
            self._last_value = value

    def _resolve_auto(self, name):
        if self._generate_next_value is not None:
            return self._generate_next_value(
                name, 1, len(self._member_names), list(self._last_values)
            )
        if self._is_str:
            return name.lower()
        if not self._is_flag:
            return self._last_value + 1
        if self._last_value <= 0:
            return 1
        high_bit = self._last_value.bit_length() - 1
        return 2 ** (high_bit + 1)


def _flag_bit_count(v):
    if not isinstance(v, int) or v <= 0:
        return 0
    return bin(v).count("1")


class EnumType(type):
    """Metaclass that turns plain class-body assignments into enum members."""

    @classmethod
    def __prepare__(mcls, cls, bases, boundary=None, **kwds):
        flag_base = globals().get("Flag")
        is_flag = flag_base is not None and any(
            isinstance(base, type) and issubclass(base, flag_base) for base in bases
        )
        str_base = globals().get("StrEnum")
        is_str = str_base is not None and any(
            isinstance(base, type) and issubclass(base, str_base) for base in bases
        )
        gen_next = _find_generate_next_value(bases)
        return _EnumDict(is_flag=is_flag, is_str=is_str, gen_next=gen_next)

    def __new__(mcls, cls, bases, classdict, boundary=None, **kwds):
        if not isinstance(classdict, _EnumDict):
            # Fallback if constructed directly with a plain dict.
            flag_base = globals().get("Flag")
            is_flag = flag_base is not None and any(
                isinstance(base, type) and issubclass(base, flag_base) for base in bases
            )
            str_base = globals().get("StrEnum")
            is_str = str_base is not None and any(
                isinstance(base, type) and issubclass(base, str_base) for base in bases
            )
            gen_next = _find_generate_next_value(bases, classdict)
            ed = _EnumDict(is_flag=is_flag, is_str=is_str, gen_next=gen_next)
            for k, v in classdict.items():
                ed[k] = v
            classdict = ed

        member_names = classdict._member_names
        ns = {}
        for key, value in classdict.items():
            if key not in member_names:
                ns[key] = value

        # Re-check _generate_next_value_ defined inside class body in case any
        # auto() values were processed before the method was registered.
        gen_fn = _find_generate_next_value(bases, classdict)

        enum_class = super().__new__(mcls, cls, bases, ns)
        enum_class._member_names_ = []
        enum_class._member_map_ = {}
        enum_class._value2member_map_ = {}
        enum_class._member_type_ = _find_data_type(bases)

        flag_base = globals().get("Flag")
        is_flag = flag_base is not None and issubclass(enum_class, flag_base)

        if is_flag:
            if boundary is not None:
                enum_class._boundary_ = boundary
            elif not hasattr(enum_class, "_boundary_"):
                intflag_base = globals().get("IntFlag")
                if intflag_base is not None and issubclass(enum_class, intflag_base):
                    enum_class._boundary_ = globals().get("KEEP")
                else:
                    enum_class._boundary_ = globals().get("STRICT")

        # First pass: collect values and resolve any remaining auto() sentinels.
        resolved_items = []
        last_vals = []
        for name in member_names:
            value = classdict[name]
            if isinstance(value, auto):
                if gen_fn is not None:
                    value = gen_fn(name, 1, len(resolved_items), list(last_vals))
                else:
                    value = classdict._resolve_auto(name)
            last_vals.append(value)
            resolved_items.append((name, value))

        flag_mask = 0
        for name, value in resolved_items:
            is_non_canonical_flag = (
                is_flag
                and isinstance(value, int)
                and (value <= 0 or (value & (value - 1)) != 0)
            )
            if not is_non_canonical_flag and is_flag and isinstance(value, int):
                flag_mask |= value
            if value in enum_class._value2member_map_:
                member_obj = enum_class._value2member_map_[value]
            elif is_non_canonical_flag:
                member_obj = enum_class._new_member_(name, value)
                enum_class._value2member_map_[value] = member_obj
            else:
                member_obj = enum_class._new_member_(name, value)
                enum_class._member_names_.append(name)
                enum_class._value2member_map_[value] = member_obj
            enum_class._member_map_[name] = member_obj
            # Do not shadow an `enum.property` / `DynamicClassAttribute` on a
            # base class (such as `Enum.name` or `Enum.value`): accessing
            # `MyEnum.value` routes through the descriptor's `__get__(None, cls)`
            # which resolves `cls._member_map_['value']`, while instance access
            # `MyEnum.X.value` still calls the property's `fget`.
            shadows_enum_prop = False
            for base in enum_class.__mro__[1:]:
                if isinstance(base.__dict__.get(name), DynamicClassAttribute):
                    shadows_enum_prop = True
                    break
            if not shadows_enum_prop:
                setattr(enum_class, name, member_obj)

        if is_flag:
            enum_class._flag_mask_ = flag_mask
            enum_class._all_bits_ = (1 << flag_mask.bit_length()) - 1 if flag_mask else 0

        return enum_class

    def __call__(
        cls,
        value,
        names=None,
        *,
        module=None,
        qualname=None,
        type=None,
        start=1,
        boundary=None,
    ):
        if names is not None:
            return cls._create_(
                value,
                names,
                module=module,
                qualname=qualname,
                type=type,
                start=start,
                boundary=boundary,
            )
        try:
            return cls._value2member_map_[value]
        except (KeyError, TypeError):
            pass
        result = cls._missing_(value)
        if isinstance(result, cls):
            return result
        flag_base = globals().get("Flag")
        if (
            flag_base is not None
            and issubclass(cls, flag_base)
            and getattr(cls, "_boundary_", None) == globals().get("EJECT")
            and isinstance(result, int)
        ):
            return result
        if result is not None:
            raise TypeError(
                "error in %s._missing_: returned %r instead of None or a valid member"
                % (cls.__name__, result)
            )
        raise ValueError("%r is not a valid %s" % (value, cls.__name__))

    def _create_(
        cls,
        class_name,
        names,
        *,
        module=None,
        qualname=None,
        type=None,
        start=1,
        boundary=None,
    ):
        bases = (cls,) if type is None else (type, cls)
        classdict = EnumType.__prepare__(class_name, bases, boundary=boundary)
        if isinstance(names, str):
            names = names.replace(",", " ").split()
        if isinstance(names, dict):
            for k, v in names.items():
                classdict[k] = v
        else:
            items = list(names)
            if items and isinstance(items[0], str):
                idx = start
                is_flag = issubclass(cls, Flag)
                for n in items:
                    classdict[n] = idx
                    idx = (idx << 1) if is_flag else (idx + 1)
            else:
                for k, v in items:
                    classdict[k] = v
        if module is not None:
            classdict["__module__"] = module
        if qualname is not None:
            classdict["__qualname__"] = qualname
        return EnumType(class_name, bases, classdict, boundary=boundary)

    def __getitem__(cls, name):
        return cls._member_map_[name]

    def __iter__(cls):
        return (cls._member_map_[name] for name in cls._member_names_)

    def __reversed__(cls):
        return (cls._member_map_[name] for name in reversed(cls._member_names_))

    def __len__(cls):
        return len(cls._member_names_)

    def __contains__(cls, member):
        if isinstance(member, cls):
            return True
        try:
            return member in cls._value2member_map_
        except TypeError:
            return False

    def __repr__(cls):
        flag_base = globals().get("Flag")
        if flag_base is not None and issubclass(cls, flag_base):
            return "<flag %r>" % cls.__name__
        return "<enum %r>" % cls.__name__


# CPython 3.12 renamed the metaclass to ``EnumType`` and keeps ``EnumMeta`` as
# a backwards-compatible alias, so ``type(Color).__name__ == 'EnumType'``.
EnumMeta = EnumType


class Enum(metaclass=EnumType):
    """Base class for creating enumerated constants."""

    @staticmethod
    def _generate_next_value_(name, start, count, last_values):
        for last_value in reversed(last_values):
            try:
                return last_value + 1
            except TypeError:
                pass
        return start

    @classmethod
    def _new_member_(cls, name, value):
        member_type = getattr(cls, "_member_type_", object)
        if member_type is object:
            member_obj = object.__new__(cls)
        else:
            member_obj = member_type.__new__(cls, value)
        member_obj._name_ = name
        member_obj._value_ = value
        return member_obj

    @classmethod
    def _missing_(cls, value):
        return None

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


class ReprEnum(Enum):
    """Only changes the repr(), leaving str() and format() to the mixed-in type."""

    def __str__(self):
        member_type = getattr(self.__class__, "_member_type_", object)
        if member_type is object:
            return Enum.__str__(self)
        return str(self._value_)

    def __format__(self, format_spec):
        member_type = getattr(self.__class__, "_member_type_", object)
        if member_type is object:
            return Enum.__format__(self, format_spec)
        return format(self._value_, format_spec)


class IntEnum(int, ReprEnum):
    """Enum where members are also (and comparable to) ints."""

    @classmethod
    def _new_member_(cls, name, value):
        member_obj = int.__new__(cls, value)
        member_obj._name_ = name
        member_obj._value_ = value
        return member_obj

    def __str__(self):
        return "%d" % int(self)

    def __format__(self, format_spec):
        return format(int(self), format_spec)


class StrEnum(str, ReprEnum):
    """Enum where members are also (and comparable to) ``str``."""

    @staticmethod
    def _generate_next_value_(name, start, count, last_values):
        return name.lower()

    @classmethod
    def _new_member_(cls, name, value):
        if not isinstance(value, str):
            raise TypeError("%r is not a string" % (value,))
        member_obj = str.__new__(cls, value)
        member_obj._name_ = name
        member_obj._value_ = value
        return member_obj

    def __str__(self):
        return self._value_

    def __format__(self, format_spec):
        return format(self._value_, format_spec)


class FlagBoundary(StrEnum):
    """Control how out of range values are handled in Flag."""

    STRICT = "strict"
    CONFORM = "conform"
    EJECT = "eject"
    KEEP = "keep"


STRICT, CONFORM, EJECT, KEEP = FlagBoundary


class EnumCheck(StrEnum):
    """Various conditions to check an enumeration for."""

    CONTINUOUS = "no skipped integer values"
    NAMED_FLAGS = "multi-flag aliases may not contain unnamed flags"
    UNIQUE = "one name per value"


CONTINUOUS, NAMED_FLAGS, UNIQUE = EnumCheck


def _format_binary(val, bit_len):
    if val < 0:
        sign = "1"
        bits = bin(val & ((1 << max(bit_len, 1)) - 1))[2:].zfill(max(bit_len, 1))
    else:
        sign = "0"
        bits = bin(val)[2:].zfill(max(bit_len, 1))
    return "0b%s %s" % (sign, bits)


class Flag(Enum):
    """Base class for creating enumerated bit-flag constants."""

    _boundary_ = STRICT
    _flag_mask_ = 0
    _all_bits_ = 0

    @staticmethod
    def _generate_next_value_(name, start, count, last_values):
        mask = 0
        for v in last_values:
            if isinstance(v, int):
                mask |= v
        if mask <= 0:
            return 1
        return 1 << mask.bit_length()

    @classmethod
    def _new_member_(cls, name, value):
        member_obj = object.__new__(cls)
        member_obj._name_ = name
        member_obj._value_ = value
        return member_obj

    @classmethod
    def _make_pseudo_member(cls, value):
        cached = cls._value2member_map_.get(value)
        if cached is not None:
            return cached
        member_type = getattr(cls, "_member_type_", object)
        if member_type is object:
            pseudo = object.__new__(cls)
        else:
            pseudo = member_type.__new__(cls, value)
        pseudo._name_ = None
        pseudo._value_ = value
        cls._value2member_map_[value] = pseudo
        return pseudo

    @classmethod
    def _missing_(cls, value):
        if not isinstance(value, int) or isinstance(value, bool):
            raise ValueError("%r is not a valid %s" % (value, cls.__name__))
        member_obj = cls._value2member_map_.get(value)
        if member_obj is not None:
            return member_obj
        flag_mask = cls._flag_mask_
        boundary = cls._boundary_
        if value < 0:
            # Negative values are two's-complement bitmasks.
            if boundary is STRICT:
                if (~value & ~flag_mask) != 0:
                    bit_len = max(flag_mask.bit_length(), (~value).bit_length())
                    raise ValueError(
                        "%r invalid value %r\n    given %s\n  allowed %s"
                        % (
                            cls,
                            value,
                            _format_binary(value, bit_len),
                            _format_binary(flag_mask, bit_len),
                        )
                    )
                value = value & flag_mask
            elif boundary is CONFORM:
                value = value & flag_mask
            elif boundary is EJECT:
                if (~value & ~flag_mask) != 0:
                    return int(value)
                value = value & flag_mask
            elif boundary is KEEP:
                value = value & flag_mask
            return cls._make_pseudo_member(value)

        unknown = value & ~flag_mask
        if unknown != 0:
            if boundary is STRICT:
                bit_len = max(flag_mask.bit_length(), value.bit_length())
                raise ValueError(
                    "%r invalid value %r\n    given %s\n  allowed %s"
                    % (
                        cls,
                        value,
                        _format_binary(value, bit_len),
                        _format_binary(flag_mask, bit_len),
                    )
                )
            elif boundary is CONFORM:
                value = value & flag_mask
            elif boundary is EJECT:
                return int(value)
            elif boundary is KEEP:
                pass
        return cls._make_pseudo_member(value)

    @property
    def name(self):
        if self._name_ is not None:
            return self._name_
        names = self._decompose_names_()
        if names:
            return "|".join(names)
        return None

    def _decompose_members_(self):
        members = []
        for name in self.__class__._member_names_:
            member_obj = self.__class__._member_map_[name]
            v = member_obj._value_
            if v and (v & (v - 1)) == 0 and (self._value_ & v) == v:
                members.append(member_obj)
        return members

    def _decompose_names_(self):
        names = [m._name_ for m in self._decompose_members_()]
        unknown = self._value_ & ~self.__class__._flag_mask_
        if unknown and names:
            names.append(str(unknown))
        return names

    def __iter__(self):
        for m in self._decompose_members_():
            yield m

    def __len__(self):
        return len(self._decompose_members_())

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
        return self.__class__(self.__class__._flag_mask_ & ~self._value_)

    def __contains__(self, other):
        if not isinstance(other, self.__class__):
            raise TypeError(
                "unsupported operand type(s) for 'in': %r and %r"
                % (type(other).__name__, self.__class__.__name__)
            )
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
        names = self._decompose_names_()
        if names:
            return "<%s.%s: %r>" % (cls_name, "|".join(names), self._value_)
        return "<%s: %r>" % (cls_name, self._value_)

    def __str__(self):
        cls_name = self.__class__.__name__
        if self._name_ is not None:
            return "%s.%s" % (cls_name, self._name_)
        names = self._decompose_names_()
        if names:
            return "%s.%s" % (cls_name, "|".join(names))
        return "%s(%r)" % (cls_name, self._value_)


class IntFlag(int, ReprEnum, Flag):
    """``Flag`` whose members are also (and comparable to) ints."""

    _boundary_ = KEEP

    @classmethod
    def _new_member_(cls, name, value):
        member_obj = int.__new__(cls, value)
        member_obj._name_ = name
        member_obj._value_ = value
        return member_obj

    def __or__(self, other):
        if isinstance(other, (self.__class__, int)):
            return self.__class__(int(self) | int(other))
        return NotImplemented

    def __and__(self, other):
        if isinstance(other, (self.__class__, int)):
            return self.__class__(int(self) & int(other))
        return NotImplemented

    def __xor__(self, other):
        if isinstance(other, (self.__class__, int)):
            return self.__class__(int(self) ^ int(other))
        return NotImplemented

    def __ror__(self, other):
        if isinstance(other, (self.__class__, int)):
            return self.__class__(int(self) | int(other))
        return NotImplemented

    def __rand__(self, other):
        if isinstance(other, (self.__class__, int)):
            return self.__class__(int(self) & int(other))
        return NotImplemented

    def __rxor__(self, other):
        if isinstance(other, (self.__class__, int)):
            return self.__class__(int(self) ^ int(other))
        return NotImplemented

    def __invert__(self):
        return self.__class__(self.__class__._flag_mask_ & ~int(self))

    def __hash__(self):
        return hash(self._value_)

    def __str__(self):
        return "%d" % int(self)

    def __format__(self, format_spec):
        return format(int(self), format_spec)


def unique(enumeration):
    """Class decorator for enumerations ensuring unique member values."""
    duplicates = []
    for name, member_obj in enumeration._member_map_.items():
        if name != member_obj.name:
            duplicates.append((name, member_obj.name))
    if duplicates:
        alias_details = ", ".join(
            ["%s -> %s" % (alias, name) for (alias, name) in duplicates]
        )
        raise ValueError(
            "duplicate values found in %r: %s" % (enumeration, alias_details)
        )
    return enumeration


def verify(*checks):
    """Class decorator to verify enum constraints (UNIQUE, CONTINUOUS, NAMED_FLAGS)."""

    def _verify(enumeration):
        for check in checks:
            if check is UNIQUE:
                duplicates = []
                for name, member_obj in enumeration._member_map_.items():
                    if name != member_obj.name:
                        duplicates.append((name, member_obj.name))
                if duplicates:
                    alias_details = ", ".join(
                        ["%s -> %s" % (alias, name) for (alias, name) in duplicates]
                    )
                    raise ValueError(
                        "aliases found in %r: %s" % (enumeration, alias_details)
                    )
            elif check is CONTINUOUS:
                values = [
                    m.value for m in enumeration if isinstance(m.value, int)
                ]
                if values:
                    low = min(values)
                    high = max(values)
                    val_set = set(values)
                    missing = [v for v in range(low, high + 1) if v not in val_set]
                    if missing:
                        raise ValueError(
                            "invalid enum %r: missing values %s"
                            % (enumeration.__name__, ", ".join(str(m) for m in missing))
                        )
            elif check is NAMED_FLAGS:
                flag_mask = getattr(enumeration, "_flag_mask_", 0)
                missed = []
                for name, member_obj in enumeration._member_map_.items():
                    val = member_obj.value
                    if isinstance(val, int) and val > 0:
                        missing_bits = val & ~flag_mask
                        if missing_bits:
                            missed.append((name, hex(missing_bits)))
                if missed:
                    details = ", ".join(
                        "alias %s is missing value %s" % (n, v) for n, v in missed
                    )
                    raise ValueError(
                        "invalid Flag %r: %s [use enum.show_flag_values(value) for details]"
                        % (enumeration.__name__, details)
                    )
        return enumeration

    return _verify


def global_enum_repr(self):
    """Use module.name as repr for global enums."""
    return "%s.%s" % (self.__class__.__module__, self._name_)


def global_flag_repr(self):
    """Use module.name (or module.A|module.B) as repr for global flags."""
    module = self.__class__.__module__
    cls_name = self.__class__.__name__
    if self._name_ is not None:
        return "%s.%s" % (module, self._name_)
    names = self._decompose_names_()
    if names:
        parts = []
        for p in names:
            if p and (p[0].isdigit() or p[0] == "-"):
                parts.append(p)
            else:
                parts.append("%s.%s" % (module, p))
        return "|".join(parts)
    return "%s.%s(%r)" % (module, cls_name, self._value_)


def global_str(self):
    """Use the enum member name as str for global enums."""
    if self._name_ is not None:
        return self._name_
    names = self._decompose_names_()
    if names:
        return "|".join(names)
    return "%s(%r)" % (self.__class__.__name__, self._value_)


def global_enum(cls, update_str=False):
    """Decorator that updates an Enum's repr/str and exports its members to module globals."""
    if issubclass(cls, Flag):
        cls.__repr__ = global_flag_repr
    else:
        cls.__repr__ = global_enum_repr
    if not issubclass(cls, ReprEnum) or update_str:
        cls.__str__ = global_str
    mod = _sys.modules.get(cls.__module__)
    if mod is not None:
        mod_dict = getattr(mod, "__dict__", None)
        if mod_dict is not None:
            mod_dict.update(cls._member_map_)
    return cls
