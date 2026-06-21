# Issue #2702: reversed() over a dict, a dict view, or a mappingproxy must
# report CPython 3.12's kind-specific iterator type names
# (dict_reversekeyiterator / dict_reversevalueiterator / dict_reverseitemiterator)
# rather than the generic list_reverseiterator.

d = {"a": 1, "b": 2, "c": 3}

# type(...).__name__ for the four dict/dict-view reversed iterators.
print(type(reversed(d)).__name__)
print(type(reversed(d.keys())).__name__)
print(type(reversed(d.values())).__name__)
print(type(reversed(d.items())).__name__)

# repr follows the same naming (address normalised away by only printing the
# type-name prefix).
print(repr(reversed(d.keys())).split(" object")[0])
print(repr(reversed(d.values())).split(" object")[0])
print(repr(reversed(d.items())).split(" object")[0])

# Empty dict still names the iterator by kind.
print(type(reversed({})).__name__)
print(type(reversed({}.values())).__name__)

# class-backed mappingproxy (type.__dict__) yields keys -> dict_reversekeyiterator.
class C:
    x = 1
    y = 2

mp = type(C).__dict__
print(type(reversed(mp)).__name__)
print(type(mp.__reversed__()).__name__)

# dict-backed mappingproxy (d.keys().mapping) -> dict_reversekeyiterator.
proxy = d.keys().mapping
print(type(reversed(proxy)).__name__)

# Yield order and the size-mutation guard are unchanged (#2448): the type-name
# fix must not alter behaviour.
print(list(reversed(d)))
print(list(reversed(d.values())))
print(list(reversed(d.items())))

it = reversed(d)
next(it)
d["z"] = 99
try:
    next(it)
except RuntimeError as e:
    print("RuntimeError:", e)
