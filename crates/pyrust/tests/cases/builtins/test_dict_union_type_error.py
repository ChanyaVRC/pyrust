# Issue #1715: dict subclass uses its own type name (not 'dict') in the binary | TypeError.

class D(dict):
    pass


# Binary | with non-dict RHS should report the actual LHS type name.
try:
    D({"a": 1}) | 42
except TypeError as e:
    print(e)

# Plain dict still reports 'dict'.
try:
    {"a": 1} | 42
except TypeError as e:
    print(e)

# Valid dict union still works for both plain dict and dict subclass.
d = {"a": 1} | {"b": 2}
print(d == {"a": 1, "b": 2})

print(D({"a": 1}) | {"b": 2} == {"a": 1, "b": 2})
