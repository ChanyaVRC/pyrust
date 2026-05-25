# Parity fixture for sequence unpacking — error cases (#1062)

# too many values
try:
    a, b = [1, 2, 3]
except ValueError as e:
    print(type(e).__name__ + ":", e)

# not enough values
try:
    a, b, c = [1, 2]
except ValueError as e:
    print(type(e).__name__ + ":", e)

# normal unpack — must still work
a, b = [1, 2]
print(a, b)

# tuple source
x, y = (10, 20)
print(x, y)

# starred unpack with too few values
try:
    a, b, *c = [1]
except ValueError as e:
    print(type(e).__name__ + ":", e)

# starred unpack with exact minimum
a, *b = [1]
print(a, b)

a, *b, c = [1, 2]
print(a, b, c)

# starred unpack normal case
a, *b, c = [1, 2, 3, 4, 5]
print(a, b, c)

print("unpacking OK")
