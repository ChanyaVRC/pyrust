# Tests for __qualname__ on nested classes (issue #592).
# CPython 3.12 computes the full dotted path at compile time.

# --- Simple nesting ---
class Outer:
    class Inner:
        pass

print(Outer.__qualname__)        # Outer
print(Outer.Inner.__qualname__)  # Outer.Inner

# --- Three levels deep ---
class A:
    class B:
        class C:
            pass

print(A.B.C.__qualname__)  # A.B.C

# --- __name__ is always the bare name ---
print(Outer.Inner.__name__)  # Inner
print(A.B.C.__name__)       # C

# --- __qualname__ readable inside the class body ---
class Outer2:
    class Inner2:
        captured = __qualname__

print(Outer2.Inner2.captured)  # Outer2.Inner2

# --- Class inside a function: CPython uses "fn.<locals>.ClassName" ---
def make():
    class Local:
        pass
    return Local

print(make().__qualname__)  # make.<locals>.Local

# --- Class inside a method ---
class Host:
    def factory(self):
        class Product:
            pass
        return Product

print(Host.factory(None).__qualname__)  # Host.factory.<locals>.Product

# --- Explicit __qualname__ assignment overrides the computed value ---
class Foo:
    __qualname__ = "OverriddenFoo"

print(Foo.__qualname__)  # OverriddenFoo

# --- Top-level class: qualname == name ---
class TopLevel:
    pass

print(TopLevel.__qualname__)  # TopLevel
print(TopLevel.__name__)      # TopLevel

# --- Non-str __qualname__ raises TypeError ---
try:
    class Bad:
        __qualname__ = 42
except TypeError as e:
    print(type(e).__name__)  # TypeError
