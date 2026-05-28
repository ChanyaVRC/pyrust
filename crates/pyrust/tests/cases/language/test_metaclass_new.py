# Parity fixture for issue #1385: metaclass.__new__ called during class creation.
# CPython 3.12 reference.

# ── Basic: both __new__ and __init__ are called ──────────────────────────────

class Meta(type):
    def __new__(mcs, name, bases, namespace):
        print(f"Meta.__new__ called: {name}")
        return super().__new__(mcs, name, bases, namespace)
    def __init__(cls, name, bases, namespace):
        print(f"Meta.__init__ called: {name}")
        super().__init__(name, bases, namespace)

class Foo(metaclass=Meta):
    pass

# ── __new__ return value becomes the class object ────────────────────────────

class TrackedMeta(type):
    def __new__(mcs, name, bases, namespace):
        namespace["_created_by"] = "TrackedMeta"
        return super().__new__(mcs, name, bases, namespace)

class Bar(metaclass=TrackedMeta):
    pass

print(Bar._created_by)

# ── __new__ can add class attributes before class is finalised ───────────────

class AttrMeta(type):
    def __new__(mcs, name, bases, namespace):
        namespace["added_in_new"] = 42
        return super().__new__(mcs, name, bases, namespace)
    def __init__(cls, name, bases, namespace):
        cls.added_in_init = 99

class Baz(metaclass=AttrMeta):
    pass

print(Baz.added_in_new)
print(Baz.added_in_init)

# ── Metaclass with only __new__, no __init__ ─────────────────────────────────

class NewOnlyMeta(type):
    def __new__(mcs, name, bases, namespace):
        print(f"NewOnlyMeta.__new__: {name}")
        return super().__new__(mcs, name, bases, namespace)

class Qux(metaclass=NewOnlyMeta):
    pass

# ── Metaclass with only __init__, no __new__ ─────────────────────────────────

class InitOnlyMeta(type):
    def __init__(cls, name, bases, namespace):
        print(f"InitOnlyMeta.__init__: {name}")
        super().__init__(name, bases, namespace)

class Quux(metaclass=InitOnlyMeta):
    pass

# ── Metaclass with bases ──────────────────────────────────────────────────────

class Base:
    pass

class Child(Base, metaclass=Meta):
    pass

# ── type.__new__ via explicit call ──────────────────────────────────────────

class ExplicitMeta(type):
    def __new__(mcs, name, bases, namespace):
        print(f"ExplicitMeta.__new__: {name}")
        return type.__new__(mcs, name, bases, namespace)
    def __init__(cls, name, bases, namespace):
        print(f"ExplicitMeta.__init__: {name}")
        type.__init__(cls, name, bases, namespace)

class Explicit(metaclass=ExplicitMeta):
    pass

# ── Class body attributes visible in __new__ namespace ───────────────────────

class InspectMeta(type):
    def __new__(mcs, name, bases, namespace):
        print("x" in namespace)
        return super().__new__(mcs, name, bases, namespace)

class WithAttr(metaclass=InspectMeta):
    x = 1

# ── Class with methods is still functional after metaclass construction ───────

class MethodMeta(type):
    def __new__(mcs, name, bases, namespace):
        return super().__new__(mcs, name, bases, namespace)

class WithMethod(metaclass=MethodMeta):
    def greet(self):
        return "hello"

obj = WithMethod()
print(obj.greet())
