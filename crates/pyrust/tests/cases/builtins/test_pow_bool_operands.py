# Parity fixture for issue #1921: pow() must treat bool operands as int
# (bool is an int subclass).  A non-negative bool/int exponent yields an int;
# a negative or float exponent yields a float — matching the `**` operator.

def show(label, v):
    print(f"{label} = {v!r}  type={type(v).__name__}")

# ── bool exponent → int result ───────────────────────────────────────────────
show("pow(2, True)", pow(2, True))        # 2  int
show("pow(5, False)", pow(5, False))      # 1  int
show("pow(2, False)", pow(2, False))      # 1  int
show("pow(-2, True)", pow(-2, True))      # -2 int
show("pow(-2, False)", pow(-2, False))    # 1  int

# ── bool base → int result ───────────────────────────────────────────────────
show("pow(True, 2)", pow(True, 2))        # 1  int
show("pow(False, 0)", pow(False, 0))      # 1  int
show("pow(True, True)", pow(True, True))  # 1  int
show("pow(False, True)", pow(False, True))# 0  int
show("pow(True, False)", pow(True, False))# 1  int
show("pow(False, False)", pow(False, False)) # 1 int

# ── bigint base, bool exponent → int (no float collapse) ─────────────────────
show("pow(10**20, True)", pow(10 ** 20, True))    # 100000000000000000000 int
show("pow(10**40, False)", pow(10 ** 40, False))  # 1 int
show("pow(2, 100)", pow(2, 100))                  # bigint via overflow

# ── 3-arg modular pow with bool operands → int ───────────────────────────────
show("pow(2, True, 100)", pow(2, True, 100))      # 2  int
show("pow(2, False, 5)", pow(2, False, 5))        # 1  int
show("pow(True, True, 2)", pow(True, True, 2))    # 1  int
show("pow(3, True, 5)", pow(3, True, 5))          # 3  int

# ── negative / float exponent still yields float ─────────────────────────────
show("pow(2, -1)", pow(2, -1))            # 0.5 float
show("pow(True, -1)", pow(True, -1))      # 1.0 float (negative exp)
show("pow(2.0, 3)", pow(2.0, 3))          # 8.0 float
show("pow(2.5, True)", pow(2.5, True))    # 2.5 float (float base)
show("pow(2, 2.0)", pow(2, 2.0))          # 4.0 float

# ── `**` operator parity (must be unaffected) ────────────────────────────────
show("2 ** True", 2 ** True)              # 2  int
show("2 ** False", 2 ** False)            # 1  int
show("True ** 2", True ** 2)              # 1  int

# ── type assertions ──────────────────────────────────────────────────────────
assert type(pow(2, True)) is int
assert type(pow(5, False)) is int
assert type(pow(True, True)) is int
assert type(pow(2, True, 100)) is int
assert type(pow(2, -1)) is float
assert type(pow(2.0, 3)) is float

print("pow bool operands OK")
