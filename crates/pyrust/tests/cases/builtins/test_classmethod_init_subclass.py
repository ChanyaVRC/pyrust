# Parity fixture for @classmethod __init_subclass__ with named parameters.
# Covers issue #1362: decorator + default-param register collision in the
# compiler caused the decorator to receive itself as its argument.

# ── Regression: @classmethod with only **kwargs (must keep working) ───────────
class Base1:
    @classmethod
    def __init_subclass__(cls, **kwargs):
        print(f"Base1: cls={cls.__name__}, kwargs={kwargs}")

class Child1(Base1):
    pass
# expected: Base1: cls=Child1, kwargs={}

# ── Regression: no @classmethod with named param (must keep working) ─────────
class Base2:
    def __init_subclass__(cls, myarg=None, **kwargs):
        print(f"Base2: myarg={myarg}")

class Child2(Base2):
    pass
# expected: Base2: myarg=None

class Child2b(Base2, myarg=99):
    pass
# expected: Base2: myarg=99

# ── Fix: @classmethod with one named param and a default ─────────────────────
class Base3:
    @classmethod
    def __init_subclass__(cls, myarg=None, **kwargs):
        print(f"Base3: myarg={myarg}")

class Child3(Base3):
    pass
# expected: Base3: myarg=None

class Child3b(Base3, myarg=42):
    pass
# expected: Base3: myarg=42

# ── Fix: class keyword arg forwarded to @classmethod __init_subclass__ ────────
class Base4:
    @classmethod
    def __init_subclass__(cls, x=0, y=0, **kwargs):
        print(f"Base4: x={x}, y={y}")

class Child4(Base4, x=10):
    pass
# expected: Base4: x=10, y=0

# ── Fix: super().__init_subclass__(**kwargs) inside @classmethod hook ─────────
class GrandBase:
    @classmethod
    def __init_subclass__(cls, flag=False, **kwargs):
        print(f"GrandBase: cls={cls.__name__}, flag={flag}")
        super().__init_subclass__(**kwargs)

class Mid(GrandBase):
    pass
# expected: GrandBase: cls=Mid, flag=False

class Leaf(Mid):
    pass
# expected: GrandBase: cls=Leaf, flag=False

class Leaf2(Mid, flag=True):
    pass
# expected: GrandBase: cls=Leaf2, flag=True

# ── Fix: general @decorator on function with one default (broader bug) ────────
def mark(fn):
    fn._marked = True
    return fn

@mark
def greet(name="world"):
    return f"hello {name}"

print(greet())
# expected: hello world
print(hasattr(greet, "_marked") and greet._marked)
# expected: True
