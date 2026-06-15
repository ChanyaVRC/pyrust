# Parity fixture for issue #2375: str.format keyword-argument path.
#
# The keyword field renderer resolves names against a borrowed `&str` slice of
# the kwargs (no per-call String re-allocation).  This guards that the borrowed
# key lookup stays byte-identical to CPython for hits, misses, repeats,
# conversions, nested specs, and unicode keys.

# --- basic keyword fields ---
print("{x}".format(x=1))                       # 1
print("{x} {y}".format(x=1, y=2))              # 1 2
print("{x} {y} {x}".format(x="A", y="B"))      # A B A

# --- keyword + positional mixed ---
print("{0} {name}".format("pos", name="kw"))   # pos kw
print("{name} {0}".format("pos", name="kw"))   # kw pos

# --- conversions on keyword fields ---
print("{x!r}".format(x="hi"))                  # 'hi'
print("{x!s}".format(x=3.5))                   # 3.5
print("{x!a}".format(x="café"))                # 'caf\xe9'

# --- nested spec referencing keyword names ---
print("{x:>{w}}".format(x="a", w=5))           #     a
print("{v:.{p}f}".format(v=3.14159, p=2))      # 3.14

# --- unicode keyword names ---
print("{日本}".format(**{"日本": "X"}))         # X

# --- repeated identical template (exercises the parse cache hot path) ---
for i in range(3):
    print("k={k} i={i}".format(k="const", i=i))

# --- missing keyword raises KeyError with the bare key ---
try:
    "{missing}".format(x=1)
except KeyError as e:
    print("KeyError:", e)                      # KeyError: 'missing'

# --- accessor chains on keyword bases ---
print("{d[a]}".format(d={"a": 7}))             # 7
print("{p[0]}-{p[1]}".format(p=("L", "R")))    # L-R
