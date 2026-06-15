# Parity fixture for issue #2377: str.format on distinct-template-per-call
# workloads.
#
# Each iteration builds a fresh template string, so the parse cache sees a new
# key every time (and the same key on the second pass).  This guards that a
# first-sighting render and a later cached render of the same template are
# byte-identical to CPython, including the error paths.

# --- distinct template per call, first sighting ---
for k in range(5):
    t = "[{}]" + str(k) + "{}"
    print(t.format(k, k * 10))

# --- the same templates again: now served from the parse cache ---
for k in range(5):
    t = "[{}]" + str(k) + "{}"
    print(t.format(k, k * 10))

# --- distinct templates that raise: error must match on first sighting ---
for k in range(3):
    t = "{}" + str(k) + "{}{}"   # one too few args
    try:
        print(t.format(k, k))
    except IndexError as e:
        print("IndexError:", e)

# --- structural error in a freshly-built template ---
for k in range(2):
    t = "{" + str(k)             # unterminated field
    try:
        print(t.format(k))
    except ValueError as e:
        print("ValueError:", e)

# --- distinct templates with keyword fields ---
for k in range(3):
    t = "{a}" + str(k) + "{b}"
    print(t.format(a=k, b=k + 1))
