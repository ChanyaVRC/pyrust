# CPython 3.12 parity fixture for bytes(bigint) → OverflowError

try:
    bytes(2**63 + 5)
except OverflowError as e:
    print("OverflowError:", e)

try:
    bytes(10**20)
except OverflowError as e:
    print("OverflowError:", e)

# Negative BigInt raises OverflowError (range check before sign check)
try:
    bytes(-(2**63 + 5))
except OverflowError as e:
    print("OverflowError:", e)

# Regression guards: small non-negative int still works
print(bytes(0))
print(bytes(1))

# Negative int still raises ValueError
try:
    bytes(-1)
except ValueError as e:
    print("ValueError:", e)
