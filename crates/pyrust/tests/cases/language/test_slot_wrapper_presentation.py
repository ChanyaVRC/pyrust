"""
Builtin slot dunders accessed as attributes present with CPython's descriptor
type and repr (#2397), and their keyword-argument rejection carries the right
`wrapper `/`{type}.` prefix (#2398).

CPython 3.12 models:
  - the unbound type-level form of a slot dunder (`list.__len__`) as a
    `wrapper_descriptor` -> `<slot wrapper '__len__' of 'list' objects>`;
  - the bound instance form (`[1].__len__`) as a `method-wrapper` ->
    `<method-wrapper '__len__' of list object at 0x...>`.
A few container slots (`__getitem__` on list/dict, `__contains__` on
dict/set/frozenset, `__reversed__` on list/dict) are `method_descriptor`s, not
slot wrappers; pyrust still presents those (and plain methods like `append`)
with the generic `builtin_function_or_method` form -- a separate descriptor
class tracked as a follow-up, deliberately NOT asserted here.

Object identity addresses are normalized so the diff is deterministic.
"""


def norm(r):
    # Replace the hex identity address in a method-wrapper repr with a marker.
    head, sep, tail = r.partition(" at 0x")
    return head if sep else r


def info(label, x):
    print(label, "::", type(x).__name__, "::", norm(repr(x)))


# --- unbound slot wrappers (wrapper_descriptor) ------------------------------
info("list.__len__", list.__len__)
info("list.__iter__", list.__iter__)
info("list.__add__", list.__add__)
info("list.__mul__", list.__mul__)
info("list.__setitem__", list.__setitem__)
info("list.__contains__", list.__contains__)
info("str.__mod__", str.__mod__)
info("str.__rmod__", str.__rmod__)
info("str.__getitem__", str.__getitem__)  # str getitem IS a slot wrapper
info("tuple.__getitem__", tuple.__getitem__)  # tuple getitem IS a slot wrapper
info("bytes.__mod__", bytes.__mod__)
info("int.__add__", int.__add__)
info("int.__mod__", int.__mod__)
info("set.__or__", set.__or__)
info("frozenset.__and__", frozenset.__and__)
info("dict.__setitem__", dict.__setitem__)
info("dict.__iter__", dict.__iter__)
# NOTE: `bool.__and__` is deliberately omitted: CPython reports objclass
# `'bool'` (bool defines its own bitwise slots) while pyrust resolves it to the
# inherited `int.__and__` -> objclass `'int'`.  That is a builtin attr-inherit
# detail orthogonal to slot-wrapper presentation; tracked as a follow-up.


# --- bound method-wrappers ---------------------------------------------------
info("[1].__len__", [1].__len__)
info("[1].__iter__", [1].__iter__)
info("[1].__add__", [1].__add__)
info("'x'.__mod__", "x".__mod__)
info("(5).__add__", (5).__add__)
info("(1,2).__getitem__", (1, 2).__getitem__)
info("frozenset().__or__", frozenset().__or__)


# --- method_descriptor / plain-method forms stay builtin_function_or_method --
# (these are the deferred follow-up class; we only assert they did NOT flip to
#  slot-wrapper)
print(type(dict.__getitem__).__name__ != "wrapper_descriptor")
print(type(set.__contains__).__name__ != "wrapper_descriptor")
print(type(list.__reversed__).__name__ != "wrapper_descriptor")
print(type(list.append).__name__ != "wrapper_descriptor")
print(type([1].append).__name__ != "method-wrapper")
print(type([1].__getitem__).__name__ != "method-wrapper")


# --- hasattr / dir membership matrix (replicated from #2396 battery) ---------
for t in (list, tuple, str, bytes, bytearray, dict, set, frozenset):
    print(t.__name__, hasattr(t, "__iter__"), hasattr(t, "__len__"))
print(hasattr(bytes, "__mod__"), hasattr(str, "__mod__"), hasattr(str, "__rmod__"))
print(hasattr(dict, "__reversed__"), hasattr(list, "__reversed__"))
print("__iter__" in dir(list), "__iter__" in dir([1]))
print("__mod__" in dir(bytes), "__reversed__" in dir(dict))
print(dir(list).count("__iter__"))


# --- #2398: keyword-argument rejection prefix --------------------------------
def kw(label, fn):
    try:
        fn()
        print(label, ":: NO-ERROR")
    except TypeError as e:
        print(label, "::", e)


# anonymous slot wrappers -> "wrapper __X__()"
kw("[1].__add__([2], x=1)", lambda: [1].__add__([2], x=1))
kw("[1].__len__(x=1)", lambda: [1].__len__(x=1))
kw("'x'.__mod__('y', z=1)", lambda: "x".__mod__("y", z=1))
kw("list.__add__([1],[2], x=1)", lambda: list.__add__([1], [2], x=1))
kw("str.__getitem__('a',0,x=1)", lambda: str.__getitem__("a", 0, x=1))
kw("(1,2).__getitem__(0,x=1)", lambda: (1, 2).__getitem__(0, x=1))
# named method-wrappers -> "{type}.__X__()"
kw("[1].__getitem__(0, x=1)", lambda: [1].__getitem__(0, x=1))
kw("{1:2}.__getitem__(1, x=1)", lambda: {1: 2}.__getitem__(1, x=1))
kw("{1}.__contains__(1, x=1)", lambda: {1}.__contains__(1, x=1))
kw("[1].__reversed__(x=1)", lambda: [1].__reversed__(x=1))
kw("{1:2}.__reversed__(x=1)", lambda: {1: 2}.__reversed__(x=1))
# NOTE: `bytes`/`bytearray` `__getitem__`/`__contains__` and
# `frozenset.__contains__` route through a separate per-type `call` path that
# still emits the bare `__X__()` kwarg wording (pre-existing); tracked as a
# follow-up, not asserted here.


# --- wrong-receiver-type + arity wordings on unbound forms (must not regress)-
def err(label, fn):
    try:
        fn()
        print(label, ":: NO-ERROR")
    except TypeError as e:
        print(label, "::", e)


err("list.__len__(5)", lambda: list.__len__(5))
err("list.__iter__(5)", lambda: list.__iter__(5))
err("list.__add__()", lambda: list.__add__())
err("list.__len__([1],[2])", lambda: list.__len__([1], [2]))
err("[1].__add__()", lambda: [1].__add__())
err("[1].__add__([2],[3])", lambda: [1].__add__([2], [3]))
