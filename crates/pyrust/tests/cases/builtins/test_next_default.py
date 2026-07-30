# next(it, default) must return the default for EVERY iterator kind whose
# advance raises StopIteration, and must let every other exception through.
# Issue #2966: built-in-module iterator classes (itertools.*, io.*) raise a
# StopIteration that the old per-arm handling did not recognise, so the
# default was ignored.
import collections
import io
import itertools


def show(label, fn):
    try:
        print(label, "->", repr(fn()))
    except BaseException as e:
        print(label, "!!", type(e).__name__, repr(e.args))


print("== built-in module iterator classes ==")
show("combinations", lambda: next(itertools.combinations([1], 2), "D"))
show("combinations_wr", lambda: next(itertools.combinations_with_replacement([], 1), "D"))
show("permutations", lambda: next(itertools.permutations([1], 2), "D"))
show("product", lambda: next(itertools.product([], [1]), "D"))
show("repeat", lambda: next(itertools.repeat(7, 0), "D"))
show("chain", lambda: next(itertools.chain([], []), "D"))
show("islice", lambda: next(itertools.islice([1, 2, 3], 0), "D"))
show("count", lambda: next(itertools.count(0), "D"))
show("zip_longest", lambda: next(itertools.zip_longest(), "D"))
show("accumulate", lambda: next(itertools.accumulate([]), "D"))
show("compress", lambda: next(itertools.compress([], []), "D"))
show("dropwhile", lambda: next(itertools.dropwhile(lambda x: True, [1]), "D"))
show("takewhile", lambda: next(itertools.takewhile(lambda x: False, [1]), "D"))
show("filterfalse", lambda: next(itertools.filterfalse(bool, [1]), "D"))
show("starmap", lambda: next(itertools.starmap(max, []), "D"))
show("pairwise", lambda: next(itertools.pairwise([1]), "D"))
show("groupby", lambda: next(itertools.groupby([]), "D"))
show("StringIO", lambda: next(io.StringIO(""), "D"))
show("BytesIO", lambda: next(io.BytesIO(b""), "D"))

_s = io.StringIO("a\n")
next(_s)
show("StringIO_exhausted", lambda: next(_s, "D"))

print("== native cursors, empty ==")
show("listiter", lambda: next(iter([]), "D"))
show("striter", lambda: next(iter(""), "D"))
show("tupleiter", lambda: next(iter(()), "D"))
show("dictiter", lambda: next(iter({}), "D"))
show("setiter", lambda: next(iter(set()), "D"))
show("frozensetiter", lambda: next(iter(frozenset()), "D"))
show("rangeiter", lambda: next(iter(range(0)), "D"))
show("bigrangeiter", lambda: next(iter(range(2**70, 2**70)), "D"))
show("bytesiter", lambda: next(iter(b""), "D"))
show("bytearrayiter", lambda: next(iter(bytearray()), "D"))
show("enumerate", lambda: next(enumerate([]), "D"))
show("zip", lambda: next(zip([], []), "D"))
show("map", lambda: next(map(str, []), "D"))
show("filter", lambda: next(filter(None, []), "D"))
show("reversed", lambda: next(reversed([]), "D"))
show("dict_keys", lambda: next(iter({}.keys()), "D"))
show("dict_items", lambda: next(iter({}.items()), "D"))
show("dict_values", lambda: next(iter({}.values()), "D"))

print("== native cursors, exhausted then re-polled ==")
_l = iter([1])
next(_l)
show("listiter_exhausted", lambda: next(_l, "D"))
show("listiter_exhausted_again", lambda: next(_l, "D"))

print("== mutation-latched cursors: the RuntimeError must NOT become the default ==")
_grow = [1, 2, 3]
_it_grow = iter(_grow)
next(_it_grow)
_grow.append(4)
show("list_appended", lambda: next(_it_grow, "D"))

_d = {1: 1}
_it_d = iter(_d)
_d[2] = 2
show("dict_resized", lambda: next(_it_d, "D"))

_st = {1}
_it_st = iter(_st)
_st.add(2)
show("set_resized", lambda: next(_it_st, "D"))

print("== generators ==")


def gen_empty():
    if False:
        yield


show("gen_empty", lambda: next(gen_empty(), "D"))


def gen_return_value():
    return 42
    yield


_gr = gen_return_value()
show("gen_return_value", lambda: next(_gr, "D"))
show("gen_return_value_again", lambda: next(_gr, "D"))

try:
    next(gen_return_value())
except StopIteration as e:
    # No default: the original StopIteration survives, .value intact (PEP 380).
    print("gen_return_value_no_default !!", repr(e.value), repr(e.args))


def gen_raises():
    yield 1
    raise ValueError("boom")


_gv = gen_raises()
next(_gv)
show("gen_raises_ValueError", lambda: next(_gv, "D"))


def gen_body_raises_stopiteration():
    raise StopIteration("inner")
    yield


# PEP 479: a StopIteration escaping the body becomes RuntimeError, so the
# default must not apply.
show("gen_body_raises_SI", lambda: next(gen_body_raises_stopiteration(), "D"))

show("genexpr", lambda: next((x for x in []), "D"))


def gen_one():
    yield 1


_g1 = gen_one()
next(_g1)
try:
    next(_g1)
except StopIteration as e:
    print("gen_exhausted_no_default !!", repr(e.value), repr(e.args))
try:
    next(iter([]))
except StopIteration as e:
    print("listiter_no_default !!", repr(e.value), repr(e.args))
try:
    next(itertools.repeat(1, 0))
except StopIteration as e:
    print("repeat_no_default !!", repr(e.value), repr(e.args))

# Re-polling a generator that has ALREADY finished takes a different arm from
# the poll that finishes it: the frame is flagged done, so the body never
# resumes.  Pin both the default and the no-default shape here — `.value` is
# None and `.args` is empty either way, and the exception must stay a real
# StopIteration for `raise ... from` / `__context__` to chain correctly.
_gd = gen_one()
next(_gd)
try:
    next(_gd)
except StopIteration:
    pass
show("gen_done_repoll_default", lambda: next(_gd, "D"))
show("gen_done_repoll_default_again", lambda: next(_gd, "D"))
try:
    next(_gd)
except StopIteration as e:
    print("gen_done_repoll_no_default !!", repr(e.value), repr(e.args), repr(str(e)))
    print("  is_StopIteration", type(e) is StopIteration, isinstance(e, Exception))
    try:
        raise ValueError("after") from e
    except ValueError as v:
        print("  chained_cause", type(v.__cause__).__name__, v.__cause__ is e)

# Same arm reached after a return-value stop: the discarded StopIteration must
# not leak its value into a later poll.
_gdv = gen_return_value()
try:
    next(_gdv)
except StopIteration:
    pass
show("gen_done_after_return_default", lambda: next(_gdv, "D"))
try:
    next(_gdv)
except StopIteration as e:
    print("gen_done_after_return_no_default !!", repr(e.value), repr(e.args))

print("== user __next__ ==")


class StopSubclass(StopIteration):
    pass


class RaisesSubclass:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopSubclass("sub")


show("user_stopiteration_subclass", lambda: next(RaisesSubclass(), "D"))
show("user_stopiteration_subclass_no_default", lambda: next(RaisesSubclass()))


class RaisesValueError:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError("nope")


show("user_value_error", lambda: next(RaisesValueError(), "D"))


class RaisesBare:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration


show("user_bare_stopiteration", lambda: next(RaisesBare(), "D"))

print("== __getitem__ and callable-sentinel iterators ==")


class GetItemStops:
    def __getitem__(self, i):
        raise IndexError


show("getitem_iter", lambda: next(iter(GetItemStops()), "D"))


class GetItemRaises:
    def __getitem__(self, i):
        raise ValueError("gi")


show("getitem_iter_raises", lambda: next(iter(GetItemRaises()), "D"))
show("callable_sentinel", lambda: next(iter(lambda: 0, 0), "D"))


def callable_boom():
    raise ValueError("cb")


show("callable_raises", lambda: next(iter(callable_boom, 0), "D"))

print("== non-iterators: TypeError wins over the default ==")


class NotAnIterator:
    pass


show("plain_object", lambda: next(NotAnIterator(), "D"))
show("list", lambda: next([1, 2], "D"))
show("int", lambda: next(5, "D"))
show("str", lambda: next("ab", "D"))

print("== arity and falsy defaults ==")
show("three_args", lambda: next(iter([]), "D", "E"))
show("zero_args", lambda: next())
show("keyword", lambda: next(iter([]), default="D"))
show("default_none", lambda: next(iter([]), None))
show("default_false", lambda: next(iter([]), False))
show("default_zero", lambda: next(iter([]), 0))

print("== single-argument next() still drives built-in iterators ==")
show("chain_drive", lambda: list(itertools.chain([1, 2], [3])))
show("islice_drive", lambda: list(itertools.islice(itertools.count(), 4)))
show("groupby_drive", lambda: [(k, list(v)) for k, v in itertools.groupby("aab")])
show("takewhile_drive", lambda: list(itertools.takewhile(lambda x: x < 3, range(9))))
show("deque_of_repeat", lambda: collections.deque(itertools.repeat(5, 3)))
show("counter_of_repeat", lambda: collections.Counter(itertools.repeat("x", 2)))
show("sum_of_islice", lambda: sum(itertools.islice(itertools.count(1), 4)))
show("dict_of_repeat", lambda: dict(itertools.repeat(("k", 1), 1)))
show("set_of_repeat", lambda: set(itertools.repeat(3, 2)))
show("list_of_user_subclass", lambda: list(RaisesSubclass()))

print("== send(None) is next() without a default ==")


def gen_send():
    yield 1


_gs = gen_send()
next(_gs)
show("send_after_exhaustion", lambda: _gs.send(None))
