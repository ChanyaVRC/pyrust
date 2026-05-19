# CPython 3.12 parity fixture for tuple.index ValueError message

try:
    (1, 2, 3).index(99)
except ValueError as e:
    print(e)  # tuple.index(x): x not in tuple

# with start
try:
    (1, 2, 3).index(99, 1)
except ValueError as e:
    print(e)  # tuple.index(x): x not in tuple

# with start and stop
try:
    (1, 2, 3).index(99, 0, 2)
except ValueError as e:
    print(e)  # tuple.index(x): x not in tuple

# non-int element
try:
    ("a", "b").index("z")
except ValueError as e:
    print(e)  # tuple.index(x): x not in tuple

# list.index message is unaffected
try:
    [1, 2, 3].index(99)
except ValueError as e:
    print(e)  # 99 is not in list

# successful index — no error
print((1, 2, 3).index(2))  # 1
