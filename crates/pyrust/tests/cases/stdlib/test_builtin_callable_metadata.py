import math


print(math.sqrt.__name__)
print(math.sqrt.__qualname__)
print(math.sqrt.__module__)
print(repr(math.sqrt))
print(math.sqrt.__call__(81.0))

for operation in ("assign", "delete"):
    try:
        if operation == "assign":
            math.sqrt.extra = 1
        else:
            del math.sqrt.extra
    except Exception as exc:
        print(type(exc).__name__, str(exc))

# Keep one module function at one bytecode call site long enough to exercise
# the monomorphic built-in call cache.  Perfect-square input keeps the expected
# result independent of platform libm rounding.
sqrt = math.sqrt
total = 0
for _ in range(2_000):
    total += int(sqrt(81.0))
print(total)
