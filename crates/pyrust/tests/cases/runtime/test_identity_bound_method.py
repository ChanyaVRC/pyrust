# Identity (`is`) and id() for bound methods and related objects (issue #722).
#
# CPython object identity for bound methods is determined by the backing
# allocation address: two aliases of the same bound-method object are
# identical; two distinct attribute lookups produce different objects even
# when they wrap the same function and receiver.

# ── Built-in bound method (list.append, dict.get, …) ─────────────────────────

lst = [1, 2, 3]
a = lst.append
b = lst.append

print(a is a)           # True  — self-identity
print(a is not a)       # False
print(a is b)           # False — two separate lookups produce distinct objects
print(a is not b)       # True

# id() must be non-zero and stable for the same object.
print(id(a) != 0)             # True
print(id(a) == id(a))         # True  — stable across reads
print(id(a) != id(b))         # True  — two distinct objects have different ids

# Alias: assigning a bound method to another variable shares the object.
c = a
print(a is c)           # True
print(id(a) == id(c))   # True

# ── User-defined bound method ─────────────────────────────────────────────────

class Foo:
    def bar(self):
        pass

f = Foo()
m = f.bar
m2 = f.bar

print(m is m)           # True
print(m is m2)          # False — each attribute lookup creates a new binding
print(id(m) != 0)       # True
print(id(m) == id(m))   # True
print(id(m) != id(m2))  # True

# ── Module identity ───────────────────────────────────────────────────────────

import sys
import sys as sys2

print(sys is sys2)          # True  — same module object
print(sys is not sys2)      # False
print(id(sys) == id(sys2))  # True
print(id(sys) != 0)         # True
