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
slot wrappers; those and plain builtin methods (`append`, `upper`) present as
`method_descriptor` -> `<method '<m>' of '<type>' objects>` unbound and as
`<built-in method <m> of <type> object at 0x...>` bound (#2422).

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
# #2424: `bool` defines its own bitwise slot wrappers -> objclass `'bool'`,
# while inherited numeric slots (`bool.__add__`) stay objclass `'int'`.
for d in ("__and__", "__or__", "__xor__", "__rand__", "__ror__", "__rxor__", "__invert__"):
    info("bool." + d, getattr(bool, d))
info("bool.__add__ (inherited)", bool.__add__)
info("int.__and__ (owner int)", int.__and__)


# --- bound method-wrappers ---------------------------------------------------
info("[1].__len__", [1].__len__)
info("[1].__iter__", [1].__iter__)
info("[1].__add__", [1].__add__)
info("'x'.__mod__", "x".__mod__)
info("(5).__add__", (5).__add__)
info("(1,2).__getitem__", (1, 2).__getitem__)
info("frozenset().__or__", frozenset().__or__)
# #2424: bound bool slot wrappers are method-wrappers of `bool`, and calling
# the unbound form dispatches through the shared numeric path.
info("True.__and__", True.__and__)
info("True.__invert__", True.__invert__)
print("bool.__and__(True, False) =", bool.__and__(True, False))
print("bool.__or__(False, True) =", bool.__or__(False, True))
print("bool.__rand__(True, 3) =", bool.__rand__(True, 3))


# --- #2422: method_descriptor (unbound) + bound builtin-method forms ---------
# Empirical rule table from python3.12: the method_descriptor container dunders
# and every plain builtin method present as `method_descriptor`.  Run each twice
# so the cached attribute-resolution path is exercised too.
for _ in range(2):
    info("dict.__getitem__", dict.__getitem__)
    info("dict.__contains__", dict.__contains__)
    info("dict.__reversed__", dict.__reversed__)
    info("list.__getitem__", list.__getitem__)
    info("list.__reversed__", list.__reversed__)
    info("set.__contains__", set.__contains__)
    info("frozenset.__contains__", frozenset.__contains__)
    info("list.append", list.append)
    info("str.upper", str.upper)
    info("dict.get", dict.get)
    info("tuple.count", tuple.count)
    info("bytes.hex", bytes.hex)
    # bound forms carry the receiver identity address ("at 0x...").
    info("[1].append", [1].append)
    info("'x'.upper", "x".upper)
    info("{}.get", {}.get)
    info("[1].__getitem__", [1].__getitem__)
    info("{1}.__contains__", {1}.__contains__)
# free functions / module functions stay builtin_function_or_method.
info("len", len)
info("sorted", sorted)


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
# #2423: bytes/bytearray `__getitem__`/`__contains__` are anonymous slot
# wrappers -> "wrapper __X__()"; `frozenset.__contains__` is a named
# method-wrapper -> "frozenset.__contains__()".  These reach a separate bound
# method-call arm than the str/list/dict/set/tuple forms above.
kw("b'a'.__getitem__(0, x=1)", lambda: b"a".__getitem__(0, x=1))
kw("b'a'.__contains__(1, x=1)", lambda: b"a".__contains__(1, x=1))
kw("bytearray(b'a').__getitem__(0, x=1)", lambda: bytearray(b"a").__getitem__(0, x=1))
kw("bytearray(b'a').__contains__(1, x=1)", lambda: bytearray(b"a").__contains__(1, x=1))
kw("frozenset().__contains__(1, x=1)", lambda: frozenset().__contains__(1, x=1))


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
