# Parity fixture: hash(None) must not be 0 (collision with hash(0)/hash(False))
# and must not be -1 (CPython's internal error sentinel).
# The exact value is process-specific in both CPython and pyrust (derived from
# a static sentinel address), so we only check properties, not the exact value.

h = hash(None)

# Non-zero: must not collide with hash(0) == 0 or hash(False) == 0
print(h != 0)        # True

# Not -1: CPython's tp_hash never returns -1 (remaps it to -2)
print(h != -1)       # True

# Stability within a process
print(hash(None) == hash(None))   # True
print(hash(None) == h)            # True

# None works as a dict key (requires consistent hash)
d = {None: "ok"}
print(d[None])       # ok

# None in sets
s1 = {None}
s2 = {None, None}
print(len(s1))       # 1
print(len(s2))       # 1

# None alongside other hashable values — no spurious collisions with 0/False
print(None not in {0, False})     # True
print(hash(None) != hash(0))      # True
print(hash(None) != hash(False))  # True
