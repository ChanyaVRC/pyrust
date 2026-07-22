# In-place `s += t` string concatenation (#2850).
#
# pyrust grows the left operand's backing in place when it is a uniquely-owned
# heap str (CPython's `unicode_concatenate` / `_PyUnicode_Append` fast path),
# turning `s += "x"` in a loop from O(n²) into O(n).  The optimization must be
# invisible: every observable result must stay byte-identical to CPython 3.12.
# The risk is aliasing corruption (mutating a backing another Value shares), so
# every "shared backing" shape is exercised here and must NOT be mutated.


# ── Basic accumulation (the optimized path) ──────────────────────────────────
s = ""
for _ in range(20):
    s += "x"
print(repr(s), len(s))

# A longer starting string so the backing is heap (> 5 bytes, past SSO).
s = "abcdefghij"
for _ in range(5):
    s += "."
print(repr(s), len(s))


# ── Aliasing: an alias must NOT be mutated by the in-place path ───────────────
# `t = s; s += ...` — CPython rebinds `s` to a new string; `t` is unchanged.
s = "hello world"  # heap
t = s
s += "!!!"
print(repr(s), repr(t))

# Alias captured, then several in-place appends: alias stays at its original.
a = "0123456789"
b = a
a += "X"
a += "Y"
a += "Z"
print(repr(a), repr(b))

# Self-append: `s += s` must produce the concatenation, not a half-mutated string.
s = "abcdef"
s += s
print(repr(s))


# ── Shared / interned literals must fall back (never mutate the other binding) ─
p = "constant_string_value"
q = p
p += "!"
print(repr(p), repr(q))


# ── Small-string-optimization boundary (inline ↔ heap) ───────────────────────
s = "abc"  # inline (3 bytes)
s += "de"  # 5 bytes, still inline
s += "f"  # crosses to 6 bytes -> heap allocation
print(repr(s), len(s))

s = ""  # empty inline
s += "12345"  # 5 bytes, inline
s += "6"  # 6 bytes, heap
print(repr(s), len(s))


# ── Unicode / non-ASCII: byte length + ASCII flag must stay correct ──────────
s = "abcdef"  # ASCII heap
s += "é"  # append non-ASCII -> result is non-ASCII, multi-byte
print(repr(s), s.encode("utf-8"), len(s))

s = "hello world"
s += "🎉"  # 4-byte emoji
print(repr(s), len(s), s.encode("utf-8"))

s = "ééééé"  # non-ASCII heap start
s += "x"  # append ASCII
print(repr(s), len(s))

# Indexing after a non-ASCII append must respect char boundaries.
s = "aaaaaa"
s += "日本語"
print(s[0], s[6], s[-1], len(s), s[6:])


# ── Empty right-hand side is a no-op (identity preserved) ─────────────────────
s = "keepme"
s += ""
print(repr(s), len(s))


# ── Variable right-hand side (BinOpInPlace path) ─────────────────────────────
s = "aaaaaa"
t = "bbb"
s += t
print(repr(s), repr(t))  # t unchanged


# ── str subclass: must fall back (CPython does not use the fast path here) ────
class MyStr(str):
    pass


x = MyStr("hi")
x += "there"
print(repr(x), type(x).__name__)


# ── bytes are unaffected by the str-only optimization ────────────────────────
b = b"abcdef"
b += b"gh"
print(b)


# ── += inside a function (local var — the case CPython optimizes) ─────────────
def build(n, sep):
    s = ""
    for i in range(n):
        s += sep
    return s


print(repr(build(8, "ab")), len(build(8, "ab")))
