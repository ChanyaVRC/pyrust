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


class IterTypeError:
    """A TypeError while acquiring the iterator is normalized by join."""

    def __iter__(self):
        raise TypeError("is not an iterator custom")


class NextTypeError:
    """The same TypeError text from __next__ must remain distinguishable."""

    def __iter__(self):
        return self

    def __next__(self):
        raise TypeError("is not an iterator custom")


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

# A TypeError from __iter__ belongs to acquisition and is normalized.
for label, fn in [
    ("bytes", lambda: b"x".join(IterTypeError())),
    ("str", lambda: "x".join(IterTypeError())),
    ("bytearray", lambda: bytearray(b"x").join(IterTypeError())),
]:
    try:
        fn()
    except TypeError as e:
        print(label, "iter-typeerror:", e)

# Even identical wording from __next__ belongs to iteration and is propagated.
for label, fn in [
    ("bytes", lambda: b"x".join(NextTypeError())),
    ("str", lambda: "x".join(NextTypeError())),
    ("bytearray", lambda: bytearray(b"x").join(NextTypeError())),
]:
    try:
        fn()
    except TypeError as e:
        print(label, "next-typeerror:", e)

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
