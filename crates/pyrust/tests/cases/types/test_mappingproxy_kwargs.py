# Parity fixture for mappingproxy methods rejecting keyword arguments.
# Issue #2687: keys(), values(), items(), get(), and copy() must raise
# TypeError on unexpected keyword arguments, matching CPython 3.12.


class Foo:
    x = 1
    y = "hello"


v = vars(Foo)

for call in [
    lambda: v.keys(z=1),
    lambda: v.values(z=1),
    lambda: v.items(z=1),
    lambda: v.get("x", z=1),
    lambda: v.copy(z=1),
]:
    try:
        call()
        print("no error")
    except TypeError as e:
        print("TypeError:", e)

# Sanity: the same methods still work with valid positional arguments.
print(sorted(v.keys()))
print(v.get("x"))
print(v.get("missing", "default"))
print(type(v.copy()).__name__)
