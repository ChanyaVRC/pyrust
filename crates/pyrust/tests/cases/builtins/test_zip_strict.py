# strict=False (default) — current behaviour preserved
print(list(zip([1, 2, 3], [4, 5])))                  # [(1, 4), (2, 5)]
print(list(zip([1, 2, 3], [4, 5], strict=False)))    # [(1, 4), (2, 5)]

# strict=True, equal lengths — happy path
print(list(zip([1, 2], [3, 4], strict=True)))        # [(1, 3), (2, 4)]
print(list(zip(strict=True)))                        # []

# strict=True, mismatched — ValueError
for args in [([1, 2], [3]), ([1], [3, 4]), ([1, 2], [3], [5, 6])]:
    try:
        list(zip(*args, strict=True))
        print("strict-mismatch", "FAIL")
    except ValueError:
        print("strict-mismatch", "ValueError")

# Unexpected kwarg
try:
    list(zip([1], bogus=True))
    print("bogus-kw", "FAIL")
except TypeError:
    print("bogus-kw", "TypeError")
