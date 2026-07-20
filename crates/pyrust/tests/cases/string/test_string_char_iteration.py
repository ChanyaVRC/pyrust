# Iterating a string yields its characters lazily (one inline char per step for
# ASCII strings, #str-lazy) instead of materialising a Vec of char strings on
# every `for` entry. Must be observationally identical to CPython, including the
# non-ASCII / CESU-8 fallback path.

# basic ASCII iteration
out = []
for ch in "hello":
    out.append(ch)
print(out)
print([c for c in "abcdef"])
print("|".join(c for c in "pipe"))
print("".join(c for c in "reversed"[::-1]))

# empty and single-char
for _c in "":
    print("unreachable")
print(list(""), list("x"), list("ab"))

# re-iterating the same string
s = "abc"
print(list(s), list(s), [c for c in s])

# counting / membership style loops
text = "the quick brown fox jumps"
print(sum(1 for c in text if c == "o"))
print([c for c in text if c in "aeiou"])
freq = {}
for c in "mississippi":
    freq[c] = freq.get(c, 0) + 1
print(sorted(freq.items()))

# ascii control chars and spaces
print(list("a\tb\nc d"))

# enumerate / zip over strings
print(list(enumerate("abc")))
print(list(enumerate("xyz", start=1)))
print(list(zip("abc", "de")))
print(dict(zip("abc", range(3))))

# nested iteration
print([[c for c in w] for w in "ab cd ef".split()])

# ── non-ASCII / multi-byte (Materialized fallback) ──────────────────────────
out = []
for ch in "café":
    out.append(ch)
print(out)
print(list("héllo"), [c for c in "naïve"])
print(list("a😀b"), list("mañana"))
print(len("café"), "café"[3])
# mixed ASCII + non-ASCII
print([c for c in "abcé😀xyz"])
# counting over a non-ASCII string
print(sum(1 for c in "aéaéa" if c == "a"))
# codepoints
print([ord(c) for c in "aé😀"])

# a longer ASCII string exercising the lazy path repeatedly
total = 0
for _ in range(3):
    for c in "abcdefghijklmnopqrstuvwxyz":
        total += ord(c)
print(total)
