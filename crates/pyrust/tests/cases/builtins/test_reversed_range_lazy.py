# reversed(range) computes the reverse cursor arithmetically. It must not drain
# the forward range before yielding its first value.

normal = reversed(range(1, 10, 3))
print("normal type:", type(normal).__name__)
print("normal identity:", iter(normal) is normal)
print("normal:", list(normal))

descending = reversed(range(9, -3, -2))
print("descending:", list(descending))

huge = reversed(range(10**9))
print("huge prefix:", type(huge).__name__, next(huge), next(huge))

# Bounds outside i64 use CPython's longrange_iterator shape but remain lazy.
big_bounds = reversed(range(10**20, 10**20 + 4))
print(
    "big bounds:",
    type(big_bounds).__name__,
    next(big_bounds),
    list(big_bounds),
)

big_length = reversed(range(10**20))
print(
    "big length prefix:",
    type(big_length).__name__,
    next(big_length),
    next(big_length),
)

# Even with i64 bounds, a length larger than Py_ssize_t uses the big cursor.
wide = reversed(range(-(2**63), 2**63 - 1))
print("wide prefix:", type(wide).__name__, next(wide), next(wide))
