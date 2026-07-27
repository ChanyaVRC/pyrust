# Parity fixture for repeated global reads in if/elif chains (issue #355).
#
# Each branch must resolve the name again. The preceding comparison can dispatch
# user code and replace a live namespace binding before the next branch.
#
# These tests verify that the behaviour is identical to CPython 3.12 across:
#   - integer discriminants (the common case)
#   - string discriminants
#   - the else branch (fall-through after all tests fail)
#   - chains of varying lengths (2-way, 3-way, 4-way)
#   - the same pattern inside a function (local discriminant — already optimal,
#     but must not regress)

# ── Integer if/elif chain (module-level global) ──────────────────────────────

x = 1
if x == 1:
    print("x=one")
elif x == 2:
    print("x=two")
elif x == 3:
    print("x=three")
else:
    print("x=other")

x = 2
if x == 1:
    print("x=one")
elif x == 2:
    print("x=two")
elif x == 3:
    print("x=three")
else:
    print("x=other")

x = 4
if x == 1:
    print("x=one")
elif x == 2:
    print("x=two")
elif x == 3:
    print("x=three")
else:
    print("x=other")

# ── String if/elif chain (module-level global) ───────────────────────────────

code = "b"
if code == "a":
    print("alpha")
elif code == "b":
    print("beta")
elif code == "c":
    print("gamma")
else:
    print("unknown")

code = "z"
if code == "a":
    print("alpha")
elif code == "b":
    print("beta")
elif code == "c":
    print("gamma")
else:
    print("unknown")

# ── Same pattern inside a function (local variable — already optimal) ────────

def dispatch(v):
    if v == 10:
        return "ten"
    elif v == 20:
        return "twenty"
    elif v == 30:
        return "thirty"
    else:
        return "other"

print(dispatch(10))
print(dispatch(20))
print(dispatch(30))
print(dispatch(99))

# ── Two-way chain (minimal case) ─────────────────────────────────────────────

y = 0
if y == 0:
    print("zero")
elif y == 1:
    print("one")

y = 1
if y == 0:
    print("zero")
elif y == 1:
    print("one")

# ── Four-way chain ───────────────────────────────────────────────────────────

season = "summer"
if season == "spring":
    print("flowers")
elif season == "summer":
    print("heat")
elif season == "autumn":
    print("leaves")
elif season == "winter":
    print("snow")
else:
    print("unknown season")

season = "winter"
if season == "spring":
    print("flowers")
elif season == "summer":
    print("heat")
elif season == "autumn":
    print("leaves")
elif season == "winter":
    print("snow")
else:
    print("unknown season")
