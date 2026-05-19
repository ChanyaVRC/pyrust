# bytes() argument-count validation (#744)
# CPython 3.12: 4+ args raises TypeError with "(N given)" suffix.

# Excess args: 4 and 5 arguments
try:
    bytes(1, 2, 3, 4)
except TypeError as e:
    print(f"4 args: TypeError: {e}")

try:
    bytes(1, 2, 3, 4, 5)
except TypeError as e:
    print(f"5 args: TypeError: {e}")

# Boundary cases that must still work
print("0 args:", bytes())
print("1 arg:", bytes(3))

# 2-arg path: non-string source with valid encoding raises its own TypeError
try:
    bytes(3, "utf-8")
except TypeError as e:
    print(f"2 args int+enc: TypeError: {e}")

# 3-arg path: valid string encoding
print("3 args str:", bytes("abc", "utf-8", "strict"))
