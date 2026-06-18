class BadIter:
    """__iter__ returns a non-iterator (an int)."""

    def __iter__(self):
        return 42


class NextBoom:
    """A real iterator whose __next__ raises a user TypeError mid-consumption."""

    def __iter__(self):
        return self

    def __next__(self):
        raise TypeError("boom")


class IterValueError:
    """__iter__ raises a non-TypeError."""

    def __iter__(self):
        raise ValueError("nope")


# __iter__ returns a non-iterator -> "can only join an iterable"
for label, fn in [
    ("bytes", lambda: b"x".join(BadIter())),
    ("str", lambda: "x".join(BadIter())),
    ("bytearray", lambda: bytearray(b"x").join(BadIter())),
]:
    try:
        fn()
    except TypeError as e:
        print(label, "non-iterator:", e)

# TypeError raised inside __next__ during consumption must propagate unchanged.
for label, fn in [
    ("bytes", lambda: b"x".join(NextBoom())),
    ("str", lambda: "x".join(NextBoom())),
    ("bytearray", lambda: bytearray(b"x").join(NextBoom())),
]:
    try:
        fn()
    except TypeError as e:
        print(label, "next-boom:", e)

# Non-TypeError raised inside __iter__ must propagate unchanged.
for label, fn in [
    ("bytes", lambda: b"x".join(IterValueError())),
    ("str", lambda: "x".join(IterValueError())),
    ("bytearray", lambda: bytearray(b"x").join(IterValueError())),
]:
    try:
        fn()
    except ValueError as e:
        print(label, "iter-valueerror:", e)

# Plain non-iterable (no __iter__) -> "can only join an iterable" (no regression).
for label, fn in [
    ("bytes", lambda: b"x".join(42)),
    ("str", lambda: "x".join(42)),
    ("bytearray", lambda: bytearray(b"x").join(42)),
]:
    try:
        fn()
    except TypeError as e:
        print(label, "plain:", e)
