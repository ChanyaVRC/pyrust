"""Parity fixture: user-defined functions and lambdas are hashable by identity (issue #1400).

CPython: any callable (function, lambda, bound method, classmethod) is hashable.
hash(f) returns a stable integer based on object identity; functions can be
used as dict keys and set members.
"""

# User-defined functions
def f():
    pass


def g():
    pass


# hash() returns an int
print(type(hash(f)).__name__)

# Same function hashes to the same value
print(hash(f) == hash(f))

# Distinct functions have distinct hashes (pointers are unique)
print(hash(f) != hash(g))

# Lambda
lam = lambda: None
print(type(hash(lam)).__name__)
print(hash(lam) == hash(lam))

# Functions as dict keys
d = {f: "f", g: "g"}
print(len(d))
print(d[f])
print(d[g])

# Functions in sets
s = {f, g}
print(len(s))
print(f in s)
print(g in s)
# Adding the same function again does not grow the set
s.add(f)
print(len(s))

# Lookup round-trips correctly
d2 = {f: 99}
print(d2[f])

# Bound methods: hash(a.method) == hash(a.method) for same func+instance
class A:
    def method(self):
        pass

    @classmethod
    def cm(cls):
        pass


a = A()
b = A()

m1 = a.method
m2 = a.method  # second binding, same func+instance
print(type(hash(m1)).__name__)
print(hash(m1) == hash(m2))

# Different instance -> different hash
m3 = b.method
print(hash(m1) != hash(m3))

# Bound methods as dict keys
dm = {m1: "m1"}
print(dm[m1])
print(dm[m2])

# Classmethod: equality and dict dedup
# Two references to the same classmethod must compare equal and hash equal,
# so that dict/set operations deduplicate them correctly.
print(type(hash(A.cm)).__name__)
print(hash(A.cm) == hash(A.cm))
print(A.cm == A.cm)

# Classmethod as dict key: updating the key must overwrite, not insert a second entry
dc = {A.cm: "first"}
dc[A.cm] = "second"
print(dc[A.cm])    # second
print(len(dc))     # 1

# Built-in functions
print(type(hash(print)).__name__)
print(type(hash(len)).__name__)
print(hash(print) == hash(print))
print(hash(len) == hash(len))
print(hash(print) != hash(len))

# Built-in functions as dict keys
db = {print: "print", len: "len"}
print(db[print])
print(db[len])
