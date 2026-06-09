# Issue #2277: a full slice of an immutable sequence (tuple, bytes) returns the
# original object, matching CPython's tuple/bytes __getitem__ fast path.  The
# short-circuit fires only for the genuinely-full slice after normalization:
# start covers 0, stop covers len (clamped), step == 1.  Partial / stepped
# slices still build a fresh object, and list always copies.

t = (1, 2, 3, 4)

# Full slice in its various spellings -> same object.
print(t[:] is t)            # True
print(t[0:len(t)] is t)     # True
print(t[::1] is t)          # True
print(t[0:4:1] is t)        # True
print(t[0:100] is t)        # True  (stop clamps to len)
print(t[-4:] is t)          # True  (start == 0 after normalize)
print(t[-5:] is t)          # True  (start clamps to 0)
print(t[-100:100] is t)     # True

# Partial / stepped slices -> new object.
print(t[0:3] is t)          # False
print(t[1:4] is t)          # False
print(t[1:] is t)           # False
print(t[0:4:2] is t)        # False
print(t[::-1] is t)         # False
print(t[::2] is t)          # False

# Empty tuple full slice -> same (empty) object.
e = ()
print(e[:] is e)            # True

# Single-element and nested.
o = (42,)
print(o[:] is o)            # True
print(t[:][:] is t)         # True

# id() agrees with `is`.
s = t[:]
print(id(s) == id(t))       # True

# Tuple holding a mutable element: still identity, no copy.
m = (1, [2], 3)
print(m[:] is m)            # True

# bytes: same fast path.
b = b"abcd"
print(b[:] is b)            # True
print(b[0:len(b)] is b)     # True
print(b[0:100] is b)        # True
print(b[::1] is b)          # True
print(b[1:] is b)           # False
print(b[::-1] is b)         # False
bid = b[:]
print(id(bid) == id(b))     # True

# list: a full slice ALWAYS copies (mutable sequence) -> new object.
li = [1, 2, 3]
print(li[:] is li)          # False
print(li[0:len(li)] is li)  # False
print(li[::1] is li)        # False

# Values are still correct, not just identity.
print(t[:])
print(t[1:3])
print(b[:])
print(li[:])
