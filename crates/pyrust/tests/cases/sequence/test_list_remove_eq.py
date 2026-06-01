# list.remove(x) must dispatch user __eq__ (like in/index/count) and use
# CPython's fixed "list.remove(x): x not in list" miss message.
#
# CPython 3.12 semantics (PyObject_RichCompareBool(item, x, Py_EQ)):
# - removes the FIRST element equal under __eq__ (not identity), in place;
# - identity is checked before __eq__, so an element that `is` x is removed
#   even when its __eq__ would return False;
# - on no match raises ValueError with the literal message above;
# - arity is exactly one argument;
# - a user __eq__ that raises propagates the exception.

class E:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return isinstance(o, E) and self.v == o.v

    def __hash__(self):
        return hash(self.v)


# user __eq__: equal-but-not-identical element is removed
l = [E(1), E(2), E(3)]
l.remove(E(2))
print([x.v for x in l])          # [1, 3]

# plain int / str: value equality
l = [1, 2, 3]
l.remove(2)
print(l)                          # [1, 3]

l = ["a", "b", "c"]
l.remove("b")
print(l)                          # ['a', 'c']

# first match only
l = [1, 2, 1]
l.remove(1)
print(l)                          # [2, 1]


# identity short-circuit: __eq__ returns False but the element IS x
class Always:
    def __eq__(self, o):
        return False

    def __hash__(self):
        return 1


o = Always()
l = [o]
l.remove(o)
print(len(l))                     # 0


# miss: plain values
try:
    [1, 2].remove(9)
except ValueError as e:
    print(e)                      # list.remove(x): x not in list

# miss: user __eq__ elements
try:
    [E(1)].remove(E(9))
except ValueError as e:
    print(e)                      # list.remove(x): x not in list

# arity errors
try:
    [1, 2].remove()
except TypeError as e:
    print(e)                      # list.remove() takes exactly one argument (0 given)

try:
    [1, 2].remove(1, 2)
except TypeError as e:
    print(e)                      # list.remove() takes exactly one argument (2 given)


# __eq__ that raises propagates
class Boom:
    def __eq__(self, o):
        raise RuntimeError("boom")

    def __hash__(self):
        return 1


try:
    [Boom()].remove(Boom())
except RuntimeError as e:
    print(e)                      # boom


# list subclass routes through the same dispatch
class MyList(list):
    pass


m = MyList([E(1), E(2), E(3)])
m.remove(E(2))
print([x.v for x in m], type(m).__name__)   # [1, 3] MyList
