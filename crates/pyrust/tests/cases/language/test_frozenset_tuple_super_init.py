# Parity fixture for issue #1004: super().__init__() in a frozenset or tuple
# subclass should not raise AttributeError.  frozenset and tuple are immutable;
# their __init__ is a no-op (data is fixed at __new__ time).

# --- frozenset ---

class MyFrozen(frozenset):
    def __init__(self, it=()):
        super().__init__()  # must not raise

f = MyFrozen([1, 2, 3])
print(len(f))                       # 3
print(isinstance(f, frozenset))     # True
print(isinstance(f, MyFrozen))      # True
print(sorted(f))                    # [1, 2, 3]

# Empty construction.
f2 = MyFrozen()
print(len(f2))                      # 0

# Subclass with extra instance attribute set in __init__.
class MyFrozen2(frozenset):
    def __init__(self, it=()):
        super().__init__()
        self.label = "myfrozen2"

f3 = MyFrozen2([4, 5, 6])
print(len(f3))                      # 3
print(f3.label)                     # myfrozen2

# --- tuple ---

class MyTuple(tuple):
    def __new__(cls, it=()):
        return super().__new__(cls, it)
    def __init__(self, it=()):
        super().__init__()  # must not raise

t = MyTuple([10, 20, 30])
print(len(t))                       # 3
print(t[0])                         # 10
print(t[2])                         # 30
print(isinstance(t, tuple))         # True
print(isinstance(t, MyTuple))       # True

# Empty construction.
t2 = MyTuple()
print(len(t2))                      # 0

# Subclass with extra instance attribute set in __init__.
class MyTuple2(tuple):
    def __new__(cls, it=()):
        return super().__new__(cls, it)
    def __init__(self, it=()):
        super().__init__()
        self.label = "mytuple2"

t3 = MyTuple2([7, 8, 9])
print(len(t3))                      # 3
print(t3[0])                        # 7
print(t3.label)                     # mytuple2

# --- list/dict/set still work (regression check from PR #988) ---

class MyList(list):
    def __init__(self, it=()):
        super().__init__(it)

ml = MyList([1, 2, 3])
print(len(ml))                      # 3
print(ml[0])                        # 1

class MyDict(dict):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)

md = MyDict(a=1, b=2)
print(len(md))                      # 2

class MySet(set):
    def __init__(self, it=()):
        super().__init__(it)

ms = MySet([1, 2, 3])
print(len(ms))                      # 3
