# Logical ops, comparison chains, is/is not, ternary, print kwargs

# Logical operations
print("logic", "" or "fallback")

# Comparison chains
print("chain-lt", 1 < 2 < 3)
print("chain-false", 1 < 2 > 3)

# is / is not
a = None
print("is-none", a is None)
print("is-not-none", a is not None)

# Ternary operator
val = "yes" if True else "no"
print("ternary", val)

# print() kwargs
print("print-kwargs", "A", "B", sep="-", end="!\n")
print("print-none", sep=None, end=None)
print("print-file-flush", file=None, flush=False)

# Semicolon-separated statements
a = 1; b = 2; print("semicolon", a + b)

# User output containing "File " should be preserved in parity comparison
print("output-with-file", "File test.txt")
print("output-file-quoted", 'File "config.py" found')
