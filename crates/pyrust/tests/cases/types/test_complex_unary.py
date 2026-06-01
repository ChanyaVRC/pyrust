# Unary operators on complex numbers (issue #1911).
# CPython's complex.__pos__ returns the value unchanged; __neg__ negates;
# ~complex raises TypeError.

# Unary + returns the value unchanged.
print(+(3 + 4j))
print(+0j)
print(+(1 - 2j))
print(+(-3 - 4j))
print(+complex(0.0, -0.0))

# Unary - negates both parts.
print(-(3 + 4j))
print(-0j)
print(-(1 - 2j))

# +x == x for complex.
x = 2 - 5j
print(+x == x)

# ~complex raises TypeError.
try:
    ~(3 + 4j)
except TypeError:
    print("TypeError")
