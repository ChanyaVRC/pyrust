class NoAnn:
    x = 1

print(hasattr(NoAnn, '__annotations__'))
print(NoAnn.__annotations__)
print(type(NoAnn.__annotations__).__name__)

# A second class with no annotations also returns empty dict
class Bar:
    pass

print(hasattr(Bar, '__annotations__'))
print(Bar.__annotations__)

# Identity: same dict on repeated access (write-back on first access)
print(NoAnn.__annotations__ is NoAnn.__annotations__)

# Mutation via subscript persists because write-back already happened
NoAnn.__annotations__['y'] = int
print(NoAnn.__annotations__)

# Explicit assignment is reflected on next read
class Baz:
    pass

Baz.__annotations__ = {'z': str}
print(Baz.__annotations__)

# Inheritance: B.__annotations__ is B's own, not A's
class A:
    x: int

class B(A):
    pass

print(B.__annotations__)
