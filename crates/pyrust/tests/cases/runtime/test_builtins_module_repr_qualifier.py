# Parity fixture for the module qualifier in the default type / instance reprs.
# Issue #2927: CPython (typeobject.c::type_repr and object_repr) qualifies the
# display with __module__ only when that attribute is present AND is a string
# other than "builtins"; otherwise the bare __name__ is used with no prefix.
# So repr(object()) is "<object object at 0x...>", not "<__main__.object ...>".
#
# Addresses differ per run, so mask the hex before printing.


def mask(text):
    out = []
    i = 0
    while i < len(text):
        if text.startswith("0x", i):
            i += 2
            while i < len(text) and text[i] in "0123456789abcdefABCDEF":
                i += 1
            out.append("0xADDR")
        else:
            out.append(text[i])
            i += 1
    return "".join(out)


# --- builtins-module classes: no qualifier, for instances and for the type ---
print(mask(repr(object())))         # <object object at 0xADDR>
print(repr(object))                 # <class 'object'>
print(repr(int), repr(list), repr(type), repr(Exception), repr(type(None)))
print(object.__module__, int.__module__)


# --- user classes keep the __main__ qualifier ---
class C:
    pass


print(mask(repr(C())))              # <__main__.C object at 0xADDR>
print(repr(C))                      # <class '__main__.C'>


# --- nested classes qualify with __qualname__ (dotted) ---
class Outer:
    class Inner:
        pass


print(mask(repr(Outer.Inner())))
print(repr(Outer.Inner))


def make_local():
    class Local:
        pass

    return Local


Local = make_local()
print(mask(repr(Local())))          # <__main__.make_local.<locals>.Local object at 0xADDR>
print(repr(Local))


# --- __module__ mutated to "builtins": qualifier dropped, __name__ used ---
class Mutated:
    pass


Mutated.__module__ = "builtins"
print(mask(repr(Mutated())))        # <Mutated object at 0xADDR>
print(repr(Mutated))                # <class 'Mutated'>


# A nested class whose module is "builtins" falls back to __name__ (CPython's
# tp_name for a heap type), NOT the dotted __qualname__.
class Nested:
    class Deep:
        pass


Nested.Deep.__module__ = "builtins"
print(mask(repr(Nested.Deep())))    # <Deep object at 0xADDR>
print(repr(Nested.Deep))            # <class 'Deep'>


# --- non-string __module__ (None / int) is treated as absent ---
class ModNone:
    pass


ModNone.__module__ = None
print(mask(repr(ModNone())))        # <ModNone object at 0xADDR>
print(repr(ModNone))                # <class 'ModNone'>


class ModInt:
    pass


ModInt.__module__ = 42
print(mask(repr(ModInt())))         # <ModInt object at 0xADDR>
print(repr(ModInt))                 # <class 'ModInt'>


# --- an empty-string __module__ is still a string, so it still qualifies ---
class ModEmpty:
    pass


ModEmpty.__module__ = ""
print(mask(repr(ModEmpty())))       # <.ModEmpty object at 0xADDR>
print(repr(ModEmpty))               # <class '.ModEmpty'>


# --- __name__ / __qualname__ are mutable presentation metadata ---
# Qualified form spells __qualname__; the bare (builtins) form spells __name__.
class Renamed:
    pass


Renamed.__name__ = "RenamedName"
Renamed.__qualname__ = "Renamed.Deep"
print(mask(repr(Renamed())))        # <__main__.Renamed.Deep object at 0xADDR>
print(repr(Renamed))                # <class '__main__.Renamed.Deep'>

Renamed.__module__ = "builtins"
print(mask(repr(Renamed())))        # <RenamedName object at 0xADDR>
print(repr(Renamed))                # <class 'RenamedName'>


# --- classes carrying a real (non-builtins) module keep their qualifier ---
class Packaged:
    __module__ = "mypackage.sub"


print(mask(repr(Packaged())))       # <mypackage.sub.Packaged object at 0xADDR>
print(repr(Packaged))               # <class 'mypackage.sub.Packaged'>

import itertools

print(repr(itertools.count), repr(itertools.chain))
print(mask(repr(itertools.count(0))))


# --- exceptions: class repr follows the rule, instance repr is arg-based ---
class MyError(Exception):
    pass


print(repr(MyError))                # <class '__main__.MyError'>
print(repr(MyError("boom")))        # MyError('boom')
MyError.__module__ = "builtins"
print(repr(MyError))                # <class 'MyError'>
print(repr(MyError("boom")))        # MyError('boom')


# --- builtin subclasses render their contents, not the object form ---
class MyList(list):
    pass


class MyStr(str):
    pass


print(repr(MyList([1, 2])), repr(MyStr("x")))
print(repr(MyList), repr(MyStr))
MyList.__module__ = "builtins"
print(repr(MyList), repr(MyList([1, 2])))


# --- a classmethod bound to a non-function value names its owning class ---
class Owner:
    cm = classmethod(3)


print(repr(Owner.cm))               # <bound method ? of <class '__main__.Owner'>>
Owner.__module__ = "builtins"
print(repr(Owner.cm))               # <bound method ? of <class 'Owner'>>


# --- three-argument type() and metaclasses follow the same rule ---
X = type("X", (), {})
Y = type("Y", (), {"__module__": "builtins"})
Z = type("Z", (), {"__module__": "zmod", "__qualname__": "Outer.Z"})
print(repr(X), mask(repr(X())))
print(repr(Y), mask(repr(Y())))
print(repr(Z), mask(repr(Z())))


class Meta(type):
    pass


class WithMeta(metaclass=Meta):
    pass


WithMeta.__module__ = "builtins"
print(repr(WithMeta), mask(repr(WithMeta())))


# __module__ is read from the class's own dict, not inherited from a base:
# a subclass gets its own "__main__" entry at class-creation time.
class BuiltinsBase:
    __module__ = "builtins"


class Derived(BuiltinsBase):
    pass


print(repr(BuiltinsBase), repr(Derived), Derived.__module__)
print(mask(repr(Derived())))


# --- str() of a plain instance uses the same form as repr() ---
class Plain:
    pass


Plain.__module__ = "builtins"
p = Plain()
print(mask(str(p)) == mask(repr(p)), mask(str(p)))
print(str(object), str(C))


# --- a str SUBCLASS __module__ is still a string (CPython gates on
# PyUnicode_Check) and the raw text is used, ignoring any __str__/__repr__
# override on the subclass.
class SubStr(str):
    def __str__(self):
        return "STR-OVERRIDE"

    def __repr__(self):
        return "REPR-OVERRIDE"


class SubMod:
    pass


SubMod.__module__ = SubStr("submod")
print(repr(SubMod), mask(repr(SubMod())))
SubMod.__module__ = SubStr("builtins")
print(repr(SubMod), mask(repr(SubMod())))


class DeepStr(SubStr):
    pass


class DeepMod:
    pass


DeepMod.__module__ = DeepStr("deepmod")
print(repr(DeepMod), mask(repr(DeepMod())))


# ...while a subclass of a *non-str* builtin stays "absent".
class SubInt(int):
    pass


class IntMod:
    pass


IntMod.__module__ = SubInt(7)
print(repr(IntMod), mask(repr(IntMod())))


class SubBytes(bytes):
    pass


class BytesMod:
    pass


BytesMod.__module__ = SubBytes(b"nope")
print(repr(BytesMod), mask(repr(BytesMod())))


# A plain instance cannot spoof a module name by carrying string-ish internal
# state; only a real str subclass counts.
class NotAStr:
    pass


spoof = NotAStr()
spoof.__builtin_data__ = "spoofed"


class SpoofMod:
    pass


SpoofMod.__module__ = spoof
print(repr(SpoofMod), mask(repr(SpoofMod())))
