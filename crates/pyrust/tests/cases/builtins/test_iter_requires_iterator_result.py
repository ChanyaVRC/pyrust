"""__iter__ must return an iterator, not merely an iterable."""


class ReturnsByteArray:
    def __iter__(self):
        return bytearray(b"ab")


class ReturnsIterator:
    def __iter__(self):
        return iter(bytearray(b"ab"))


for value in (ReturnsByteArray(),):
    try:
        list(value)
    except Exception as exc:
        print(type(exc).__name__, str(exc))

for operation in (
    lambda: iter(ReturnsByteArray()),
    lambda: [value for value in ReturnsByteArray()],
):
    try:
        operation()
    except Exception as exc:
        print(type(exc).__name__, str(exc))

print(list(ReturnsIterator()))

try:
    next(bytearray(b"ab"))
except Exception as exc:
    print(type(exc).__name__, str(exc))
