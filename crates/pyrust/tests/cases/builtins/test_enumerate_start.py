print(list(enumerate(["a", "b"], 10)))
print(list(enumerate(["a", "b"], start=10)))
print(list(enumerate(["a", "b"])))
print(list(enumerate([], start=5)))
print(list(enumerate(["x"], -3)))
# Type errors
try:
    list(enumerate(["a"], "bad"))
    print("type-error", "FAIL")
except TypeError:
    print("type-error", "TypeError")
# Duplicate args
try:
    list(enumerate(["a"], 1, start=2))
    print("dup-arg", "FAIL")
except TypeError:
    print("dup-arg", "TypeError")
