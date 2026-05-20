# Parity fixtures for divmod() and pow() after migration to the typed-signature
# dialect (#400).  Covers all type combinations and error paths.

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

# ── pow: 2-argument form ──────────────────────────────────────────────────────
assert pow(2, 10) == 1024
assert pow(2, 0) == 1
assert pow(2, 1) == 2
assert pow(-2, 3) == -8
assert pow(0, 0) == 1

# negative exponent → float
assert pow(2, -1) == 0.5
assert pow(4, -1) == 0.25
assert pow(2, -2) == 0.25

# float inputs
assert pow(2.0, 3) == 8.0
assert pow(4.0, 0.5) == 2.0

# bool inputs
assert pow(True, 5) == 1
assert pow(False, 5) == 0
assert pow(2, True) == 2
assert pow(2, False) == 1

# BigInt result
assert pow(2, 100) == 2 ** 100
assert type(pow(2, 100)) is int

# ── pow: 3-argument form ──────────────────────────────────────────────────────
assert pow(2, 10, 1000) == 24
assert pow(3, 4, 5) == 1
assert pow(True, 10, 3) == 1   # True == 1
assert pow(2, True, 3) == 2    # True exponent
assert pow(2, 0, 5) == 1       # 2^0 mod 5

try:
    pow(2, 3, 0)
except ValueError as e:
    print("pow mod=0 ValueError:", e)

try:
    pow(2.0, 3, 5)
except TypeError as e:
    print("pow float-base TypeError:", e)

try:
    pow(2, 3.0, 5)
except TypeError as e:
    print("pow float-exp TypeError:", e)

try:
    pow(2, 3, 5.0)
except TypeError as e:
    print("pow float-mod TypeError:", e)

print("divmod-pow typed-dialect OK")
