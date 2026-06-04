# `x in <non-iterable scalar>` must raise TypeError (not RuntimeError) with the
# operand's type name, matching CPython 3.12 (issue #2030).  `not in` raises the
# same error, and `except TypeError` must catch it.

for code in ["1 in 5", "1 in None", "'a' in 5", "1 in 3j", "1 in 2.5", "1 in True", "1 not in 5"]:
    try:
        eval(code)
        print(code, "=> NO ERROR")
    except TypeError as e:
        print(code, "=> TypeError:", e)

# `except TypeError` catches it (a leaked RuntimeError would not).
try:
    1 in 5
except TypeError:
    print("caught by except TypeError")

# Bigint RHS (still a non-iterable scalar).
try:
    1 in (10**30)
except TypeError as e:
    print("bigint:", e)

# Iterable containment is unaffected.
print(3 in [1, 2, 3])
print("b" in "abc")
print(2 in {1: "a", 2: "b"})
print(5 not in (1, 2, 3))
