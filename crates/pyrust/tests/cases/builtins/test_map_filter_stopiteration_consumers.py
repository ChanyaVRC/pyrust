import collections
import functools
import itertools


class MyStop(StopIteration):
    pass


def f_stop(x):
    if x == 2:
        raise StopIteration("stop!")
    return x * 10


def f_substop(x):
    if x == 2:
        raise MyStop("sub!")
    return x * 10


def f_pair(x):
    if x == 2:
        raise StopIteration("stop!")
    return (x, x * 10)


def f_str(x):
    if x == 2:
        raise StopIteration("stop!")
    return str(x)


def f_add_stop(x, y):
    if x == 2:
        raise StopIteration("pair_stop")
    return x + y


def f_bool_stop(x):
    if x == 2:
        raise StopIteration("filt_stop")
    return x % 2 == 1


def f_boom(x):
    if x == 2:
        raise ValueError("boom")
    return x * 10


class BoolStop:
    def __init__(self, v):
        self.v = v

    def __bool__(self):
        if self.v == 2:
            raise StopIteration("bool_stop")
        return self.v % 2 == 1

    def __repr__(self):
        return f"BoolStop({self.v})"


xs = [1, 2, 3, 4]

# --- map across all native consumers ---
print("list(map):", list(map(f_stop, xs)))
print("list(map sub):", list(map(f_substop, xs)))
print("list(map multi):", list(map(f_add_stop, xs, [10, 20, 30, 40])))
print("tuple(map):", tuple(map(f_stop, xs)))
print("set(map):", sorted(set(map(f_stop, xs))))
print("frozenset(map):", sorted(frozenset(map(f_stop, xs))))
print("dict(map):", dict(map(f_pair, xs)))
print("sum(map):", sum(map(f_stop, xs)))
print("min(map):", min(map(f_stop, xs)))
print("max(map):", max(map(f_stop, xs)))
print("any(map):", any(map(f_stop, [0, 2, 1])))
print("all(map):", all(map(f_stop, [1, 2, 0])))
print("sorted(map):", sorted(map(f_stop, [3, 1, 2, 0])))
print("enumerate(map):", list(enumerate(map(f_stop, xs))))
print("zip(map):", list(zip(map(f_stop, xs), [100, 200, 300])))

try:
    list(zip(map(f_stop, xs), [100, 200, 300], strict=True))
except ValueError as exc:
    print("zip strict(map):", exc.args)

loop_out = []
for v in map(f_stop, xs):
    loop_out.append(v)
print("for(map):", loop_out)
print("listcomp(map):", [v for v in map(f_stop, xs)])
print("setcomp(map):", sorted({v for v in map(f_stop, xs)}))
print("dictcomp(map):", {k: v for k, v in map(f_pair, xs)})
print("genexpr(map):", list(v for v in map(f_stop, xs)))
print("unpack list(map):", [*map(f_stop, xs)])
print("unpack tuple(map):", (*map(f_stop, xs),))
a, *rest = map(f_stop, xs)
print("unpack assign(map):", a, rest)
print("str.join(map):", ",".join(map(f_str, xs)))
print("bytes(map):", bytes(map(f_stop, xs)))
print("bytearray(map):", list(bytearray(map(f_stop, xs))))
print("deque(map):", list(collections.deque(map(f_stop, xs))))

ext = [0]
ext.extend(map(f_stop, xs))
print("list.extend(map):", ext)

su = {0}
su.update(map(f_stop, xs))
print("set.update(map):", sorted(su))

du = {0: 0}
du.update(map(f_pair, xs))
print("dict.update(map):", du)

print("reduce(map):", functools.reduce(lambda acc, v: acc + v, map(f_stop, xs), 0))
print("islice(map):", list(itertools.islice(map(f_stop, xs), 5)))
print("chain(map):", list(itertools.chain(map(f_stop, xs), [99])))
print("nested map(map):", list(map(lambda y: y + 1, map(f_stop, xs))))
print("nested filter(map):", list(filter(lambda y: y > 0, map(f_stop, xs))))

# --- filter across all native consumers ---
print("list(filter):", list(filter(f_bool_stop, xs)))
print("list(filter sub):", list(filter(f_substop, xs)))
print("list(filter None BoolStop):", list(filter(None, [BoolStop(1), BoolStop(2), BoolStop(3)])))
print("tuple(filter):", tuple(filter(f_bool_stop, xs)))
print("set(filter):", sorted(set(filter(f_bool_stop, xs))))
print("frozenset(filter):", sorted(frozenset(filter(f_bool_stop, xs))))
print("sum(filter):", sum(filter(f_bool_stop, xs)))
print("min(filter):", min(filter(f_bool_stop, xs)))
print("max(filter):", max(filter(f_bool_stop, xs)))
print("any(filter):", any(filter(f_bool_stop, [0, 2, 1])))
print("all(filter):", all(filter(f_bool_stop, [1, 2, 0])))
print("sorted(filter):", sorted(filter(f_bool_stop, [3, 1, 2, 0])))
print("enumerate(filter):", list(enumerate(filter(f_bool_stop, xs))))
print("zip(filter):", list(zip(filter(f_bool_stop, xs), [100, 200])))
print("for(filter):", [v for v in filter(f_bool_stop, xs)])
print("unpack(filter):", [*filter(f_bool_stop, xs)])
print("bytes(filter):", bytes(filter(f_bool_stop, xs)))
print("deque(filter):", list(collections.deque(filter(f_bool_stop, xs))))
print("islice(filter):", list(itertools.islice(filter(f_bool_stop, xs), 5)))
print("chain(filter):", list(itertools.chain(filter(f_bool_stop, xs), [99])))
print("nested map(filter):", list(map(lambda y: y * 2, filter(f_bool_stop, xs))))

# --- itertools callables raising StopIteration inside collect ---
print("starmap:", list(itertools.starmap(f_add_stop, [(1, 10), (2, 20), (3, 30)])))
print("filterfalse:", list(itertools.filterfalse(f_bool_stop, xs)))
print("takewhile:", list(itertools.takewhile(f_bool_stop, xs)))
print("dropwhile:", list(itertools.dropwhile(f_bool_stop, [1, 2, 3])))

# --- map/filter NOT latched done on callable StopIteration ---
m = map(f_stop, [1, 2, 3, 4])
print("m next 1:", next(m))
try:
    next(m)
except StopIteration as exc:
    print("m next 2 exc:", type(exc).__name__, exc.args)
print("m next 3:", next(m))
print("m next 4:", next(m))
print("m next 5 default:", next(m, "END"))

m2 = map(f_stop, [1, 2, 3, 4])
print("m2 list 1:", list(m2))
print("m2 list 2:", list(m2))

m3 = map(f_stop, [1, 2, 3, 4])
print("m3 next(default):", next(m3, "D"), next(m3, "D"), next(m3, "D"), next(m3, "D"), next(m3, "D"))

flt = filter(f_bool_stop, [1, 2, 3, 4])
print("flt list 1:", list(flt))
print("flt list 2:", list(flt))

# --- non-StopIteration errors still propagate ---
for label, fn in [
    ("list(map boom)", lambda: list(map(f_boom, xs))),
    ("list(filter boom)", lambda: list(filter(f_boom, xs))),
    ("sum(map boom)", lambda: sum(map(f_boom, xs))),
    ("next(map boom, def)", lambda: next(map(f_boom, [2]), "DEF")),
]:
    try:
        fn()
    except ValueError as exc:
        print(f"{label}: ValueError {exc.args}")
