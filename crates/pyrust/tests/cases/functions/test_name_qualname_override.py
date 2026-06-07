# f.__name__ / f.__qualname__ overrides (#2256): the override state is boxed and
# allocated only when assigned, but the observable behaviour must match CPython —
# defaults fall back to the declared name/qualname, assignment sticks, and the
# two are independent.


def foo():
    pass


print(foo.__name__, foo.__qualname__)

foo.__name__ = "bar"
print(foo.__name__, foo.__qualname__)  # bar foo  (qualname unchanged)

foo.__qualname__ = "Q.bar"
print(foo.__name__, foo.__qualname__)  # bar Q.bar


# a second, untouched function keeps its own defaults (no shared override state)
def baz():
    pass


print(baz.__name__, baz.__qualname__)


# nested / lambda defaults
def outer():
    def inner():
        pass

    return inner


print(outer().__qualname__)  # outer.<locals>.inner
print((lambda: 1).__name__)  # <lambda>


# methods, bound methods, staticmethod, classmethod
class C:
    def m(self):
        pass

    @staticmethod
    def s():
        pass

    @classmethod
    def c(cls):
        pass


print(C.m.__name__, C.m.__qualname__)  # m C.m
o = C()
print(o.m.__name__, o.m.__qualname__)  # m C.m  (bound method delegates)
print(C.s.__name__, C.s.__qualname__)
print(C.c.__name__, C.c.__qualname__)


# non-str assignment raises TypeError
try:
    foo.__name__ = 5
except TypeError:
    print("name TypeError")
try:
    foo.__qualname__ = 5
except TypeError:
    print("qualname TypeError")
