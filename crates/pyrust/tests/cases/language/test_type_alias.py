# PEP 695 type alias statement: `type X = <expr>`
# Tests basic TypeAliasType creation and attribute access.

# Basic type alias
type Vector = list[float]
print(Vector)            # Vector
print(Vector.__name__)   # Vector
print(Vector.__value__)  # list[float]

# Alias with compound value expression
type Point = tuple[int, int]
print(Point.__name__)    # Point
print(Point.__value__)   # tuple[int, int]

# TypeAliasType name survives reassignment of the variable
type Mapping = dict[str, int]
name_before = Mapping.__name__
Mapping = None
print(name_before)  # Mapping

# Alias in function scope
def make_alias():
    type Inner = list[str]
    return Inner

alias = make_alias()
print(alias.__name__)    # Inner
print(alias.__value__)   # list[str]

# type(alias).__name__ — just check it's a string (module path differs from CPython)
tn = type(alias).__name__
print(isinstance(tn, str))  # True
print(len(tn) > 0)          # True

# `type` as a soft keyword: still valid as a plain identifier
# (test in a function scope so it doesn't shadow the builtin type above)
def test_soft_keyword():
    type = 1
    print(type)   # 1
    type = "ok"
    print(type)   # ok

test_soft_keyword()
