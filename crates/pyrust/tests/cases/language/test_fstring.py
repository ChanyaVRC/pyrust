name = "world"
assert f"Hello, {name}!" == "Hello, world!"

assert f"{1 + 2}" == "3"

x = 3.14159
assert f"{x:.2f}" == "3.14"

assert f"{{escaped}}" == "{escaped}"

assert f"{True}" == "True"

assert f"{'nested'}" == "nested"

n = 42
assert f"0x{n:x}" == "0x2a"

assert f"{n!r}" == "42"

# Multi-part concatenation
first = "Alice"
last = "Smith"
assert f"{first} {last}" == "Alice Smith"

# Nested expression
assert f"{2 ** 10}" == "1024"

# Empty f-string
assert f"" == ""

# Literal braces mixed with expressions
assert f"{{{n}}}" == "{42}"

# str conversion (default)
assert f"{3.0}" == "3.0"

# format spec: width
assert f"{n:5d}" == "   42"

# format spec: zero-padded
assert f"{n:05d}" == "00042"

print("f-string OK")
