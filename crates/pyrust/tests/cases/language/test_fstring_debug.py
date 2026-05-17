# Parity fixture for the f-string debug form f"{x=}" (Python 3.8+).
# Complements the debug-form section in test_fstring.py with additional
# cases: constant-expression source labels, trailing-space labels, and
# the form mixed with a literal prefix.

x = 42
assert f"{x=}" == "x=42", repr(f"{x=}")

name = "Alice"
assert f"{name=}" == "name='Alice'", repr(f"{name=}")

# Expression with constant operands — the source label "1+2" must be
# preserved verbatim even when the compiler const-folds the sub-expression.
assert f"{1+2=}" == "1+2=3", repr(f"{1+2=}")

# Format spec disables the implicit repr (uses __format__ / str-like path)
assert f"{3.14159=:.2f}" == "3.14159=3.14", repr(f"{3.14159=:.2f}")

# !s overrides the implicit repr
assert f"{name=!s}" == "name=Alice", repr(f"{name=!s}")

# Debug form mixed with a leading literal prefix
assert f"result: {x=}" == "result: x=42", repr(f"result: {x=}")

# Whitespace inside braces is preserved verbatim in the source label
assert f"{x + 1 = }" == "x + 1 = 43", repr(f"{x + 1 = }")
assert f"{x = }" == "x = 42", repr(f"{x = }")

# Explicit !r matches the implicit repr default
assert f"{name=!r}" == "name='Alice'", repr(f"{name=!r}")

# Normal (non-debug) f-string is unaffected by the feature
assert f"{x}" == "42", repr(f"{x}")
assert f"{name}" == "Alice", repr(f"{name}")

print("fstring debug OK")
