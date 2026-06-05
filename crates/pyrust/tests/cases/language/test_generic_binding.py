# PEP 695 (#2003): generic type parameters are bound as TypeVar objects in the
# function/class scope, so annotations and bodies that reference them resolve
# instead of raising NameError; generic classes are subscriptable.

# ── Function: T usable in parameter and return annotations ───────────────────

def f[T](x: T) -> T:
    return x

print(f(5))                                  # 5
print(f("hi"))                               # hi
print(f.__type_params__[0].__name__)         # T
print(f.__annotations__["x"].__name__)       # T
print(f.__annotations__["return"].__name__)  # T
# CPython keeps the *same* TypeVar object in annotations and __type_params__.
print(f.__annotations__["x"] is f.__type_params__[0])  # True

# ── Function: T nested inside another generic (`list[T]`) ────────────────────

def first[T](x: list[T]) -> T:
    return x[0]

print(first([10, 20]))                       # 10

# ── Function: T referenced in the body at call time ──────────────────────────

def get_tv[T]():
    return T

print(get_tv().__name__)                     # T

# ── Class: T usable in a method annotation and body ──────────────────────────

class C[T]:
    def m(self, x: T):
        return x
    def tv(self):
        return T

print(C().m(3))                              # 3
print(C().tv().__name__)                     # T
print(C.__type_params__[0].__name__)         # T

# ── Generic class is subscriptable and constructs instances ──────────────────

class D[T]:
    def __init__(self):
        self.tag = "D"

alias = D[int]
print(alias.__origin__ is D)                 # True
print(alias.__args__)                        # (<class 'int'>,)
print(repr(D[int]))                          # __main__.D[int]
print(D[int]().tag)                          # D

# ── Generic subclass: `class Sub[T](Base[T])` ────────────────────────────────

class Base[T]:
    def who(self):
        return "base"

class Sub[T](Base[T]):
    pass

print(Sub().who())                           # base
print(issubclass(Sub, Base))                 # True
print(Sub.__type_params__[0].__name__)       # T

# ── Subclassing a built-in generic alias works ───────────────────────────────

class IntList(list[int]):
    pass

print(IntList([1, 2, 3]))                    # [1, 2, 3]

# ── Multiple and bounded type params ─────────────────────────────────────────

def multi[T, U, V](a: T, b: U, c: V):
    return (a, b, c)

print(multi(1, 2, 3))                                  # (1, 2, 3)
print([tp.__name__ for tp in multi.__type_params__])   # ['T', 'U', 'V']

def bounded[T: int](x: T) -> T:
    return x

print(bounded(7))                            # 7
print(bounded.__type_params__[0].__name__)   # T

# ── Generic def nested inside a function ─────────────────────────────────────

def outer():
    def inner[T](x: T) -> T:
        return x
    return inner(99)

print(outer())                               # 99

# ── Decorated generic function still carries __type_params__ ─────────────────

def deco(fn):
    fn.tagged = True
    return fn

@deco
def gd[T](x: T) -> T:
    return x

print(gd(1), gd.tagged, gd.__type_params__[0].__name__)  # 1 True T

# ── Non-generic class remains non-subscriptable ──────────────────────────────

class Plain:
    pass

try:
    Plain[int]
except TypeError as e:
    print("TypeError:", e)                   # TypeError: type 'Plain' is not subscriptable
