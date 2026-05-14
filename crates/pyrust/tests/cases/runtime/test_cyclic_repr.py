# Cycle detection on `repr` / `==` of self-referential collections (#364).
#
# CPython uses a thread-local re-entrancy guard so a structure that points
# back at itself prints as `[...]` / `{...}` / `(...)` and equality between
# a cyclic structure and itself terminates instead of blowing the stack.
# Before the fix, the recursive `Value::repr` / `Value::eq` walked the
# self-reference forever and aborted the pyrust process.
#
# Below we focus on dict-keyed cycles, which are the cleanest way to wire
# up a true alias-shared cycle that both CPython and pyrust observe the
# same way: subscript assignment (`d["k"] = d`) stores the existing dict
# object directly, so both runtimes see `d is d["k"]`.  Lists go through
# `list.append`, which in pyrust currently copies on aliasing — a separate
# concern — so we don't pin the list-self-cycle repr string here.

# 1. Self-referential dict via key — `repr` short-circuits as `{...}`.
d = {}
d["k"] = d
print("dict self-ref repr:", repr(d))
print("dict self-ref str:", str(d))

# 2. Self-referential dict — equality with itself doesn't blow the stack.
print("dict eq self:", d == d)

# 3. Print of a self-referential dict (uses `__repr__` for container values).
print("print self-ref:", d)

# 4. A nested cycle should still produce a finite repr.  We construct one
# entirely with dicts to avoid the list-aliasing wrinkle described above.
n = {}
n["self"] = n
n["other"] = {"back": n}
print("nested cycle:", repr(n))
