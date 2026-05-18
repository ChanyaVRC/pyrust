# Parity fixture for issue #562 — fn_cache MemoKey fix.
#
# The fn_cache previously used Vec<PyKey> as the argument key.  Because
# PR #554 made PyKey::Float(1.0) == PyKey::Int(1) (CPython numeric equality
# invariant), a pure function that branches on type(x) could return a stale
# cached result when called with 1.0 after 1 (or vice versa).
#
# MemoKey wraps PyKey and includes the ValueKind discriminant so Float(1.0)
# and Int(1) are distinct cache entries even though they are equal dict keys.
#
# This fixture verifies the underlying hash/equality invariants that make the
# fix necessary and correct:
#   - equal values of different types hash equal in CPython (dict semantics)
#   - but they are different types, so a type-branching function must not
#     confuse them in the memo cache.

# --- dict/set equality semantics (PyKey) ---
# CPython: equal values are equal dict keys regardless of type.
d = {}
d[1] = 'int'
print(d[1.0])        # int   (float 1.0 looks up the int key)
d[1.0] = 'float'
print(d[1])          # float (same slot, overwritten)
print(len(d))        # 1     (one slot — they are the same key)

# --- hash equality for equal numeric values ---
print(hash(1) == hash(1.0))   # True
print(hash(0) == hash(0.0))   # True
print(hash(-1) == hash(-1.0)) # True
print(hash(2) == hash(2.0))   # True

# --- type is NOT equal ---
print(type(1) == type(1.0))   # False
print(type(1).__name__)        # int
print(type(1.0).__name__)      # float

# --- a function that branches on type() must see the right type ---
# (simulates what the memo cache must preserve)
def type_name(x):
    return type(x).__name__

print(type_name(1))    # int
print(type_name(1.0))  # float
print(type_name(1) == type_name(1.0))  # False

# --- set membership (same key slot) ---
s = {1}
print(1.0 in s)   # True  (equal values)
print(1 in s)     # True
s.add(1.0)
print(len(s))     # 1   (same slot)
