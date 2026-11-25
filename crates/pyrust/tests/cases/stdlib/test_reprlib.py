# CPython 3.12 parity tests for the native `reprlib` module (issue #3248).

import array
import collections
import reprlib

print("all:", reprlib.__all__)
print("aRepr_type:", type(reprlib.aRepr).__name__)
print("defaults:", (
    reprlib.aRepr.maxlevel,
    reprlib.aRepr.maxtuple,
    reprlib.aRepr.maxlist,
    reprlib.aRepr.maxarray,
    reprlib.aRepr.maxdict,
    reprlib.aRepr.maxset,
    reprlib.aRepr.maxfrozenset,
    reprlib.aRepr.maxdeque,
    reprlib.aRepr.maxstring,
    reprlib.aRepr.maxlong,
    reprlib.aRepr.maxother,
    reprlib.aRepr.fillvalue,
    reprlib.aRepr.indent,
))

# Basic types and size limits
print("list_short:", reprlib.repr([1, 2, 3]))
print("list_long:", reprlib.repr(list(range(20))))
print("tuple_empty:", reprlib.repr(()))
print("tuple_one:", reprlib.repr((1,)))
print("tuple_long:", reprlib.repr(tuple(range(10))))
print("dict_empty:", reprlib.repr({}))
print("dict_long:", reprlib.repr({i: i * 10 for i in range(10)}))
print("set_empty:", reprlib.repr(set()))
print("set_long:", reprlib.repr(set(range(10))))
print("fset_empty:", reprlib.repr(frozenset()))
print("fset_long:", reprlib.repr(frozenset(range(10))))
print("deque_empty:", reprlib.repr(collections.deque()))
print("deque_long:", reprlib.repr(collections.deque(range(10))))
print("array_empty:", reprlib.repr(array.array("i")))
print("array_long:", reprlib.repr(array.array("i", range(10))))
print("str_short:", reprlib.repr("hello"))
print("str_long:", reprlib.repr("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"))
print("int_short:", reprlib.repr(12345))
print("int_long:", reprlib.repr(10 ** 60))

# Nesting depth
nested = [[[[[[[1]]]]]]]
print("nested:", reprlib.repr(nested))

# Custom Repr parameters & Python 3.12 indent parameter
r = reprlib.Repr(maxlist=3, maxdict=2, fillvalue="---")
print("custom_list:", r.repr([1, 2, 3, 4, 5]))
print("custom_dict:", r.repr({"a": 1, "b": 2, "c": 3}))

r_ind_int = reprlib.Repr(indent=2, maxlist=3)
print("indent_int:\n" + r_ind_int.repr([1, [2, 3, 4, 5], (6,), {"k": "v"}]))

r_ind_str = reprlib.Repr(indent="..", maxtuple=2)
print("indent_str:\n" + r_ind_str.repr((1, (2, 3, 4), 5)))

r_ind_zero = reprlib.Repr(indent=0)
print("indent_zero:\n" + r_ind_zero.repr([1, [2, 3]]))

# Negative and invalid indent errors
for bad_indent, exc_name in [(-1, "ValueError"), (1.5, "TypeError")]:
    rb = reprlib.Repr(indent=bad_indent)
    try:
        rb.repr([1, 2])
        print("UNEXPECTED", bad_indent)
    except Exception as e:
        print("bad_indent_err:", type(e).__name__ == exc_name, str(e))

# Subclassing Repr with custom repr_<typename>
class Box:
    def __init__(self, v):
        self.v = v

class CustomRepr(reprlib.Repr):
    def repr_Box(self, x, level):
        return f"Box<{self.repr1(x.v, level - 1)}>"

cr = CustomRepr(maxlist=2)
print("custom_subclass:", cr.repr(Box([1, 2, 3, 4])))

# repr_instance exception fallback
class BrokenRepr:
    def __repr__(self):
        raise RuntimeError("boom")

broken_s = reprlib.repr(BrokenRepr())
print("broken_prefix:", broken_s.startswith("<BrokenRepr instance at 0x") and broken_s.endswith(">"))

# recursive_repr decorator
class Node:
    def __init__(self, name):
        self.name = name
        self.children = []

    @reprlib.recursive_repr()
    def __repr__(self):
        return f"Node({self.name!r}, {self.children!r})"

a = Node("a")
b = Node("b")
a.children.append(b)
b.children.append(a)
print("recursive:", repr(a))

class CustomFill:
    def __init__(self):
        self.me = self

    @reprlib.recursive_repr("<loop>")
    def __repr__(self):
        """custom doc"""
        return f"CustomFill({self.me!r})"

cf = CustomFill()
print("recursive_custom:", repr(cf))
print("wrapped_meta:", CustomFill.__repr__.__name__, CustomFill.__repr__.__doc__)
