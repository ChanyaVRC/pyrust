# Parity fixture for issue #2964:
#   1. Builtin function / slot wrapper / method descriptor introspection
#      (__self__, __objclass__, __get__, missing-attribute AttributeError)
#   2. Bare-builtin direct-call arity & keyword wording (reversed, sum, format, enumerate)
#   3. dir(int)/dir(bool)/dir(float) __abs__ and object.__repr__ bypass

import builtins
import collections
import math
import re


def mask_addr(s):
    return re.sub(r"0x[0-9a-fA-F]+", "0xADDR", str(s))


def show(label, fn):
    try:
        r = fn()
        print(f"{label}: OK -> {mask_addr(repr(r))}")
    except BaseException as e:
        print(f"{label}: ERR -> {type(e).__name__}: {mask_addr(str(e))}")


# ---------------------------------------------------------------------------
# 1. Builtin function / slot wrapper / method descriptor introspection
# ---------------------------------------------------------------------------
show("len.__self__ is builtins", lambda: len.__self__ is builtins)
show("abs.__self__ is builtins", lambda: abs.__self__ is builtins)
show("sorted.__self__ is builtins", lambda: sorted.__self__ is builtins)
show("print.__self__ is builtins", lambda: print.__self__ is builtins)
show("math.sqrt.__self__ is math", lambda: math.sqrt.__self__ is math)
show("len.__module__", lambda: len.__module__)
show("math.sqrt.__module__", lambda: math.sqrt.__module__)
show("len.__name__", lambda: len.__name__)
show("len.__qualname__", lambda: len.__qualname__)

# __objclass__
show("list.__len__.__objclass__ is list", lambda: list.__len__.__objclass__ is list)
show("int.__abs__.__objclass__ is int", lambda: int.__abs__.__objclass__ is int)
show("bool.__abs__.__objclass__ is int", lambda: bool.__abs__.__objclass__ is int)
show("float.__abs__.__objclass__ is float", lambda: float.__abs__.__objclass__ is float)
show("object.__repr__.__objclass__ is object", lambda: object.__repr__.__objclass__ is object)
show("list.append.__objclass__ is list", lambda: list.append.__objclass__ is list)
show("dict.get.__objclass__ is dict", lambda: dict.get.__objclass__ is dict)
show("str.upper.__objclass__ is str", lambda: str.upper.__objclass__ is str)
show("int.bit_length.__objclass__ is int", lambda: int.bit_length.__objclass__ is int)

# type(x).__name__ and missing-attribute AttributeError consistency
for name, obj in [
    ("len", len),
    ("math.sqrt", math.sqrt),
    ("list.__len__", list.__len__),
    ("int.__abs__", int.__abs__),
    ("object.__repr__", object.__repr__),
    ("list.append", list.append),
    ("dict.get", dict.get),
    ("str.upper", str.upper),
    ("[].append", [].append),
    ("[].__len__", [].__len__),
]:
    show(f"type({name}).__name__", lambda o=obj: type(o).__name__)
    show(f"getattr({name}, 'missing')", lambda o=obj: getattr(o, "missing"))

show("list.__len__.__self__", lambda: list.__len__.__self__)
show("list.append.__self__", lambda: list.append.__self__)
show("len.__objclass__", lambda: len.__objclass__)
show("list.__len__.__module__", lambda: list.__len__.__module__)
show("list.append.__module__", lambda: list.append.__module__)
show("[1, 2].__len__.__self__", lambda: [1, 2].__len__.__self__)
show("[1, 2].append.__self__", lambda: [1, 2].append.__self__)


# __get__ on unbound slot wrappers and method descriptors
class LSub(list):
    pass


class ISub(int):
    pass


show("list.__len__.__get__([1, 2])()", lambda: list.__len__.__get__([1, 2])())
show("list.__len__.__get__(LSub([1, 2]))()", lambda: list.__len__.__get__(LSub([1, 2]))())
show("list.__len__.__get__(None, list) is list.__len__", lambda: list.__len__.__get__(None, list) is list.__len__)
show("list.__len__.__get__([1, 2], list)()", lambda: list.__len__.__get__([1, 2], list)())
show("list.__len__.__get__(None, int) is list.__len__", lambda: list.__len__.__get__(None, int) is list.__len__)
show("list.__len__.__get__(None, 123) is list.__len__", lambda: list.__len__.__get__(None, 123) is list.__len__)
show("list.__len__.__get__(123)", lambda: list.__len__.__get__(123))
show("list.__len__.__get__(123, int)", lambda: list.__len__.__get__(123, int))
show("list.__len__.__get__(None)", lambda: list.__len__.__get__(None))
show("list.__len__.__get__(None, None)", lambda: list.__len__.__get__(None, None))
show("list.__len__.__get__()", lambda: list.__len__.__get__())
show("list.__len__.__get__([1], list, 3)", lambda: list.__len__.__get__([1], list, 3))

def t_list_append_get():
    a = [1]
    b = list.append.__get__(a)
    b(2)
    return (a, type(b).__name__, b.__self__ is a, b.__name__)

show("list.append.__get__", t_list_append_get)
show("list.append.__get__(None, list) is list.append", lambda: list.append.__get__(None, list) is list.append)
show("list.append.__get__(123)", lambda: list.append.__get__(123))
show("dict.get.__get__({'a': 1})('a')", lambda: dict.get.__get__({"a": 1})("a"))
show("str.upper.__get__('hi')()", lambda: str.upper.__get__("hi")())
show("str.upper.__get__(123)", lambda: str.upper.__get__(123))
show("int.__abs__.__get__(-5)()", lambda: int.__abs__.__get__(-5)())
show("int.__abs__.__get__(True)()", lambda: int.__abs__.__get__(True)())
show("int.__abs__.__get__(ISub(-5))()", lambda: int.__abs__.__get__(ISub(-5))())
show("float.__abs__.__get__(-3.5)()", lambda: float.__abs__.__get__(-3.5)())
show("object.__repr__.__get__(1)()", lambda: object.__repr__.__get__(1)())
show("type(list.__len__.__get__([1])).__name__", lambda: type(list.__len__.__get__([1])).__name__)
show("type(object.__repr__.__get__(1)).__name__", lambda: type(object.__repr__.__get__(1)).__name__)


# ---------------------------------------------------------------------------
# 2. Bare-builtin direct-call arity & keyword wording
# ---------------------------------------------------------------------------
show("reversed()", lambda: reversed())
show("reversed(1, 2)", lambda: reversed(1, 2))
show("reversed(seq=[])", lambda: reversed(seq=[]))
show("reversed([], bad=1)", lambda: reversed([], bad=1))

show("sum()", lambda: sum())
show("sum(1, 2, 3)", lambda: sum(1, 2, 3))
show("sum(iterable=[])", lambda: sum(iterable=[]))
show("sum(bad=1)", lambda: sum(bad=1))
show("sum([], bad=1)", lambda: sum([], bad=1))
show("sum([], start=10)", lambda: sum([], start=10))
show("sum([1, 2], start=10)", lambda: sum([1, 2], start=10))
show("sum([1], 10, start=20)", lambda: sum([1], 10, start=20))
show("sum([], None)", lambda: sum([], None))

show("format()", lambda: format())
show("format(1, 'd', 3)", lambda: format(1, "d", 3))
show("format(value=1)", lambda: format(value=1))
show("format(1, format_spec='d')", lambda: format(1, format_spec="d"))
show("format(1, 2)", lambda: format(1, 2))
show("format(1, None)", lambda: format(1, None))

show("enumerate()", lambda: enumerate())
show("enumerate([], 0, 1)", lambda: enumerate([], 0, 1))
show("enumerate([1], 2, 3, bad=1)", lambda: enumerate([1], 2, 3, bad=1))
show("enumerate(iterable=[1, 2])", lambda: list(enumerate(iterable=[1, 2])))
show("enumerate([1, 2], start=5)", lambda: list(enumerate([1, 2], start=5)))
show("enumerate([1, 2], 5, start=6)", lambda: enumerate([1, 2], 5, start=6))
show("enumerate([1, 2], iterable=[3])", lambda: enumerate([1, 2], iterable=[3]))
show("enumerate(start=5)", lambda: enumerate(start=5))
show("enumerate(bad=1)", lambda: enumerate(bad=1))


# ---------------------------------------------------------------------------
# 3. dir(int) __abs__ and object.__repr__ bypass
# ---------------------------------------------------------------------------
show("'__abs__' in dir(int)", lambda: "__abs__" in dir(int))
show("'__abs__' in dir(1)", lambda: "__abs__" in dir(1))
show("'__abs__' in dir(bool)", lambda: "__abs__" in dir(bool))
show("'__abs__' in dir(True)", lambda: "__abs__" in dir(True))
show("'__abs__' in dir(float)", lambda: "__abs__" in dir(float))
show("'__abs__' in dir(complex)", lambda: "__abs__" in dir(complex))
show("'__abs__' in int.__dict__", lambda: "__abs__" in int.__dict__)
show("'__abs__' in bool.__dict__", lambda: "__abs__" in bool.__dict__)
show("bool.__abs__ is int.__abs__", lambda: bool.__abs__ is int.__abs__)
show("'__abs__' in float.__dict__", lambda: "__abs__" in float.__dict__)


class Foo:
    def __repr__(self):
        return "CUSTOM_FOO"


class Plain:
    pass


class IntSub(int):
    def __repr__(self):
        return "CUSTOM_INT"


class ListSub(list):
    def __repr__(self):
        return "CUSTOM_LIST"


class StrSub(str):
    def __repr__(self):
        return "CUSTOM_STR"


class SuperObj:
    def __repr__(self):
        return "WRAP:" + super().__repr__()


class SuperInt(int):
    def __repr__(self):
        return "WRAP_INT:" + super().__repr__()


class CustomMeta(type):
    pass


class MetaInst(metaclass=CustomMeta):
    pass


for label, val in [
    ("1", 1),
    ("True", True),
    ("None", None),
    ("'abc'", "abc"),
    ("3.14", 3.14),
    ("[1, 2]", [1, 2]),
    ("{'a': 1}", {"a": 1}),
    ("(1,)", (1,)),
    ("b'x'", b"x"),
    ("int", int),
    ("object", object),
    ("len", len),
    ("Foo()", Foo()),
    ("Plain()", Plain()),
    ("IntSub(5)", IntSub(5)),
    ("ListSub([1])", ListSub([1])),
    ("StrSub('hi')", StrSub("hi")),
    ("math", math),
    ("deque([1])", collections.deque([1])),
    ("MetaInst", MetaInst),
]:
    show(f"object.__repr__({label})", lambda v=val: object.__repr__(v))

show("object.__repr__()", lambda: object.__repr__())
show("object.__repr__(1, 2)", lambda: object.__repr__(1, 2))
show("object.__repr__(1, a=2)", lambda: object.__repr__(1, a=2))
show("SuperObj()", lambda: repr(SuperObj()))
show("SuperInt(7)", lambda: repr(SuperInt(7)))
show("list.__repr__(ListSub([1, 2]))", lambda: list.__repr__(ListSub([1, 2])))
show("int.__repr__(IntSub(5))", lambda: int.__repr__(IntSub(5)))
show("str.__repr__(StrSub('hi'))", lambda: str.__repr__(StrSub("hi")))
show("object.__str__(1)", lambda: object.__str__(1))
show("object.__str__(Foo())", lambda: object.__str__(Foo()))
show("object.__str__(IntSub(5))", lambda: object.__str__(IntSub(5)))
