# Parity fixture for bytes slice-arg None handling (issue #796).
# CPython 3.12 treats None for start/end as omitted (0 / len(b)).

b = b'hello world'

# find: None start, None end, mixed
print(b.find(b'l', None, None))    # 2
print(b.find(b'l', None))          # 2
print(b.find(b'l', None, 4))       # 2
print(b.find(b'l', 3, None))       # 3

# rfind
print(b.rfind(b'l', None, None))   # 9
print(b.rfind(b'l', None, 4))      # 3
print(b.rfind(b'l', 4, None))      # 9

# index
print(b.index(b'l', None, None))   # 2
print(b.index(b'l', 3, None))      # 3

# rindex
print(b.rindex(b'l', None, None))  # 9
print(b.rindex(b'l', 4, None))     # 9

# count
print(b.count(b'l', None, None))   # 3
print(b.count(b'l', None, 4))      # 2
print(b.count(b'l', 4, None))      # 1

# startswith / endswith
print(b.startswith(b'hel', None, None))  # True
print(b.startswith(b'wor', 6, None))     # True
print(b.endswith(b'rld', None, None))    # True
print(b.endswith(b'hel', None, 3))       # True

# Wrong type still raises TypeError with the full CPython message
try:
    b.find(b'l', 'x')
except TypeError as e:
    print(e)

try:
    b.find(b'l', None, [])
except TypeError as e:
    print(e)

# Empty bytes with None
print(b''.find(b'x', None, None))        # -1
print(b''.startswith(b'', None, None))   # True
print(b''.endswith(b'', None, None))     # True
