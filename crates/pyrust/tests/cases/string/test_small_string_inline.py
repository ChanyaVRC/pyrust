# Small-string optimisation (#2832): strings of <= 5 bytes are stored inline in
# the NaN-box payload instead of on the heap. This must be completely invisible
# to Python semantics — content, equality, hashing, dict/set keys, slicing,
# iteration, concatenation, and (crucially) the format-spec cache, which used to
# key on the spec string's backing pointer and false-hit for inline specs.

# construction / length across the 5-byte inline boundary
for s in ["", "a", "ab", "abc", "abcd", "abcde", "abcdef", "abcdefg"]:
    print(repr(s), len(s), s.upper(), s[::-1])

# equality and ordering (inline vs inline, inline vs heap)
print("abcde" == "abcde", "abcde" == "abcdf", "abcde" == "abcdef")
print(sorted(["ab", "aa", "a", "abc", "", "b"]))
print("a" < "b", "ab" < "abc", "z" > "aa")

# hashing / dict / set keys with short strings
counts = {}
for ch in "mississippi":
    counts[ch] = counts.get(ch, 0) + 1
print(sorted(counts.items()))
print(set("aabbccddee") == {"a", "b", "c", "d", "e"})
d = {chr(65 + i): i for i in range(26)}
print(d["A"], d["Z"], sum(d.values()), "M" in d, "m" in d)

# slicing produces correct short strings (single-char and multi)
t = "hello world"
print(t[0], t[-1], t[6], t[0:5], t[6:11], t[::2], t[2:2])
print(list("hello"), list("héllo"), list(""))

# concatenation crossing the inline boundary
print("ab" + "cd", "ab" + "cdef", ("a" + "b" + "c" + "d" + "e"), "abc" + "def")
x = ""
for c in "abcdefgh":
    x += c
print(x, len(x))

# unicode (multi-byte chars): "é" is 2 bytes, "😀" is 4 bytes — both inline
print("café", len("café"), "café"[3], list("café"))
print("a😀b", len("a😀b"), "a😀b"[1], ord("😀"), chr(0x1F600))

# format-spec cache correctness: a *varying* spec at one call site must not be
# served a stale parse (the inline-pointer false-hit bug, #2832)
x = 3.14159
for p in (1, 2, 3, 4, 5):
    print(p, f"{x:.{p}f}")
def mk(prec):
    return f"{x:.{prec}f}"
print(mk(2), mk(4), mk(2), mk(6))
for w in (3, 6, 9):
    print(f"{'hi':>{w}}|", f"{42:0{w}d}")

# str <-> bytes round trips for short strings
print("abc".encode(), b"abc".decode(), "x".encode(), bytes([104, 105]).decode())

# interning-style identity: equal short strings share bits, so id() matches
print(id("abc") == id("abc"), id("a") == id("a"))
