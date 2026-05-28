# Parity fixture for issue #1135:
# str.format() and str.format_map() must pass a non-standard format spec to
# the object's __format__ method rather than raising ValueError.

class Foo:
    """User class with a __format__ that accepts arbitrary specs."""
    def __format__(self, spec):
        return f"Foo<{spec}>"

f = Foo()

# ── str.format() ──────────────────────────────────────────────────────────────

# Auto-numbered field
print("{:abc}".format(f))       # Foo<abc>

# Explicit positional field
print("{0:abc}".format(f))      # Foo<abc>

# Named keyword field
print("{foo:abc}".format(foo=f))  # Foo<abc>

# Empty spec still works
print("{:}".format(f))          # Foo<>
print("{}".format(f))           # Foo<>

# Multiple different specs in one template
print("{0:x} {0:y}".format(f))  # Foo<x> Foo<y>

# ── str.format_map() ──────────────────────────────────────────────────────────

print("{key:xyz}".format_map({"key": f}))   # Foo<xyz>
print("{key:}".format_map({"key": f}))      # Foo<>

# ── format() builtin (reference; was already correct) ─────────────────────────

print(format(f, "abc"))   # Foo<abc>
print(format(f, ""))      # Foo<>

# ── f-strings (reference; was already correct) ────────────────────────────────

print(f"{f:abc}")         # Foo<abc>
print(f"{f:}")            # Foo<>

# ── __format__ return-type check applies via str.format() too ─────────────────

class BadFormat:
    def __format__(self, spec):
        return 123  # int, not str

try:
    "{:x}".format(BadFormat())
except TypeError as e:
    print(f"TypeError: {e}")

try:
    "{key:x}".format_map({"key": BadFormat()})
except TypeError as e:
    print(f"TypeError: {e}")

# ── Pure user class (no custom __format__) with non-empty spec ─────────────────

class Plain:
    pass

try:
    "{:x}".format(Plain())
except TypeError as e:
    print(f"TypeError: {e}")

# ── MRO: spec is passed to the class that defines __format__, not the subclass ──

class Base:
    def __format__(self, spec):
        return f"Base<{spec}>"

class Child(Base):
    pass

print("{:q}".format(Child()))   # Base<q>

# ── Primitive subclass: format spec handled by the primitive __format__ ─────────

class MyInt(int):
    pass

print("{:05d}".format(MyInt(7)))   # 00007
