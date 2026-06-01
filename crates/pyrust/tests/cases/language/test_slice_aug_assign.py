# Augmented assignment to a slice target: `l[a:b] OP= rhs`.
# Equivalent to `l[a:b] = l[a:b] OP rhs` — read the slice, apply the op,
# write the result back. The container is evaluated exactly once.
#
# Each case is wrapped in its own function so the cases keep independent
# register allocation (module scope shares one register space).


def basic_add():
    # += extends the extracted sublist, then writes it back.
    l = [1, 2, 3, 4, 5]
    l[1:3] += [99]
    print(l)  # [1, 2, 99, 4, 5]


def basic_mul():
    # *= repeats the extracted sublist.
    l = [1, 2, 3, 4, 5]
    l[1:3] *= 2
    print(l)  # [1, 2, 3, 2, 3, 4, 5]


def bounds():
    l = [1, 2, 3, 4, 5]
    l[:2] += [9]
    print(l)  # [1, 2, 9, 3, 4, 5]

    l = [1, 2, 3, 4, 5]
    l[-2:] += [9]
    print(l)  # [1, 2, 3, 4, 5, 9]

    l = [1, 2, 3, 4, 5]
    l[-3:-1] += [88]
    print(l)  # [1, 2, 3, 88, 4, 5]

    # Empty slice is an insertion point.
    l = [1, 2, 3]
    l[1:1] += [99]
    print(l)  # [1, 99, 2, 3]


def iterable_rhs():
    # Any iterable is accepted for list += (string, tuple).
    l = [1, 2, 3, 4, 5]
    l[1:3] += "ab"
    print(l)  # [1, 2, 3, 'a', 'b', 4, 5]

    l = [1, 2, 3]
    l[1:2] += (9,)
    print(l)  # [1, 2, 9, 3]


def extended_slice():
    # Extended slice (step != 1) with a length-matching RHS.
    l = [1, 2, 3, 4, 5]
    l[::2] += []
    print(l)  # [1, 2, 3, 4, 5]

    l = [1, 2, 3, 4, 5]
    l[::2] *= 1
    print(l)  # [1, 2, 3, 4, 5]

    # Extended slice with a length mismatch raises ValueError.
    l = [1, 2, 3, 4, 5]
    try:
        l[::2] += [10, 20]
    except ValueError as e:
        print(type(e).__name__, e)
    # ValueError attempt to assign sequence of size 5 to extended slice of size 3


def mul_zero():
    # Whole-slice *= 0 clears in place.
    l = [1, 2, 3]
    l[:] *= 0
    print(l)  # []


def single_eval():
    # Container evaluated exactly once (side-effect appended once).
    calls = []

    def get():
        calls.append(1)
        return [1, 2, 3, 4, 5]

    get()[1:3] += [99]
    print(len(calls))  # 1


def bytearray_slice():
    ba = bytearray(b"hello")
    ba[1:3] += b"XY"
    print(ba)  # bytearray(b'helXYlo')

    ba = bytearray(b"hello")
    ba[1:3] *= 2
    print(ba)  # bytearray(b'helello')


def errors():
    # Tuple slice augmented assignment is a TypeError (tuples are immutable).
    t = (1, 2, 3)
    try:
        t[0:2] += (9,)
    except TypeError as e:
        print(type(e).__name__, e)
    # TypeError 'tuple' object does not support item assignment

    # RHS must be iterable for list +=.
    l = [1, 2, 3]
    try:
        l[0:1] += 5
    except TypeError as e:
        print(type(e).__name__, e)
    # TypeError 'int' object is not iterable


def unrelated_paths_intact():
    # Slice target inside a tuple-unpack assignment (plain, not augmented).
    l = [1, 2, 3, 4, 5]
    m = [0, 0]
    l[1:3], m[0] = [9], 7
    print(l, m)  # [1, 9, 4, 5] [7, 0]

    # Pre-existing subscript and plain-slice paths remain intact.
    l = [1, 2, 3]
    l[0] += 10
    print(l)  # [11, 2, 3]
    l[1:3] = [9]
    print(l)  # [11, 9]


basic_add()
basic_mul()
bounds()
iterable_rhs()
extended_slice()
mul_zero()
single_eval()
bytearray_slice()
errors()
unrelated_paths_intact()
