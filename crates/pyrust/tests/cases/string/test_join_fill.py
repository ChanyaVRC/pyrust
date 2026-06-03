# str.join fills the result's backing buffer directly (no intermediate String)
# and resolves the cached ASCII flag lazily for large results. This exercises
# both the eager (small) and lazy (large, >256 bytes) ASCII paths followed by
# operations that depend on the ASCII flag (index / slice / find), for ASCII
# and non-ASCII output, so a wrong cached flag would diverge from CPython.

# --- small ASCII result: eager ASCII flag ---
s = ",".join(["abc", "def", "ghi"])
print(s, s.index("d"), s[4], len(s))

# --- small non-ASCII result: eager flag, non-ASCII ---
u = "•".join(["café", "naïve"])
print(u, u.index("n"), u[5], len(u))

# --- large ASCII result (>256 bytes): lazy ASCII flag, then ASCII-dependent ops ---
big = ",".join(["abcdefghij"] * 50)        # 549 bytes
print(len(big), big.count(","), big.index("b"), big[-1])

# --- large non-ASCII result (>256 bytes): lazy flag must resolve non-ASCII ---
ub = "·".join(["café"] * 80)               # multibyte, >256 bytes
print(len(ub), ub.count("·"), ub.index("é"), ub[3])

# --- separator-only multibyte across a large join ---
sep = "→".join(["x"] * 200)
print(len(sep), sep.count("→"), sep[0], sep[-1])

# --- str-subclass elements in a list/tuple still coerce to their str value;
#     the fast-path scan must not clone the all-exact-str container (#1927). ---
class S(str):
    pass

print(",".join([S("a"), "b", S("c")]))   # a,b,c
print(",".join((S("x"), S("y"))))        # x,y (tuple of subclasses)
print(",".join(["plain", "exact"]))      # no coercion: returned untouched
