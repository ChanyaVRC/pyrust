# Parity fixture for sum() str/bytes rejection.
# CPython 3.12 raises TypeError when start is str or bytes.

# str start raises immediately, even for empty iterable
try:
    sum([], "")
except TypeError as e:
    print("sum([], str):", e)

try:
    sum(["a", "b"], "")
except TypeError as e:
    print("sum(str_list, str):", e)

# bytes start raises immediately, even for empty iterable
try:
    sum([], b"")
except TypeError as e:
    print("sum([], bytes):", e)

try:
    sum([b"a", b"b"], b"")
except TypeError as e:
    print("sum(bytes_list, bytes):", e)

# Happy paths — these must still work
print(sum([1, 2, 3]))
print(sum([1.0, 2.0]))
print(sum([], 0))
print(sum([1, 2], 10))
print(sum([[1, 2], [3, 4]], []))
