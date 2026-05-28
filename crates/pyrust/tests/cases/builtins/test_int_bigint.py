# Parity fixture: int() with BigInt arguments (issue #1504)
# int(x) where x is already a BigInt must return the value unchanged,
# not raise TypeError.

print(int(10**30))           # 1000000000000000000000000000000
print(int(2**100))           # 1267650600228229401496703205376
print(int(-10**30))          # -1000000000000000000000000000000
print(type(int(10**30)).__name__)  # int
print(int(42))               # 42 (small int still works)
print(int(0))                # 0

# Arithmetic that produces BigInt, then int() of that
x = 2**64
print(int(x))                # 18446744073709551616

# int() of a negative BigInt
y = -(3**40)
print(int(y))                # -12157665459056928801

# Double-applying int() is idempotent
z = 10**25
print(int(int(z)))           # 10000000000000000000000000
