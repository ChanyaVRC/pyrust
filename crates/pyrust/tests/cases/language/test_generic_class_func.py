# PEP 695 generic class and function syntax: `class Foo[T]:` and `def f[T]():`
# Verifies that TypeVar objects are created and attached as __type_params__.

# ── Generic class: single type param ─────────────────────────────────────────

class Stack[T]:
    pass

print(Stack.__type_params__)            # (T,)
T = Stack.__type_params__[0]
print(type(T).__name__)                 # TypeVar
print(T.__name__)                       # T
print(T.__constraints__)                # ()
print(T.__bound__)                      # None

# ── Generic class: multiple type params ──────────────────────────────────────

class Map[K, V]:
    pass

print(Map.__type_params__)              # (K, V)
K, V = Map.__type_params__
print(K.__name__)                       # K
print(V.__name__)                       # V

# ── Generic function: single type param ──────────────────────────────────────

def first[T](lst):
    return lst[0]

print(first.__type_params__)            # (T,)
FT = first.__type_params__[0]
print(type(FT).__name__)                # TypeVar
print(FT.__name__)                      # T

# ── Generic function: multiple type params ────────────────────────────────────

def swap[T, U](a, b):
    return b, a

print(swap.__type_params__)             # (T, U)
ST, SU = swap.__type_params__
print(ST.__name__)                      # T
print(SU.__name__)                      # U

# ── Generic function with bound in type params ────────────────────────────────

class Pair[X, Y: str]:
    pass

print(len(Pair.__type_params__))        # 2
print(Pair.__type_params__[0].__name__) # X
print(Pair.__type_params__[1].__name__) # Y

# ── Non-generic class and function are unaffected (still define / run) ────────

class Plain:
    x = 1

def plain_fn():
    return 2

print(Plain.x)     # 1
print(plain_fn())  # 2

# ── Generic class body runs normally ─────────────────────────────────────────

class Container[T]:
    value = 42
    def get(self):
        return self.value

c = Container()
print(c.get())                          # 42
print(Container.__type_params__[0].__name__)  # T

# ── Generic function executes normally ────────────────────────────────────────

def identity[T](x):
    return x

print(identity(99))                     # 99
print(identity("hi"))                   # hi
print(identity.__type_params__[0].__name__)  # T

# ── Decorator applied after __type_params__ is set ───────────────────────────

seen_type_params = []

def record(f):
    seen_type_params.append(hasattr(f, '__type_params__'))
    return f

@record
class Decorated[T]:
    pass

@record
def decorated_fn[T](x):
    return x

print(seen_type_params)                 # [True, True]
