# Test that all() and any() short-circuit: they stop iterating as soon as the
# result is determined, without consuming the rest of the iterator.

# --- Empty iterables ---
print(all([]))   # True
print(any([]))   # False

# --- Homogeneous inputs ---
print(all([True, True]))    # True
print(any([False, False]))  # False

# --- Normal short-circuit without side effects ---
print(all([1, 0, 1]))   # False
print(any([0, 1, 0]))   # True

# --- all() stops at the first False; generator tail must not be consumed ---
def gen_all_stop():
    yield False
    raise RuntimeError("all() consumed past first False")

try:
    print(all(gen_all_stop()))   # False
except RuntimeError as e:
    print("FAIL:", e)

# --- any() stops at the first True; generator tail must not be consumed ---
def gen_any_stop():
    yield True
    raise RuntimeError("any() consumed past first True")

try:
    print(any(gen_any_stop()))   # True
except RuntimeError as e:
    print("FAIL:", e)

# --- Side-effect count: all() with early exit ---
calls = []
def check_all(x):
    calls.append(x)
    return x > 0

all(check_all(x) for x in [1, -1, 2, 3])
print(calls)   # [1, -1]  — stopped after first False

# --- Side-effect count: any() with early exit ---
calls2 = []
def check_any(x):
    calls2.append(x)
    return x > 0

any(check_any(x) for x in [0, 1, 2, 3])
print(calls2)  # [0, 1]  — stopped after first True

# --- all() exhausts the iterator when all are truthy ---
print(all([1, 2, 3]))   # True

# --- any() exhausts the iterator when all are falsy ---
print(any([0, 0, 0]))   # False

# --- all() on a non-bool-falsy value (empty string) ---
print(all(["a", "", "b"]))  # False
print(any(["", "", "c"]))   # True
