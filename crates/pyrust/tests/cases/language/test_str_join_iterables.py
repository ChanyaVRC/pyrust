# str.join() must accept any iterable, not just list/tuple/str/dict.
# Parity fixture for issue #1045.

# --- fast paths (list, tuple, str) ---
print(", ".join(["a", "b", "c"]))        # a, b, c
print(", ".join(("a", "b")))             # a, b
print("-".join("abc"))                   # a-b-c

# --- map object ---
print(", ".join(map(str, [1, 2, 3])))    # 1, 2, 3

# --- filter object ---
print(", ".join(filter(None, ["a", "", "b"])))  # a, b

# --- iter() wrapping a list ---
print("".join(iter(["a", "b"])))         # ab

# --- generator expression ---
print(", ".join(x for x in ["a", "b", "c"]))  # a, b, c

# --- generator function ---
def letters():
    yield "p"
    yield "q"

print(", ".join(letters()))              # p, q

# --- set (order varies; sort output to stabilise) ---
result = list(", ".join({"x"}))
result.sort()
print("".join(result))                   # x

# --- frozenset ---
result = list("".join(frozenset(["z"])))
result.sort()
print("".join(result))                   # z

# --- custom __iter__ class ---
class MyIter:
    def __iter__(self):
        return iter(["m", "n"])

print(",".join(MyIter()))                # m,n

# --- non-string items raise TypeError ---
try:
    ",".join([1, 2])
except TypeError as e:
    print(e)                             # sequence item 0: expected str instance, int found

# --- non-iterable raises TypeError ---
try:
    "".join(42)
except TypeError as e:
    print(e)                             # can only join an iterable

# --- TypeError from inside generator body must propagate unchanged ---
def bad_gen():
    raise TypeError("from generator")
    yield "x"

try:
    "".join(bad_gen())
except TypeError as e:
    print(e)                             # from generator
