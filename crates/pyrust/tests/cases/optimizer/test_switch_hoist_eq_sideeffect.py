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

# ── Case 4: user-defined __eq__ that MUTATES the global between elif branches ─
# This is the core case motivating issue #844.  If pass_switch_hoist hoisted
# LoadGlobal(g) and reused the cached register across the elif, mutating g in
# __eq__ would not be visible to the second test — pyrust would compare the
# stale cached object, and the elif that should match would be skipped.
#
# The guard (primitive-const check on the RHS constant) ensures the hoist only
# fires when the VM's fast int-int / str-str path applies, which never invokes
# user __eq__ and therefore cannot mutate the global.
#
# Expected (CPython 3.12 and correct pyrust):
#   - __eq__ called once with other=1; sets g to an int (2)
#   - elif g == 2: re-reads g (now 2), fast int==int path → True → "mutated-match"
# If hoist fires incorrectly:
#   - elif would compare cached Switcher against 2 → calls __eq__ again → "no-match"

_mut_log = []

class Switcher:
    def __eq__(self, other):
        global g
        _mut_log.append(other)
        g = 2            # replace the global with a plain int
        return False     # this object never matches directly

g = Switcher()

def _run_mut():
    global g
    if g == 1:
        print("mutated-one")
    elif g == 2:
        print("mutated-match")
    else:
        print("mutated-no-match")

_run_mut()
# One __eq__ call (for the first branch); second branch uses re-read g == 2 (int fast path).
print(len(_mut_log))
