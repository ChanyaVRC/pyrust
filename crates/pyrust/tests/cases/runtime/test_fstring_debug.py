x = 42
print(f"{x=}")           # x=42

name = "Alice"
print(f"{name=}")        # name='Alice'

# Expression form
print(f"{1+2=}")         # 1+2=3

# With format spec
print(f"{3.14159=:.2f}") # 3.14159=3.14

# With !s conversion
print(f"{name=!s}")      # name=Alice

# Mixed with other f-string content
print(f"result: {x=}")   # result: x=42

# Whitespace in the source label is preserved verbatim
print(f"{x + 1 = }")     # x + 1 = 43
print(f"{x = }")         # x = 42

# Explicit !r matches the implicit repr
print(f"{name=!r}")      # name='Alice'

# Normal (non-debug) f-string is unaffected
print(f"{x}")            # 42
print(f"{name}")         # Alice
