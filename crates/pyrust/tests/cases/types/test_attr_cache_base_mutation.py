# Regression test for issue #935: inline attribute cache must invalidate when a
# base-class attribute is mutated, not only when the leaf class is mutated.
#
# The guard previously only checked `Child.mutation_version`, which is unchanged
# when `Base.method` is reassigned.  A global class-mutation epoch counter now
# guarantees any class mutation in the hierarchy invalidates cached entries.

class Base:
    def method(self):
        return "original"

class Child(Base):
    pass

# Fill the cache for Child().method by calling it twice (first call fills,
# second call exercises the fast path).
c = Child()
assert c.method() == "original", f"before mutation: {c.method()!r}"
assert c.method() == "original"

# Mutate the base class.  With the bug, Child's cached entry still points to
# the old function because Child.mutation_version did not change.
def replacement(self):
    return "replacement"

Base.method = replacement

# The cache must be invalidated: next call must return "replacement".
result = c.method()
assert result == "replacement", f"after base mutation: {result!r}"

# A fresh Child instance must also see the new method.
result2 = Child().method()
assert result2 == "replacement", f"fresh instance after base mutation: {result2!r}"

# Sanity: mutating the leaf class directly also works (pre-existing behaviour).
def leaf_method(self):
    return "leaf"

Child.method = leaf_method
result3 = c.method()
assert result3 == "leaf", f"after leaf mutation: {result3!r}"

# After deleting the leaf override, the base method comes back.
del Child.method
result4 = c.method()
assert result4 == "replacement", f"after del leaf override: {result4!r}"

# Test via attribute access (GetAttr path, not just CallMethod).
class Base2:
    x = 10

class Child2(Base2):
    pass

d = Child2()
# Access via attribute (not call) to exercise GetAttr cache.
assert d.x == 10
assert d.x == 10  # second access exercises fast path
Base2.x = 42
assert d.x == 42, f"GetAttr after base mutation: {d.x!r}"

print("ok")
