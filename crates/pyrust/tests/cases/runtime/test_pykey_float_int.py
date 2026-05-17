# dict: float key and int key collapse when equal (CPython parity for #549)
d = {1.0: 'a', 1: 'b'}
print(len(d))    # 1
print(d[1.0])    # b
print(d[1])      # b

# set: {1.0, 1} has one element
s = {1.0, 1}
print(len(s))    # 1

# Non-integer floats remain distinct from ints
d2 = {0.5: 'x', 0: 'y'}
print(len(d2))   # 2

# Negative integer float
d3 = {-1.0: 'a', -1: 'b'}
print(len(d3))   # 1
print(d3[-1.0])  # b
print(d3[-1])    # b

# Bool is a subtype of int; float(True)==1.0 collapses with True
d4 = {True: 'a', 1.0: 'b'}
print(len(d4))   # 1
print(d4[True])  # b

# Large integer-valued float within i64 range
d5 = {42.0: 'a', 42: 'b'}
print(len(d5))   # 1
print(d5[42.0])  # b

# Fractional float 0.5 is distinct from int 0
s2 = {0.5, 0}
print(len(s2))   # 2
