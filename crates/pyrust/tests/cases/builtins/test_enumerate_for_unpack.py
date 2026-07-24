# Fused-unpack / native-iterator fast path for `for i, x in enumerate(seq)`.
# Exercises the shapes the VM specialises (list/tuple/range/str sources with a
# 2-target unpack) plus the fallbacks that must stay on the generic path.

# --- fused 2-tuple unpack over a list ---
out = []
for i, x in enumerate(["a", "b", "c"]):
    out.append((i, x))
print(out)

# --- start offset (positional and keyword) ---
for i, x in enumerate([10, 20], 5):
    print(i, x)
for i, x in enumerate([10, 20], start=100):
    print(i, x)

# --- tuple source ---
for i, x in enumerate(("p", "q")):
    print(i, x)

# --- range source ---
for i, x in enumerate(range(3, 6)):
    print(i, x)

# --- str source ---
for i, c in enumerate("hi"):
    print(i, c)

# --- single-var target keeps the (i, x) tuple identity/type ---
for t in enumerate([9, 8]):
    print(t, type(t).__name__)

# --- empty ---
print(list(enumerate([])))

# --- nested unpack: outer fuses, inner unpacks the pair ---
for i, (a, b) in enumerate([(1, 2), (3, 4)]):
    print(i, a, b)

# --- enumerate(enumerate(...)) ---
for i, (j, v) in enumerate(enumerate(["x", "y"])):
    print(i, j, v)

# --- negative start ---
print(list(enumerate([1, 2], -3)))

# --- single-pass: re-iterating an exhausted enumerate yields nothing ---
e = enumerate([5, 6, 7])
print(list(e))
print(list(e))

# --- partially consumed then loop (must not restart from 0) ---
e2 = enumerate([1, 2, 3, 4], 10)
print(next(e2))
for i, x in e2:
    print(i, x)

# --- fallbacks that must stay correct ---
# generator source (lazy)
def g():
    yield "m"
    yield "n"
for i, x in enumerate(g(), 1):
    print(i, x)

# dict source: size mutation during iteration still raises
d = {"x": 1, "y": 2, "z": 3}
try:
    for i, k in enumerate(d):
        if i == 0:
            d["w"] = 9
    print("no-raise", "FAIL")
except RuntimeError:
    print("dict-mutation", "RuntimeError")
