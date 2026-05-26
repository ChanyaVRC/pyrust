# PEP 3131: Unicode identifiers (ID_Start / ID_Continue).
# All outputs must match CPython 3.12 exactly.

# Greek letters as variable names
α = 1
β = 2
print(α + β)       # 3

π = 3.14159
print(π)           # 3.14159

# Unicode class name
class Θ:
    pass

print(Θ.__name__)  # Θ

# Unicode function name and parameter
def φ(x):
    return x * 2

print(φ(5))        # 10

# CJK characters
变量 = 42
print(变量)        # 42

# Underscore-prefixed unicode identifier
_α = "underscore"
print(_α)          # underscore

# Mixed ASCII + unicode continuation characters
résumé = "cv"
print(résumé)      # cv

# Unicode identifier used in an expression
Ω = 100
print(Ω * 2)       # 200

# ASCII identifiers continue to work
x = 7
_y = 8
z2 = 9
print(x, _y, z2)  # 7 8 9
