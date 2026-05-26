# bytes.join() must accept any iterable of bytes-like objects, not just list/tuple.

# List and tuple (baseline)
print(b",".join([b"a", b"b", b"c"]))
print(b",".join((b"x", b"y")))

# iter() wrapping a list
print(b",".join(iter([b"a", b"b"])))

# generator expression
print(b"-".join(x for x in [b"p", b"q", b"r"]))

# map()
print(b",".join(map(lambda x: x, [b"hello", b"world"])))

# map(bytes, ...) producing bytes from int sequences
print(b",".join(map(bytes, [[65], [66]])))

# Empty iterable (list)
print(b",".join([]))

# Empty iterable (generator)
print(b",".join(x for x in []))

# Single element via iter
print(b"X".join(iter([b"only"])))

# TypeError: element is not bytes-like
try:
    b",".join([b"ok", "not bytes"])
except TypeError as e:
    print(f"TypeError: {e}")

# TypeError: argument is not iterable at all
try:
    b",".join(42)
except TypeError as e:
    print(f"TypeError: {e}")
