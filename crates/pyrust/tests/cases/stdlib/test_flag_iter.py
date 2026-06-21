# enum.Flag — iteration, len, bitwise ops, composite/empty values (issue #2769).
#
# Iterating a composite Flag yields its constituent single-bit members in
# definition order; len() counts them; empty (zero) flags iterate to nothing.
# repr/str, the bitwise operators, membership, and invalid-value errors are all
# compared against CPython 3.12 by the parity harness.

from enum import Flag, auto


class Color(Flag):
    RED = auto()
    GREEN = auto()
    BLUE = auto()


# ── auto() resolves to powers of two ─────────────────────────────────────────
print(Color.RED.value, Color.GREEN.value, Color.BLUE.value)  # 1 2 4

# ── iteration over a composite flag (definition order) ───────────────────────
composite = Color.RED | Color.GREEN
for c in composite:
    print(c)                                  # Color.RED / Color.GREEN
print(list(Color.RED | Color.GREEN | Color.BLUE))
print(list(Color.RED))                        # [<Color.RED: 1>]

# ── len() ────────────────────────────────────────────────────────────────────
print(len(composite))                         # 2
print(len(Color.RED))                         # 1

# ── empty / zero flag ────────────────────────────────────────────────────────
empty = Color(0)
print(list(empty))                            # []
print(len(empty))                             # 0
print(repr(empty), str(empty))                # <Color: 0> Color(0)
print(bool(empty), bool(Color.RED))           # False True

# ── repr / str ───────────────────────────────────────────────────────────────
print(repr(composite))                        # <Color.RED|GREEN: 3>
print(str(composite))                         # Color.RED|GREEN
print(repr(Color.RED))                        # <Color.RED: 1>
print(str(Color.RED))                         # Color.RED

# ── definition order is preserved regardless of value ────────────────────────
class Perm(Flag):
    EXECUTE = 4
    WRITE = 2
    READ = 1


print(list(Perm.READ | Perm.EXECUTE))         # [Perm.EXECUTE, Perm.READ]

# ── bitwise operators ────────────────────────────────────────────────────────
print(repr(Color.RED & Color.RED))            # <Color.RED: 1>
print(repr(Color.RED & Color.GREEN))          # <Color: 0>
print(repr(Color.RED | Color.GREEN))          # <Color.RED|GREEN: 3>
print(repr(Color.RED ^ Color.GREEN))          # <Color.RED|GREEN: 3>
print(repr(~Color.RED))                        # <Color.GREEN|BLUE: 6>
print(repr(~(Color.RED | Color.GREEN | Color.BLUE)))  # <Color: 0>

# ── value lookup synthesises composite pseudo-members (cached/identity) ───────
print(repr(Color(3)))                         # <Color.RED|GREEN: 3>
print(Color(3) == (Color.RED | Color.GREEN))  # True
print(Color(3) is Color(3))                   # True

# ── membership ───────────────────────────────────────────────────────────────
print(Color.RED in (Color.RED | Color.GREEN))   # True
print(Color.BLUE in (Color.RED | Color.GREEN))  # False

# ── named composite alias defined in the class body ──────────────────────────
class C2(Flag):
    A = 1
    B = 2
    AB = 3


print(list(C2))                               # [<C2.A: 1>, <C2.B: 2>]
print(repr(C2.AB))                            # <C2.AB: 3>
print(list(C2.AB))                            # [<C2.A: 1>, <C2.B: 2>]

# ── invalid value raises ValueError ──────────────────────────────────────────
try:
    Color(8)
except ValueError as e:
    print(str(e))
