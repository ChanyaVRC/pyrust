# Test that complex() accepts string arguments, matching CPython 3.12.

# Basic forms
print(complex("1+2j"))       # (1+2j)
print(complex("3j"))         # 3j
print(complex("-1-2j"))      # (-1-2j)
print(complex("1"))          # (1+0j)

# Whitespace: leading/trailing is OK
print(complex("  1+2j  "))  # (1+2j)

# Bare j variants (no numeric coefficient)
print(complex("j"))          # 1j
print(complex("+j"))         # 1j
print(complex("-j"))         # -1j

# Special float values
print(complex("inf"))        # (inf+0j)
print(complex("infj"))       # infj
print(complex("nan+nanj"))   # (nan+nanj)
print(complex("inf+nanj"))   # (inf+nanj)

# Scientific notation
print(complex("1e2+3.5j"))   # (100+3.5j)
print(complex("1.5e+2-2.5e-1j"))  # (150-0.25j)

# Parenthesized form
print(complex("(1+2j)"))     # (1+2j)

# Error: empty string -> ValueError
try:
    complex("")
except ValueError as e:
    print(e)

# Error: internal whitespace -> ValueError
try:
    complex("1 + 2j")
except ValueError as e:
    print(e)

# Error: second arg with string first arg -> TypeError
try:
    complex("1+2j", 0)
except TypeError as e:
    print(e)
