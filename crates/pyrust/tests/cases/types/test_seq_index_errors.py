# Parity fixture for seq_index error types and messages.
# Covers issue #657 (TypeError for non-integer start/stop) and
# issue #658 (tuple.index ValueError message wording).

# --- TypeError: non-integer start arg ---
try:
    [1, 2, 3].index(1, "a")
except TypeError as e:
    print("list TypeError start:", e)

try:
    (1, 2, 3).index(1, "a")
except TypeError as e:
    print("tuple TypeError start:", e)

# --- TypeError: non-integer stop arg ---
try:
    [1, 2, 3].index(1, 0, "b")
except TypeError as e:
    print("list TypeError stop:", e)

try:
    (1, 2, 3).index(1, 0, "b")
except TypeError as e:
    print("tuple TypeError stop:", e)

# --- ValueError: value not found in list ---
try:
    [1, 2, 3].index(99)
except ValueError as e:
    print("list not found:", e)

# --- ValueError: value not found in tuple (different message format) ---
try:
    (1, 2, 3).index(99)
except ValueError as e:
    print("tuple not found:", e)

# --- Normal usage: happy path ---
print([1, 2, 3].index(2))
print((1, 2, 3).index(3))

# --- Happy path with start/stop bounds ---
print([10, 20, 10, 30].index(10, 1))
print((10, 20, 10, 30).index(10, 1))
