# Parity fixture for issue #706: globals() must return the live module namespace.
#
# In CPython 3.12, globals() returns the actual module dict — the same object
# on every call. Mutations to the returned dict (direct key writes) are visible
# as module globals and vice versa. locals() at module scope is identical to
# globals().

# 1. Identity: globals() returns the same object on every call.
g = globals()
print("identity", g is globals())  # True

# 2. Live view: assigning a new value is reflected in the already-captured dict.
x = 1
g2 = globals()
x = 2
print("live-view", g2["x"])  # 2

# 3. Dict write becomes a global: inserting into the dict via globals() makes
#    the name accessible as a module-level global.
globals()["injected"] = 99
print("dict-write", injected)  # 99

# 4. locals() at module scope returns the same dict as globals().
print("locals-is-globals", locals() is globals())  # True

# 5. Augmented assignment on a user-defined type at module scope must still
#    call __imatmul__ (not fall through to __matmul__).  This regression was
#    caused by the optimizer's BinOpInPlace-to-BinOp downgrade firing on
#    all-env code where num_locals == 0 (all registers look like temps).
class MV:
    def __init__(self, v):
        self.v = v
    def __matmul__(self, other):
        return MV(self.v * 10 + other.v)
    def __imatmul__(self, other):
        self.v = self.v + other.v
        return self

mv = MV(5)
mv @= MV(7)
print("imatmul", mv.v)  # 12, not 57

# 6. del removes from both env and globals dict; subsequent LoadGlobal raises
#    NameError rather than resurrecting the deleted name.
y = 42
del y
try:
    print(y)
    print("del-fail")  # should not reach here
except NameError:
    print("del-ok")  # NameError

# 7. Nonlocal shadowing a same-named module global must resolve to the
#    enclosing function's binding, not the module globals dict.
z = 999
def outer_z():
    z = 0
    def inc_z():
        nonlocal z
        z += 1
    inc_z()
    inc_z()
    return z

print("nonlocal-shadow", outer_z())  # 2
print("global-intact", z)            # 999
