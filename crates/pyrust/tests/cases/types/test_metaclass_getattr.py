# Issue #1960: a metaclass __getattr__ is consulted for a missing class
# attribute (CPython falls back to type(cls).__getattr__(cls, name)).

class Meta(type):
    def __getattr__(cls, name):
        return f"meta-{name}"


class Q(metaclass=Meta):
    pass


print(Q.anything)  # meta-anything
print(Q.foo)       # meta-foo


# __getattr__ only fires on a miss; real class attributes win.
class Meta2(type):
    def __getattr__(cls, name):
        return "FALLBACK"


class C(metaclass=Meta2):
    real = 42


print(C.real)     # 42
print(C.missing)  # FALLBACK


# A plain metaclass attribute (no __getattr__) is reachable via cls.attr.
class Meta3(type):
    shared = "from-meta"

    def describe(cls):
        return "I am " + cls.__name__


class D(metaclass=Meta3):
    pass


print(D.shared)        # from-meta
print(D.describe())    # I am D


# A class's own attribute shadows a same-named metaclass attribute.
class Meta4(type):
    name_attr = "meta"


class E(metaclass=Meta4):
    name_attr = "class"


print(E.name_attr)  # class


# A metaclass with no __getattr__ still raises AttributeError on a real miss.
class Meta5(type):
    pass


class G(metaclass=Meta5):
    pass


try:
    G.nope
    print("NO ERROR")
except AttributeError as exc:
    print(exc)  # type object 'G' has no attribute 'nope'
