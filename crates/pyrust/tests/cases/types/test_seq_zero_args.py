# Parity fixture: list.index(), list.count(), tuple.index(), tuple.count()
# raise TypeError (not RuntimeError) when called with zero arguments,
# and the message matches CPython 3.12 exactly.

try:
    [1, 2].index()
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    [1, 2].count()
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    (1, 2).index()
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    (1, 2).count()
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

# Edge cases: empty sequences should also raise TypeError, not RuntimeError
try:
    [].index()
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    [].count()
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    ().index()
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    ().count()
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))
