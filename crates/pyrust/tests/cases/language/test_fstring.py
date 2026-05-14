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

# !s conversion (str())
assert f"{n!s}" == "42"

# !a conversion (ascii())
assert f"{'café'!a}" == "'caf\\xe9'"

# Method call inside interpolation
items = [1, 2, 3]
assert f"len={len(items)}" == "len=3"
assert f"{'abc'.upper()}" == "ABC"

# Concatenation of f-string with regular string
s = f"a={n}" + "!"
assert s == "a=42!"

# ── Python 3.8 self-documenting debug form `f"{x=}"` ────────────────────────

# Basic: emits "name=" then repr() of the value
x = 42
assert f"{x=}" == "x=42"

# String value uses repr (so quotes are preserved)
s = "hi"
assert f"{s=}" == "s='hi'"

# Explicit !s overrides the implicit repr
assert f"{x=!s}" == "x=42"
# Explicit !r matches the default
assert f"{x=!r}" == "x=42"

# Format spec disables the implicit repr (uses str-like formatting)
y = 1.5
assert f"{y=:.2f}" == "y=1.50"

# Expression source is preserved verbatim, including operators
assert f"{x + 1=}" == "x + 1=43"

# Whitespace inside the braces is preserved in the source label
assert f"{ x =}" == " x =42"
assert f"{x = }" == "x = 42"

# `==` is NOT treated as the debug marker
a, b = 1, 1
assert f"{a == b}" == "True"

# Other comparison operators are not confused with debug
assert f"{a <= b}" == "True"
assert f"{a >= b}" == "True"
assert f"{a != b}" == "False"

# Debug form mixed with regular substitution
assert f"{x=} and {a}" == "x=42 and 1"

# Quoted strings inside the expression
assert f"{'a=b'=}" == "'a=b'='a=b'"

# Function call source preserved
def _f():
    return 7
assert f"{_f()=}" == "_f()=7"

print("f-string OK")
