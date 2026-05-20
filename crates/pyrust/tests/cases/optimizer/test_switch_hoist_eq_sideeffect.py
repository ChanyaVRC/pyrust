# Parity fixture for issue #844: pass_switch_hoist safety for __eq__ side effects.
#
# pass_switch_hoist hoists a LoadGlobal across if/elif chains.  The safety guard
# added in the fix requires the compared constant to be a primitive (int, str, None,
# bool, float) — i.e. the comparison cannot dispatch user __eq__ purely from the
# constant side.
#
# This fixture covers:
#   1. Integer discriminants: the fast-path (int,int) comparison never calls user
#      __eq__, so hoisting is always safe.
#   2. A user-defined __eq__ that logs side effects but does NOT mutate the global
#      being tested — hoisting is safe and output must match CPython 3.12.
#   3. String discriminants: another common primitive case.

# ── Case 1: integer if/elif chain, hoist is safe ────────────────────────────

for val in (1, 2, 3, 99):
    x = val
    if x == 1:
        print("one")
    elif x == 2:
        print("two")
    elif x == 3:
        print("three")
    else:
        print("other")

# ── Case 2: user-defined __eq__ that logs calls but doesn't mutate the global ─
# The global `g` itself is not overwritten by __eq__, so reusing the loaded
# register value is correct even if the optimizer hoists.

_calls = []

class Logged:
    def __init__(self, val):
        self.val = val

    def __eq__(self, other):
        _calls.append(other)
        return self.val == other

g = Logged(2)

if g == 1:
    print("logged-one")
elif g == 2:
    print("logged-two")
elif g == 3:
    print("logged-three")
else:
    print("logged-other")

# Number of __eq__ invocations: CPython calls __eq__ twice (1 fails, 2 matches).
# pyrust must produce the same call count.
print(len(_calls))

# ── Case 3: string discriminants ─────────────────────────────────────────────

for word in ("apple", "banana", "cherry", "durian"):
    s = word
    if s == "apple":
        print("fruit-apple")
    elif s == "banana":
        print("fruit-banana")
    elif s == "cherry":
        print("fruit-cherry")
    else:
        print("fruit-other")
