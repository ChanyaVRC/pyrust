# Parity fixture for issue #3409.
#
# `del x[-3:0]` / `x[4:1] = [99]` on a list normalise to `start > stop`; the
# contiguous (step == 1) path used to build a reversed `drain`/`splice` range
# and panic.  CPython treats `start > stop` as an empty slice anchored at
# `start`: deleting is a no-op and assigning inserts at `start`.
#
# This fixture pins the reproducer, then sweeps list / list subclass /
# bytearray / bytearray subclass / array.array over start, stop and step
# values (negative, past-the-end, bool, huge int) for get, set and del so no
# combination may panic and every result (including the ValueError wording for
# an extended-slice size mismatch) matches CPython.
import array


class L(list):
    pass


class B(bytearray):
    pass


# --- Reproducer from the issue -------------------------------------------
x = list(range(10))
del x[-3:0]
print(x)

y = list(range(5))
y[4:1] = [99]
print(y)

b = bytearray(b"abcde")
del b[-2:0]
b[4:1] = b"XY"
print(bytes(b))

# --- Hand-picked start > stop cases ----------------------------------------
z = list(range(6))
del z[5:2]
print(z)
z[5:2] = []
print(z)
z[4:1] = [7, 8, 9]
print(z)
z[100:3] = ["tail"]
print(z)
z[-1:-4] = ["neg"]
print(z)
z[3:-100] = ["front"]
print(z)

ls = L(range(5))
del ls[3:1]
ls[3:1] = [42]
print(ls, type(ls).__name__)

ba = bytearray(b"hello")
ba[3:1] = b"-"
print(bytes(ba))
del ba[4:0]
print(bytes(ba))

bs = B(b"hello")
bs[3:1] = b"-"
del bs[4:0]
print(bytes(bs), type(bs).__name__)

arr = array.array("b", range(6))
del arr[4:1]
arr[4:1] = array.array("b", [90])
print(arr.tolist())

# Extended slice with an empty selection (negative step and start < stop).
w = list(range(5))
w[1:4:-1] = []
print(w)
try:
    w[1:4:-1] = [1]
except ValueError as e:
    print("ValueError:", e)
del w[1:4:-1]
print(w)

# A bytearray extended slice assigned an empty value deletes the selection.
e = bytearray(b"abcdef")
e[::2] = b""
print(bytes(e))
e = bytearray(b"abcdef")
e[::-2] = b""
print(bytes(e))

# --- Exhaustive sweep --------------------------------------------------------
BOUNDS = [None, -7, -1, 0, 2, 4, True, 10**30, -(10**30)]
STEPS = [None, 2, -1, -2, 10**30, -(10**30)]


def make(kind, n):
    if kind == "list":
        return list(range(n))
    if kind == "list-subclass":
        return L(range(n))
    if kind == "bytearray":
        return bytearray(range(65, 65 + n))
    if kind == "bytearray-subclass":
        return B(range(65, 65 + n))
    return array.array("b", range(n))


def rhs(kind, k):
    if kind in ("list", "list-subclass"):
        return [100 + i for i in range(k)]
    if kind in ("bytearray", "bytearray-subclass"):
        return bytes(range(120, 120 + k))
    return array.array("b", range(50, 50 + k))


def show(v):
    if isinstance(v, (bytes, bytearray)):
        return repr(bytes(v))
    return repr(list(v))


def outcome(fn):
    try:
        return fn()
    except Exception as exc:
        return type(exc).__name__ + ": " + str(exc)


def do_get(kind, n, sl):
    return show(make(kind, n)[sl])


def do_del(kind, n, sl):
    v = make(kind, n)
    del v[sl]
    return show(v)


def do_set(kind, n, sl, k):
    v = make(kind, n)
    v[sl] = rhs(kind, k)
    return show(v)


digest = {}
for kind in ["list", "list-subclass", "bytearray", "bytearray-subclass", "array"]:
    lines = []
    for n in (0, 1, 3, 5):
        for a in BOUNDS:
            for c in BOUNDS:
                for s in STEPS:
                    sl = slice(a, c, s)
                    tag = "%d %r:%r:%r" % (n, a, c, s)
                    lines.append(tag + " get " + outcome(lambda: do_get(kind, n, sl)))
                    lines.append(tag + " del " + outcome(lambda: do_del(kind, n, sl)))
                    for k in (0, 1, 3):
                        lines.append(
                            tag + " set%d " % k + outcome(lambda: do_set(kind, n, sl, k))
                        )
    # Print a stable checksum plus the line count so any divergence is visible
    # without dumping thousands of lines per kind.
    total = 0
    for line in lines:
        for ch in line:
            total = (total * 131 + ord(ch)) % 1000000007
    digest[kind] = (len(lines), total)
    print(kind, len(lines), total)

# Spot-check lines are also printed verbatim so a mismatch is easy to read.
for kind in ["list", "bytearray", "array"]:
    for line_args in [
        (6, 4, 1, None, "set1"),
        (6, -3, 0, None, "del"),
        (5, 1, 4, -1, "set1"),
        (5, 4, 1, 2, "set1"),
        (5, 10**30, -(10**30), -(10**30), "del"),
    ]:
        n, a, c, s, op = line_args
        sl = slice(a, c, s)
        if op == "del":
            print(kind, line_args, outcome(lambda: do_del(kind, n, sl)))
        else:
            print(kind, line_args, outcome(lambda: do_set(kind, n, sl, 1)))
