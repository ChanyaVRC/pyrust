# enumerate() with arbitrary-precision counter — issue #2125.
# A BigInt `start` used to be rejected with a wrong TypeError, and the counter
# silently wrapped to negative on i64 overflow.  The counter is now BigInt-capable.

# --- BigInt start ---
print(list(enumerate(['x', 'y'], 10**30)))
print(list(enumerate(['x'], 2**63)))
print(list(enumerate(['x'], -(10**30))))
print(list(enumerate('ab', 10**19)))

# --- counter promotes on overflow instead of wrapping negative ---
print(list(enumerate(['a', 'b', 'c'], 2**63 - 2)))

# --- boundary value that still fits i64 ---
print(list(enumerate(['x'], 2**63 - 1)))

# --- small (i64) starts are unaffected ---
print(list(enumerate('abc')))
print(list(enumerate('abc', 5)))
print(list(enumerate('abc', True)))
