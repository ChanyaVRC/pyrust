# `x in list` / `x in tuple` membership, exercising the single-pass fast scan
# and the dispatch fallback (#2341).  The fast scan compares scalar elements
# with primitive equality and bails to user `__eq__` dispatch on the first
# non-scalar element (or when the searched item itself can dispatch).

# --- all-scalar fast path -------------------------------------------------
print(3 in [1, 2, 3])          # True
print(9 in [1, 2, 3])          # False
print(1 in (1, 2, 3))          # True (tuple)
print(0 in [])                 # False (empty)
print(7 not in [1, 2, 3])      # True
print(2 not in [1, 2, 3])      # False
print(1 in [1])                # single element, hit at front

# --- bool/int/float cross equality (matches CPython ==) -------------------
print(True in [1, 2, 3])       # True
print(1 in [True, False])      # True
print(0 in [False])            # True
print(1.0 in [1])              # True
print(1 in (1.0, 2.0))         # True

# --- item is a dispatching object (user __eq__) ---------------------------
class E:
    def __init__(self, v):
        self.v = v
    def __eq__(self, o):
        return isinstance(o, E) and self.v == o.v
    def __hash__(self):
        return hash(self.v)

print(E(3) in [E(1), E(2), E(3)])   # True
print(E(9) in [E(1), E(2), E(3)])   # False
print(E(2) in (E(1), E(2)))         # True (tuple)

# --- scalar item, dispatching element mid-list (fast scan bails) ----------
print(5 in [1, 2, E(5)])            # False (5 != E(5))
print(2 in [1, 2, E(5)])            # True  (matched before the bail)
print("x" in ["a", E(1), "x"])     # True  (matched after the bail)
print(3 in (1, 2, E(9)))           # False

# --- nested containers (element-wise __eq__) ------------------------------
print([1, 2] in [[1, 2], [3, 4]])  # True
print((1, 2) in [(1, 2), (3, 4)])  # True
print([9] in [[1, 2], [3, 4]])     # False

# --- str membership is unaffected by this path ----------------------------
print("b" in "abc")                # True
print("z" not in "abc")            # True
