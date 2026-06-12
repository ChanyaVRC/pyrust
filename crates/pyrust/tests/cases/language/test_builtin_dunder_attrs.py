"""
Builtin slot dunders are exposed as *attributes* on the type and its
subclasses, not just dispatchable through the operator/iteration machinery
(#2387).  CPython exposes every slot as a method/wrapper descriptor, so
`list.__iter__`, `hasattr(bytes, '__mod__')`, and `LI([1]).__iter__()` (LI a
list subclass) all resolve.

This fixture pins the *behaviour* (resolution, callability, bound/unbound
semantics, subclass MRO, override-wins, format errors).  It deliberately does
NOT assert the descriptor's `type(...).__name__` / `repr(...)` /
`wrapper __X__() takes no keyword arguments` prefix: those wordings diverge
from CPython for *every* already-exposed builtin dunder (a pre-existing,
orthogonal gap), so locking them in here would be wrong.
"""


def show(label, fn):
    try:
        print(label, "::", repr(fn()))
    except Exception as e:
        print(label, "::", type(e).__name__ + ":", e)


# --- hasattr / dir membership ------------------------------------------------
print(hasattr(list, "__iter__"))
print(hasattr(tuple, "__iter__"))
print(hasattr(str, "__iter__"))
print(hasattr(bytes, "__iter__"))
print(hasattr(bytearray, "__iter__"))
print(hasattr(dict, "__iter__"))
print(hasattr(set, "__iter__"))
print(hasattr(frozenset, "__iter__"))
print(hasattr(bytes, "__mod__"))
print(hasattr(str, "__mod__"))
print(hasattr(bytearray, "__mod__"))
print(hasattr(str, "__rmod__"))
print(hasattr(dict, "__reversed__"))
print(hasattr(list, "__reversed__"))
print(hasattr(list, "__next__"))  # list is not its own iterator -> False

print("__iter__" in dir(list))
print("__iter__" in dir([1]))
print("__mod__" in dir(bytes))
print("__reversed__" in dir(dict))
print(dir(list).count("__iter__"))  # de-duplicated -> 1


# --- type-level (unbound) call -----------------------------------------------
print(list(list.__iter__([1, 2, 3])))
print(list(str.__iter__("ab")))
print(list(dict.__iter__({1: "a", 2: "b"})))
print(str.__mod__("%s=%d", ("x", 5)))
print(bytes.__mod__(b"%d", 5))
print(list(list.__reversed__([1, 2, 3])))


# --- bound instance call -----------------------------------------------------
print(list([1, 2].__iter__()))
print("%s and %d".__mod__(("a", 7)))
print(b"%d".__mod__(9))
print(bytearray(b"%d").__mod__(9))
print("x=%s".__rmod__ is not None)
print("val".__rmod__("x=%s"))


# --- detached bound method (m = x.__iter__; m() works) -----------------------
m = [10, 20, 30].__iter__()
print(next(m), next(m))


# --- subclass MRO resolution (inherited slot reachable on the subclass) ------
class LI(list):
    pass


class LS(str):
    pass


class BB(bytes):
    pass


print(list(LI([1, 2]).__iter__()))
print(list(LS("ab").__iter__()))
print(list(BB(b"ab").__iter__()))
print(LS("%s").__mod__("z"))
# type-level call dispatched onto a subclass instance:
print(list(list.__iter__(LI([4, 5]))))
print(list.__add__(LI([1]), [2]))
print(list.__len__(LI([1, 2, 3])))
print(list.__getitem__(LI([7, 8]), 1))
print(list.__contains__(LI([1, 2]), 2))


# --- user override still wins ------------------------------------------------
class LO(list):
    def __iter__(self):
        return iter([99])


print(list(LO([1, 2]).__iter__()))


# --- numeric __mod__ unaffected (must not regress) ---------------------------
print((17).__mod__(5))
print((17.0).__mod__(5.0))
print((5).__rmod__(17))


# --- error / arity parity ----------------------------------------------------
show("iter too many (unbound)", lambda: list.__iter__([1], 2))
show("iter too many (bound)", lambda: [1].__iter__(2))
show("iter no arg (descriptor)", lambda: list.__iter__())
show("mod bad type %d str", lambda: "%d".__mod__("x"))
show("mod bad type %d list", lambda: "%d".__mod__([1]))
show("mod arity 0", lambda: "%s".__mod__())
show("mod arity 2", lambda: b"%s".__mod__(b"x", b"y"))
