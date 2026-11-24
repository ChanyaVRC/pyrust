# Issue #2980: generator / coroutine / async_generator introspection residuals:
#   1. str() / format() / f-string qualname & address (plus format-spec errors)
#   2. gi_code / cr_code / ag_code across all states + identity with f.__code__
#   3. hash(gen) / hash(coro) / hash(agen) + dict/set keys & equality
#   4. gi_suspended / cr_suspended / ag_suspended / cr_origin + dir() + readonly
#   5. map/filter/zip/enumerate/iterator AttributeError type nouns
#   6. Running gi_frame / cr_frame / ag_frame live frame identity

import re
import sys
import types


def mask_addr(text):
    return re.sub(r"0x[0-9a-fA-F]+", "0xADDR", text)


# ── 1. str() / format() / f-string of generators, coroutines, async generators ─


def gen_fn(a, b=2):
    x = a + b
    yield x
    yield x + 1


class Holder:
    def method_gen(self):
        yield 1


async def coro_fn(a):
    return a + 1


async def agen_fn(a):
    yield a


g = gen_fn(1)
gm = Holder().method_gen()
ge = (i for i in [1])
c = coro_fn(1)
ag = agen_fn(1)

for label, obj, expected_type in (
    ("gen", g, "generator"),
    ("method_gen", gm, "generator"),
    ("genexpr", ge, "generator"),
    ("coro", c, "coroutine"),
    ("agen", ag, "async_generator"),
):
    r = mask_addr(repr(obj))
    s = mask_addr(str(obj))
    f1 = mask_addr(format(obj))
    f2 = mask_addr(f"{obj}")
    f3 = mask_addr("{}".format(obj))
    assert r == s == f1 == f2 == f3, (label, r, s, f1, f2, f3)
    print(f"fmt {label}:", s)
    try:
        format(obj, "s")
        assert False, f"format({label}, 's') must raise TypeError"
    except TypeError as exc:
        assert (
            str(exc) == f"unsupported format string passed to {expected_type}.__format__"
        ), (label, str(exc))
    try:
        f"{obj:s}"
        assert False, f"f'{{{label}:s}}' must raise TypeError"
    except TypeError as exc:
        assert (
            str(exc) == f"unsupported format string passed to {expected_type}.__format__"
        ), (label, str(exc))

# Mutating __qualname__ updates repr(), str(), format(), and f-strings.
g.__qualname__ = "custom_gen"
assert mask_addr(repr(g)) == "<generator object custom_gen at 0xADDR>"
assert mask_addr(str(g)) == "<generator object custom_gen at 0xADDR>"
assert mask_addr(format(g)) == "<generator object custom_gen at 0xADDR>"
assert mask_addr(f"{g}") == "<generator object custom_gen at 0xADDR>"

c.__qualname__ = "custom_coro"
assert mask_addr(str(c)) == "<coroutine object custom_coro at 0xADDR>"
assert mask_addr(format(c)) == "<coroutine object custom_coro at 0xADDR>"

ag.__qualname__ = "custom_agen"
assert mask_addr(str(ag)) == "<async_generator object custom_agen at 0xADDR>"
assert mask_addr(format(ag)) == "<async_generator object custom_agen at 0xADDR>"
c.close()


# ── 2. gi_code / cr_code / ag_code across all states ──────────────────────────

g2 = gen_fn(10)
assert type(g2.gi_code) is types.CodeType
assert g2.gi_code is gen_fn.__code__
assert g2.gi_code.co_name == "gen_fn"
assert g2.gi_code.co_qualname == "gen_fn"
assert g2.gi_code.co_argcount == 2
assert g2.gi_code.co_varnames == ("a", "b", "x")
assert g2.gi_frame.f_code is g2.gi_code
next(g2)
assert g2.gi_code is gen_fn.__code__
assert g2.gi_frame.f_code is g2.gi_code
g2.close()
assert g2.gi_code is gen_fn.__code__

# Exhausted generator keeps gi_code.
g_ex = gen_fn(1)
list(g_ex)
assert g_ex.gi_code is gen_fn.__code__

# Coroutine cr_code across unstarted / closed.
c2 = coro_fn(5)
assert type(c2.cr_code) is types.CodeType
assert c2.cr_code is coro_fn.__code__
assert c2.cr_code.co_name == "coro_fn"
assert c2.cr_code.co_argcount == 1
assert c2.cr_frame.f_code is c2.cr_code
c2.close()
assert c2.cr_code is coro_fn.__code__

# Async generator ag_code across unstarted / paused / closed.
ag2 = agen_fn(7)
assert type(ag2.ag_code) is types.CodeType
assert ag2.ag_code is agen_fn.__code__
assert ag2.ag_code.co_name == "agen_fn"
assert ag2.ag_code.co_argcount == 1
assert ag2.ag_frame.f_code is ag2.ag_code


# ── 3. hash(gen) / hash(coro) / hash(agen) + dict/set keys ────────────────────

ga, gb = gen_fn(1), gen_fn(1)
ca, cb = coro_fn(1), coro_fn(1)
aga, agb = agen_fn(1), agen_fn(1)

for label, a, b in (("gen", ga, gb), ("coro", ca, cb), ("agen", aga, agb)):
    ha = hash(a)
    assert isinstance(ha, int), (label, type(ha))
    assert hash(a) == ha
    assert a == a
    assert a != b
    d = {a: "first", b: "second"}
    assert d[a] == "first" and d[b] == "second"
    assert len({a, a, b}) == 2
    assert hash((a, 1)) == hash((a, 1))
    assert {(a, 1): 99}[(a, 1)] == 99

ca.close()
cb.close()


# ── 4. gi_suspended / cr_suspended / ag_suspended / cr_origin + dir + readonly ─

g_dir = dir(gen_fn(1))
c_tmp = coro_fn(1)
c_dir = dir(c_tmp)
c_tmp.close()
ag_dir = dir(agen_fn(1))

assert "gi_suspended" in g_dir
assert "cr_suspended" in c_dir
assert "cr_origin" in c_dir
assert "ag_suspended" in ag_dir

# Read-only assignment & deletion errors match CPython 3.12.3 exactly.
for factory, attrs in (
    (
        lambda: gen_fn(1),
        [
            ("gi_code", "attribute 'gi_code' of 'generator' objects is not writable"),
            ("gi_frame", "attribute 'gi_frame' of 'generator' objects is not writable"),
            ("gi_running", "attribute 'gi_running' of 'generator' objects is not writable"),
            (
                "gi_suspended",
                "attribute 'gi_suspended' of 'generator' objects is not writable",
            ),
            (
                "gi_yieldfrom",
                "attribute 'gi_yieldfrom' of 'generator' objects is not writable",
            ),
            ("nope", "'generator' object has no attribute 'nope'"),
        ],
    ),
    (
        lambda: coro_fn(1),
        [
            ("cr_code", "attribute 'cr_code' of 'coroutine' objects is not writable"),
            ("cr_frame", "attribute 'cr_frame' of 'coroutine' objects is not writable"),
            ("cr_running", "attribute 'cr_running' of 'coroutine' objects is not writable"),
            (
                "cr_suspended",
                "attribute 'cr_suspended' of 'coroutine' objects is not writable",
            ),
            ("cr_await", "attribute 'cr_await' of 'coroutine' objects is not writable"),
            ("cr_origin", "readonly attribute"),
            ("nope", "'coroutine' object has no attribute 'nope'"),
        ],
    ),
    (
        lambda: agen_fn(1),
        [
            (
                "ag_code",
                "attribute 'ag_code' of 'async_generator' objects is not writable",
            ),
            (
                "ag_frame",
                "attribute 'ag_frame' of 'async_generator' objects is not writable",
            ),
            ("ag_running", "readonly attribute"),
            (
                "ag_suspended",
                "attribute 'ag_suspended' of 'async_generator' objects is not writable",
            ),
            (
                "ag_await",
                "attribute 'ag_await' of 'async_generator' objects is not writable",
            ),
            ("nope", "'async_generator' object has no attribute 'nope'"),
        ],
    ),
):
    for attr, expected_msg in attrs:
        obj = factory()
        try:
            setattr(obj, attr, None)
            assert False, f"setattr({attr}) should have raised AttributeError"
        except AttributeError as exc:
            assert str(exc) == expected_msg, (attr, "set", str(exc), expected_msg)
        try:
            delattr(obj, attr)
            assert False, f"delattr({attr}) should have raised AttributeError"
        except AttributeError as exc:
            assert str(exc) == expected_msg, (attr, "del", str(exc), expected_msg)
        if hasattr(obj, "close"):
            obj.close()


# ── 5. map / filter / zip / enumerate / native iterator AttributeError nouns ──

for obj, expected_noun in (
    (map(int, [1]), "map"),
    (filter(None, [1]), "filter"),
    (zip([1]), "zip"),
    (enumerate([1]), "enumerate"),
    (reversed([1]), "list_reverseiterator"),
    (iter([1]), "list_iterator"),
    (iter((1,)), "tuple_iterator"),
    (iter("a"), "str_ascii_iterator"),
    (iter({1: 2}), "dict_keyiterator"),
    (iter({1}), "set_iterator"),
    (iter(b"a"), "bytes_iterator"),
    (iter(bytearray(b"a")), "bytearray_iterator"),
    (iter(range(1)), "range_iterator"),
):
    for op in (
        lambda o: getattr(o, "foo"),
        lambda o: setattr(o, "foo", 1),
        lambda o: delattr(o, "foo"),
    ):
        try:
            op(obj)
            assert False, f"missing attr on {expected_noun} must raise AttributeError"
        except AttributeError as exc:
            assert str(exc) == f"'{expected_noun}' object has no attribute 'foo'", (
                expected_noun,
                str(exc),
            )
    assert not hasattr(obj, "send")
    assert not hasattr(obj, "throw")
    assert not hasattr(obj, "close")


# ── 6. Running gi_frame / cr_frame / ag_frame + suspended flags across states ─

# Generator: unstarted -> running -> paused -> running2 -> exhausted, plus closed.
gen_states = []


def inspect_gen():
    f_now = sys._getframe()
    gen_states.append(
        (
            "running1",
            g_live.gi_running,
            g_live.gi_suspended,
            g_live.gi_frame is f_now,
            g_live.gi_frame.f_code is inspect_gen.__code__,
            g_live.gi_code is inspect_gen.__code__,
        )
    )
    yield 1
    f_now2 = sys._getframe()
    gen_states.append(
        (
            "running2",
            g_live.gi_running,
            g_live.gi_suspended,
            g_live.gi_frame is f_now2,
            g_live.gi_frame is f_now,
        )
    )


g_live = inspect_gen()
pre_frame = g_live.gi_frame
gen_states.append(("unstarted", g_live.gi_running, g_live.gi_suspended, pre_frame is not None))
next(g_live)
paused_frame = g_live.gi_frame
gen_states.append(
    (
        "paused",
        g_live.gi_running,
        g_live.gi_suspended,
        paused_frame is pre_frame,
    )
)
try:
    next(g_live)
except StopIteration:
    pass
gen_states.append(
    ("exhausted", g_live.gi_running, g_live.gi_suspended, g_live.gi_frame is None)
)

g_closed = gen_fn(1)
next(g_closed)
assert g_closed.gi_suspended is True
g_closed.close()
gen_states.append(
    ("closed", g_closed.gi_running, g_closed.gi_suspended, g_closed.gi_frame is None)
)
print("gen states:", gen_states)


# Gen-drive trampoline (`for x in g:`) also exposes live gi_frame.
tramp_seen = []


def tramp_gen():
    f = sys._getframe()
    tramp_seen.append(
        (
            g_tramp.gi_running,
            g_tramp.gi_suspended,
            g_tramp.gi_frame is f,
            g_tramp.gi_frame.f_code.co_name,
        )
    )
    yield 42


g_tramp = tramp_gen()
for val in g_tramp:
    assert val == 42
    assert g_tramp.gi_running is False
    assert g_tramp.gi_suspended is True
    assert g_tramp.gi_frame is not None
assert g_tramp.gi_suspended is False
assert g_tramp.gi_frame is None
print("trampoline:", tramp_seen)


# Coroutine: unstarted -> running -> suspended at await -> running2 -> finished.
class PauseOnce:
    def __await__(self):
        yield "paused"
        return 7


c_live = None


async def inspect_coro():
    f1 = sys._getframe()
    s1 = (
        c_live.cr_running,
        c_live.cr_suspended,
        c_live.cr_origin,
        c_live.cr_frame is f1,
        c_live.cr_frame.f_code is inspect_coro.__code__,
        c_live.cr_code is inspect_coro.__code__,
    )
    val = await PauseOnce()
    f2 = sys._getframe()
    s2 = (
        c_live.cr_running,
        c_live.cr_suspended,
        c_live.cr_frame is f2,
        c_live.cr_frame is f1,
        val,
    )
    return (s1, s2)


c_live = inspect_coro()
coro_pre = c_live.cr_frame
assert c_live.cr_running is False
assert c_live.cr_suspended is False
assert c_live.cr_origin is None
assert coro_pre is not None
assert c_live.send(None) == "paused"
assert c_live.cr_running is False
assert c_live.cr_suspended is True
assert c_live.cr_frame is coro_pre
try:
    c_live.send(None)
    assert False, "coro should have finished with StopIteration"
except StopIteration as si:
    print("coro inside:", si.value)
assert c_live.cr_running is False
assert c_live.cr_suspended is False
assert c_live.cr_frame is None
assert c_live.cr_code is inspect_coro.__code__


# Async generator: unstarted -> running -> paused -> closed.
ag_live = None


async def inspect_agen():
    f = sys._getframe()
    yield (
        ag_live.ag_running,
        ag_live.ag_suspended,
        ag_live.ag_frame is f,
        ag_live.ag_frame.f_code is inspect_agen.__code__,
        ag_live.ag_code is inspect_agen.__code__,
    )


ag_live = inspect_agen()
ag_pre = ag_live.ag_frame
assert ag_live.ag_running is False
assert ag_live.ag_suspended is False
assert ag_pre is not None
an = ag_live.__anext__()
try:
    an.send(None)
    assert False
except StopIteration as si:
    print("agen inside:", si.value)
assert ag_live.ag_running is False
assert ag_live.ag_suspended is True
assert ag_live.ag_frame is ag_pre
ac = ag_live.aclose()
try:
    ac.send(None)
except StopIteration:
    pass
assert ag_live.ag_running is False
assert ag_live.ag_suspended is False
assert ag_live.ag_frame is None
assert ag_live.ag_code is inspect_agen.__code__

print("generator introspection 2980 ok")
