# Parity fixture for issue #994: user classes that inherit from frozenset
# or tuple should populate __builtin_data__ from the constructor argument
# and behave like the base type for subscript, method, and len() operations.

class MyFrozen(frozenset):
    pass

# Construction from an iterable.
f = MyFrozen([1, 2, 3])
print(len(f))                       # 3
print(type(f).__name__)             # MyFrozen
print(isinstance(f, frozenset))     # True
print(isinstance(f, MyFrozen))      # True

# Duplicate elements are deduplicated.
f2 = MyFrozen([1, 2, 2, 3, 3])
print(len(f2))                      # 3

# Empty construction.
f3 = MyFrozen()
print(len(f3))                      # 0

# frozenset methods work via the backing.
print(f.issubset(frozenset([1, 2, 3, 4])))  # True
print(sorted(f))                    # [1, 2, 3]

class MyTuple(tuple):
    pass

# Construction from an iterable.
t = MyTuple([1, 2, 3])
print(len(t))                       # 3
print(type(t).__name__)             # MyTuple
print(isinstance(t, tuple))         # True
print(isinstance(t, MyTuple))       # True

# Indexing.
print(t[0])                         # 1
print(t[2])                         # 3

# Empty construction.
t2 = MyTuple()
print(len(t2))                      # 0

# Tuple methods work via the backing.
t3 = MyTuple([1, 2, 2, 3])
print(t3.count(2))                  # 2

# Subclass with a custom __init__ (extra attributes, content from __new__).
class MyFrozen2(frozenset):
    def __init__(self, iterable=None):
        self.label = "frozen"

f4 = MyFrozen2([10, 20, 30])
print(len(f4))                      # 3
print(f4.label)                     # frozen
print(type(f4).__name__)            # MyFrozen2

class MyTuple2(tuple):
    def __init__(self, iterable=None):
        self.label = "tuple"

t4 = MyTuple2([10, 20, 30])
print(len(t4))                      # 3
print(t4[0])                        # 10
print(t4.label)                     # tuple
print(type(t4).__name__)            # MyTuple2

# Slice subscripting on tuple subclass.
t5 = MyTuple([1, 2, 3, 4, 5])
print(t5[1:3])                      # (2, 3)
print(t5[::2])                      # (1, 3, 5)
print(t5[-2:])                      # (4, 5)
