# Parity fixture for divmod() after migration to the typed-signature dialect
# (#400).  pow() remains on the (args) dialect pending macro support for
# mixed-arity overload sets.

# ── divmod: int / int ─────────────────────────────────────────────────────────
assert divmod(7, 3) == (2, 1)
assert divmod(-7, 3) == (-3, 2)
assert divmod(7, -3) == (-3, -2)
assert divmod(-7, -3) == (2, -1)
assert divmod(0, 1) == (0, 0)

try:
    divmod(0, 0)
except ZeroDivisionError as e:
    print("divmod(0,0) ZeroDivisionError:", e)

# ── divmod: bool / int combinations ──────────────────────────────────────────
assert divmod(True, 2) == (0, 1)      # bool/int returns int tuple
assert divmod(2, True) == (2, 0)      # int/bool returns int tuple
assert divmod(True, True) == (1, 0)
assert divmod(False, True) == (0, 0)

try:
    divmod(True, False)
except ZeroDivisionError as e:
    print("divmod(True,False) ZeroDivisionError:", e)

# ── divmod: float / float ─────────────────────────────────────────────────────
assert divmod(7.0, 3.0) == (2.0, 1.0)
assert divmod(1.5, 0.5) == (3.0, 0.0)

try:
    divmod(0.0, 0.0)
except ZeroDivisionError as e:
    print("divmod(0.0,0.0) ZeroDivisionError:", e)

# ── divmod: float / int and int / float ──────────────────────────────────────
assert divmod(7.0, 2) == (3.0, 1.0)
assert divmod(7, 2.0) == (3.0, 1.0)
assert divmod(True, 2.0) == (0.0, 1.0)
assert divmod(2.0, True) == (2.0, 0.0)

# ── divmod: BigInt ────────────────────────────────────────────────────────────
big = 2 ** 100
assert divmod(big, 3) == (422550200076076467165567735125, 1)
assert divmod(big, 2 ** 50) == (1125899906842624, 0)
assert divmod(-big, 3) == (-422550200076076467165567735126, 2)

try:
    divmod(big, 0)
except ZeroDivisionError as e:
    print("divmod(big,0) ZeroDivisionError:", e)

# ── divmod: TypeError for unsupported types ───────────────────────────────────
try:
    divmod("a", "b")
except TypeError as e:
    print("divmod str TypeError:", e)

try:
    divmod(1, "x")
except TypeError as e:
    print("divmod int+str TypeError:", e)

print("divmod typed-dialect OK")
