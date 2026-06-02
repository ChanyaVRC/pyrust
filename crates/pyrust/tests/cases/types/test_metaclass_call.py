# Issue #1956: a metaclass __call__ override intercepts instance construction.
# Cls(*args) is uniformly type(Cls).__call__(Cls, *args); the default
# type.__call__ runs __new__ + __init__, and super().__call__ inside a metaclass
# override chains back to that default.

class CallMeta(type):
    def __call__(cls, *a, **k):
        inst = super().__call__(*a, **k)
        inst.extra = "X"
        return inst


class F(metaclass=CallMeta):
    def __init__(self):
        self.v = 1


f = F()
print(f.v, getattr(f, "extra", "MISSING"))  # 1 X


# Singleton via a metaclass __call__ cache stored on the metaclass.
class Singleton(type):
    _instances = {}

    def __call__(cls, *a, **k):
        if cls not in cls._instances:
            cls._instances[cls] = super().__call__(*a, **k)
        return cls._instances[cls]


class S(metaclass=Singleton):
    def __init__(self):
        self.x = 1


a = S()
b = S()
print(a is b, a.x)  # True 1


# __init__ should run exactly once per default construction.
init_calls = []


class CountMeta(type):
    def __call__(cls, *a, **k):
        return super().__call__(*a, **k)


class C(metaclass=CountMeta):
    def __init__(self):
        init_calls.append(1)


C()
C()
print(len(init_calls))  # 2


# Metaclass __call__ with keyword arguments.
class KMeta(type):
    def __call__(cls, *a, **k):
        inst = super().__call__(*a, **k)
        inst.tag = k.get("tag", "none")
        return inst


class K(metaclass=KMeta):
    def __init__(self, v=0, tag=None):
        self.v = v


k = K(5, tag="hi")
print(k.v, k.tag)  # 5 hi


# Metaclass __call__ inherited from a base metaclass.
class BaseMeta(type):
    def __call__(cls, *a, **k):
        inst = super().__call__(*a, **k)
        inst.b = "base"
        return inst


class SubMeta(BaseMeta):
    pass


class W(metaclass=SubMeta):
    def __init__(self):
        self.v = 9


w = W()
print(w.v, w.b)  # 9 base


# Plain classes (metaclass is `type`) are unaffected.
class P:
    def __init__(self):
        self.y = 7


print(P().y)  # 7
print(type(W).__name__, isinstance(W, SubMeta))  # SubMeta True
