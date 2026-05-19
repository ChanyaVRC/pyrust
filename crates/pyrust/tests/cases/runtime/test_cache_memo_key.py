# Parity fixture for issue #593 — MemoKey recursive tuple element comparison.
#
# The fn_cache (internal pure-function memoization) previously used
# MemoKey only at the top level.  For a tuple argument like (1, 1.0),
# both outer PyKey values are Tuple — same discriminant — so the
# comparison fell through to PyKey::PartialEq for element-wise comparison,
# where Float(1.0) == Int(1) (CPython numeric equality invariant).  This
# caused (1, 1) and (1, 1.0) to collide in the fn_cache, returning a
# stale result when the function branches on the element type.
#
# MemoKey now recursively wraps Tuple (and FrozenSet) elements as MemoKey,
# so Float(1.0) and Int(1) inside a container are always treated as
# distinct cache keys.
#
# Each scenario is exercised as a plain function (potentially JIT-memoized
# by the compiler via CallMemo for pure callee detection) so that the
# fn_cache is the mechanism under test rather than functools.lru_cache.

# --- Scenario 1: second element type distinguishes the call ---
# (1, 1) and (1, 1.0) must be different cache entries.

def second_type(t):
    return type(t[1]).__name__

r_int = second_type((1, 1))
print(r_int)        # int

r_float = second_type((1, 1.0))
print(r_float)      # float

r_int2 = second_type((1, 1))
print(r_int2)       # int  (cache hit — same key as first call)

# --- Scenario 2: single-element tuples ---

def first_type(t):
    return type(t[0]).__name__

print(first_type((1,)))      # int
print(first_type((1.0,)))    # float
print(first_type((1,)))      # int  (cache hit)

# --- Scenario 3: nested tuples ---
# ((1,),) and ((1.0,),) must be different cache entries.

def nested_first_elem(t):
    return type(t[0][0]).__name__

print(nested_first_elem(((1,),)))     # int
print(nested_first_elem(((1.0,),)))  # float

# --- Scenario 4: identity — value is preserved on miss ---
# Ensures the cache returns the value associated with the key, not a stale one.

def identity_t(t):
    return t

r_a = identity_t((42,))
print(repr(r_a))          # (42,)

r_b = identity_t((42.0,))
print(repr(r_b))          # (42.0,)  — different key, different value

r_a2 = identity_t((42,))
print(repr(r_a2))         # (42,)    — cache hit, value unchanged

# --- Scenario 5: mixed-type multi-element tuples ---
# (1, 2.0) and (1.0, 2) must be different cache entries.

def both_types(t):
    return (type(t[0]).__name__, type(t[1]).__name__)

rt1 = both_types((1, 2.0))
print(rt1)    # ('int', 'float')

rt2 = both_types((1.0, 2))
print(rt2)    # ('float', 'int')

# --- Scenario 6: homogeneous tuple — same-type repeated call is a hit ---

def sum_t(t):
    return t[0] + t[1]

s1 = sum_t((1, 2))
s2 = sum_t((1, 2))
print(s1, s2)   # 3 3

# --- Scenario 7: top-level int vs float still distinct (regression guard) ---
# This was fixed by PR #589; make sure it still holds.

def top_type(x):
    return type(x).__name__

print(top_type(1))      # int
print(top_type(1.0))    # float
print(top_type(1))      # int  (cache hit)
