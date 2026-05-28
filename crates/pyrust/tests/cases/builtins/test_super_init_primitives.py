# Parity fixture for issues #988, #1004, #1015.
#
# super().__init__() inside a subclass of a primitive built-in type must not
# raise AttributeError.  CPython 3.12 resolves __init__ on each primitive via
# MRO: mutable container types (list/dict/set) provide a real __init__ that
# re-populates the backing store; immutable types (tuple/frozenset/int/str/
# float/bytes/complex) provide a no-op __init__ (the real work happens in
# __new__).

class MyDict(dict):
    def __init__(self):
        super().__init__()

d = MyDict()
print(type(d).__name__)   # MyDict
print(len(d))             # 0

class MyDictWithData(dict):
    def __init__(self, mapping):
        super().__init__(mapping)

d2 = MyDictWithData({"x": 1, "y": 2})
print(sorted(d2.items()))  # [('x', 1), ('y', 2)]

class MyList(list):
    def __init__(self, it=()):
        super().__init__(it)

l = MyList([10, 20, 30])
print(type(l).__name__)   # MyList
print(l)                  # [10, 20, 30]

class MyListEmpty(list):
    def __init__(self):
        super().__init__()

le = MyListEmpty()
print(type(le).__name__)  # MyListEmpty
print(le)                 # []

class MySet(set):
    def __init__(self, it=()):
        super().__init__(it)

s = MySet([1, 2, 3])
print(type(s).__name__)   # MySet
print(sorted(s))          # [1, 2, 3]

class MySetEmpty(set):
    def __init__(self):
        super().__init__()

se = MySetEmpty()
print(type(se).__name__)  # MySetEmpty
print(se)                 # set()

class MyFrozen(frozenset):
    def __init__(self, it=()):
        super().__init__()

f = MyFrozen([1, 2, 3])
print(type(f).__name__)   # MyFrozen
print(sorted(f))          # [1, 2, 3]

class MyTuple(tuple):
    def __new__(cls, it=()):
        return super().__new__(cls, it)
    def __init__(self, it=()):
        super().__init__()

t = MyTuple([4, 5, 6])
print(type(t).__name__)   # MyTuple
print(t)                  # (4, 5, 6)

class MyInt(int):
    def __init__(self, val):
        super().__init__()

i = MyInt(42)
print(type(i).__name__)   # MyInt
print(i)                  # 42

class MyStr(str):
    def __init__(self, val):
        super().__init__()

ms = MyStr("hello")
print(type(ms).__name__)  # MyStr
print(ms)                 # hello

class MyFloat(float):
    def __init__(self, val):
        super().__init__()

mf = MyFloat(3.14)
print(type(mf).__name__)  # MyFloat
print(mf)                 # 3.14

class MyBytes(bytes):
    def __new__(cls, val):
        return super().__new__(cls, val)
    def __init__(self, val):
        super().__init__()

mb = MyBytes(b"hi")
print(type(mb).__name__)  # MyBytes
print(mb)                 # b'hi'

class MyComplex(complex):
    def __init__(self, val):
        super().__init__()

mc = MyComplex(1+2j)
print(type(mc).__name__)  # MyComplex
print(mc)                 # (1+2j)
