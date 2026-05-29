# Parity fixture for PEP 487 class keyword arguments forwarded to
# __init_subclass__.  Covers issue #1080.

# ── Basic forwarding ──────────────────────────────────────────────────────────
class Base:
    def __init_subclass__(cls, tag=None, **kwargs):
        super().__init_subclass__(**kwargs)
        print(f"tag={tag!r}")

class Sub(Base, tag="hello"):
    pass
# expected: tag='hello'

class Sub2(Base):
    pass
# expected: tag=None

# ── Unknown kwarg reaches object.__init_subclass__ → TypeError ───────────────
# The error message must name the new class (not 'object').
try:
    class Multi(Base, tag="x", other=42):
        pass
except TypeError as e:
    print(e)
# expected: Multi.__init_subclass__() takes no keyword arguments

# ── Class with no explicit base + unknown kwarg → TypeError ──────────────────
try:
    class A(unknown_kwarg=1):
        pass
except TypeError as e:
    print(e)
# expected: A.__init_subclass__() takes no keyword arguments

# ── Direct class attribute access ────────────────────────────────────────────
# A.__init_subclass__ should be a bound callable whose __name__ is the dunder.
class C:
    pass

print(C.__init_subclass__.__name__)
# expected: __init_subclass__

# Calling it with no kwargs is a no-op (returns None)
result = C.__init_subclass__()
print(result is None)
# expected: True

# Calling with unknown kwargs raises TypeError naming C
try:
    C.__init_subclass__(foo=1)
except TypeError as e:
    print(e)
# expected: C.__init_subclass__() takes no keyword arguments

# ── Multiple inheritance: each base consumes its own kwarg ───────────────────
class B1:
    def __init_subclass__(cls, foo=None, **kwargs):
        super().__init_subclass__(**kwargs)
        print(f"B1: foo={foo!r}")

class B2:
    def __init_subclass__(cls, bar=None, **kwargs):
        super().__init_subclass__(**kwargs)
        print(f"B2: bar={bar!r}")

class D(B1, B2, foo="a", bar="b"):
    pass
# expected: B2: bar='b'
# expected: B1: foo='a'

# ── Inherited __init_subclass__ is called for each subclass ──────────────────
class E(B1, foo="top"):
    pass
# expected: B1: foo='top'

class F(E):
    pass
# expected: B1: foo=None
