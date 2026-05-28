# Unknown keyword arguments must be rejected before type-checking valid kwargs.
# CPython 3.12 raises the unknown-keyword error first regardless of argument order.

try:
    print("x", sep=1, unknown=2)
except TypeError as e:
    print(e)

try:
    print("x", sep=None, unknown=2)
except TypeError as e:
    print(e)

try:
    print("x", sep=1)
except TypeError as e:
    print(e)

try:
    print(unknown=1)
except TypeError as e:
    print(e)

# Valid usage must still work.
print("x", sep=None, end="\n")
print("a", "b", sep="-", end="!\n")
