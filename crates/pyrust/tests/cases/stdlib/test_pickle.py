# CPython 3.12 parity tests for `pickle` (issue #3188).

import collections
import functools
import io
import math
import os
import pickle
import sys
import tempfile
from pickle import (
    DEFAULT_PROTOCOL,
    HIGHEST_PROTOCOL,
    PickleBuffer,
    PickleError,
    Pickler,
    PicklingError,
    Unpickler,
    UnpicklingError,
    dump,
    dumps,
    load,
    loads,
)

ALL_PROTOCOLS = (0, 1, 2, 3, 4, 5)


# ---------------------------------------------------------------------------
# 1. Module metadata, constants, exceptions
# ---------------------------------------------------------------------------

print("mod_id:", pickle is sys.modules["pickle"])
print("has_all:", hasattr(pickle, "__all__"))
print("protocols:", HIGHEST_PROTOCOL, DEFAULT_PROTOCOL)
print(
    "exc_hierarchy:",
    issubclass(PickleError, Exception),
    issubclass(PicklingError, PickleError),
    issubclass(UnpicklingError, PickleError),
)
print(
    "core_in_all:",
    all(
        n in pickle.__all__
        for n in (
            "PickleError",
            "PicklingError",
            "UnpicklingError",
            "Pickler",
            "Unpickler",
            "dump",
            "dumps",
            "load",
            "loads",
            "PickleBuffer",
            "HIGHEST_PROTOCOL",
            "DEFAULT_PROTOCOL",
        )
    ),
)


# ---------------------------------------------------------------------------
# 2. Wire-format parity: exact byte-for-byte comparison with CPython 3.12
# ---------------------------------------------------------------------------

wire_samples = [
    ("none", None),
    ("true", True),
    ("false", False),
    ("zero", 0),
    ("int_u8", 42),
    ("int_u16", 1000),
    ("int_neg", -123456),
    ("int_i32_min", -(1 << 31)),
    ("int_i32_max", (1 << 31) - 1),
    ("int_big_pos", 10**30),
    ("int_big_neg", -(10**30)),
    ("int_huge", (1 << 2048) + 12345),
    ("float_zero", 0.0),
    ("float_neg_zero", -0.0),
    ("float_pi", 3.141592653589793),
    ("float_inf", float("inf")),
    ("float_ninf", float("-inf")),
    ("complex", 1.5 - 2.25j),
    ("str_empty", ""),
    ("str_ascii", "hello\nworld\r\t\\'\x00\x1a"),
    ("str_unicode", "caf\u00e9 \u65e5\u672c\u8a9e \U0001f680"),
    ("bytes_empty", b""),
    ("bytes_short", b"\x00\x01\x7f\x80\xffhello"),
    ("bytearray_empty", bytearray()),
    ("bytearray_short", bytearray(b"abc\x00\xff")),
    ("tuple_0", ()),
    ("tuple_1", (1,)),
    ("tuple_2", (1, 2)),
    ("tuple_3", (1, 2, 3)),
    ("tuple_4", (1, 2, 3, 4)),
    ("list_empty", []),
    ("list_1", [1]),
    ("list_3", [1, 2, 3]),
    ("dict_empty", {}),
    ("dict_1", {"a": 1}),
    ("dict_2", {"a": 1, "b": 2}),
    ("set_empty", set()),
    ("set_1", {42}),
    ("frozenset_empty", frozenset()),
    ("frozenset_1", frozenset([42])),
    ("slice", slice(1, 10, 2)),
    ("range", range(1, 10, 2)),
    ("ellipsis", Ellipsis),
    ("notimpl", NotImplemented),
]

for label, val in wire_samples:
    for proto in ALL_PROTOCOLS:
        raw = dumps(val, protocol=proto)
        rt = loads(raw)
        ok = (rt is val or rt == val) and (type(rt) is type(val))
        if isinstance(val, float):
            ok = ok and (math.copysign(1.0, rt) == math.copysign(1.0, val))
        if not ok:
            print("WIRE_RT_FAIL:", label, proto, raw, rt)
        print(f"wire:{label}:p{proto}:", raw)

# Check protocol=-1 and protocol=None
print("proto_neg1:", dumps([1, 2], protocol=-1) == dumps([1, 2], protocol=5))
print("proto_none:", dumps([1, 2], protocol=None) == dumps([1, 2], protocol=DEFAULT_PROTOCOL))


# ---------------------------------------------------------------------------
# 3. Primitives, NaN, large values, batching, long strings/bytes
# ---------------------------------------------------------------------------

for proto in ALL_PROTOCOLS:
    # NaN
    nan_rt = loads(dumps(float("nan"), protocol=proto))
    assert math.isnan(nan_rt)

    # Edge integers around LONG1 / LONG4 / BININT boundaries
    edge_ints = [
        -1,
        -128,
        -129,
        -256,
        -32768,
        -32769,
        127,
        128,
        255,
        256,
        32767,
        32768,
        65535,
        65536,
        (1 << 31) - 1,
        1 << 31,
        -(1 << 31),
        -(1 << 31) - 1,
        (1 << 63) - 1,
        -(1 << 63),
        10**300,
        -(10**300),
        (1 << 2500) - 1,
        -(1 << 2500),
    ]
    assert loads(dumps(edge_ints, protocol=proto)) == edge_ints

    # Long strings (> 256 bytes), long bytes (> 256 bytes), long bytearray
    long_str = ("a\u00e9\u65e5\U0001f680" * 100)
    long_bytes = bytes(range(256)) * 2
    long_ba = bytearray(range(256)) * 2
    assert loads(dumps(long_str, protocol=proto)) == long_str
    assert loads(dumps(long_bytes, protocol=proto)) == long_bytes
    rt_ba = loads(dumps(long_ba, protocol=proto))
    assert type(rt_ba) is bytearray and rt_ba == long_ba

    # Large containers (> _BATCHSIZE = 1000)
    big_list = list(range(1050))
    assert loads(dumps(big_list, protocol=proto)) == big_list
    big_dict = {i: i * 2 for i in range(1050)}
    assert loads(dumps(big_dict, protocol=proto)) == big_dict
    big_set = set(range(1050))
    assert loads(dumps(big_set, protocol=proto)) == big_set

    # Multi-element set & frozenset
    s = {1, "two", (3, 4)}
    fs = frozenset([1, "two", (3, 4)])
    assert loads(dumps(s, protocol=proto)) == s
    assert loads(dumps(fs, protocol=proto)) == fs

    # Slices & ranges
    for sl in (slice(None), slice(5), slice(1, 10), slice(1, 10, 2), slice(-10, -1, 3)):
        assert loads(dumps(sl, protocol=proto)) == sl
    for rg in (range(0), range(10), range(1, 20, 3), range(10, -5, -2)):
        assert loads(dumps(rg, protocol=proto)) == rg

    # Special singleton types
    for tp in (type(None), type(Ellipsis), type(NotImplemented)):
        assert loads(dumps(tp, protocol=proto)) is tp

print("primitives_and_containers: OK")


# ---------------------------------------------------------------------------
# 4. Decoding real CPython-generated streams (including legacy Python 2 opcodes)
# ---------------------------------------------------------------------------

# Python 2 STRING opcode with single quotes, double quotes, and escapes
print("py2_string_ascii:", loads(b"S'hello\\nworld\\x21\\101'\np0\n."))
print("py2_string_dquote:", loads(b'S"a\'b\\\\c"\np0\n.'))
print("py2_string_bytes:", loads(b"S'\\x80\\xff'\np0\n.", encoding="bytes"))
print("py2_string_latin1:", loads(b"S'caf\\xe9'\np0\n.", encoding="latin-1"))

# Python 2 BINSTRING / SHORT_BINSTRING
print("py2_short_binstring:", loads(b"U\x05helloq\x00."))
print("py2_binstring:", loads(b"T\x05\x00\x00\x00worldq\x00."))
print("py2_binstring_bytes:", loads(b"U\x02\xfe\xffq\x00.", encoding="bytes"))

# Real CPython 3.12.3 streams across protocols 0, 2, 4, 5
cpython_streams = [
    ("cp312_p0_complex", b"c__builtin__\ncomplex\np0\n(F1.0\nF2.0\ntp1\nRp2\n."),
    ("cp312_p0_bytes", b"c_codecs\nencode\np0\n(V\\u0000\x01\xffhello\np1\nVlatin1\np2\ntp3\nRp4\n."),
    ("cp312_p0_uni", b"Vcaf\xe9 \\u65e5\\u672c\\u8a9e \\U0001f680\np0\n."),
    ("cp312_p2_bytes", b"\x80\x02c_codecs\nencode\nq\x00X\t\x00\x00\x00\x00\x01\xc3\xbfhelloq\x01X\x06\x00\x00\x00latin1q\x02\x86q\x03Rq\x04."),
    ("cp312_p2_range", b"\x80\x02c__builtin__\nxrange\nq\x00K\x01K\nK\x02\x87q\x01Rq\x02."),
    ("cp312_p4_frame_dict", b"\x80\x04\x95\x11\x00\x00\x00\x00\x00\x00\x00}\x94(\x8c\x01a\x94K\x01\x8c\x01b\x94K\x02u."),
    ("cp312_p4_set", b"\x80\x04\x95\t\x00\x00\x00\x00\x00\x00\x00\x8f\x94(K\x01K\x02\x90."),
    ("cp312_p4_frozenset", b"\x80\x04\x95\x08\x00\x00\x00\x00\x00\x00\x00(K\x01K\x02\x91\x94."),
    ("cp312_p5_bytearray", b"\x80\x05\x95\x0f\x00\x00\x00\x00\x00\x00\x00\x96\x04\x00\x00\x00\x00\x00\x00\x00abc\xff\x94."),
]
for label, stream in cpython_streams:
    print(label, repr(loads(stream)))


# ---------------------------------------------------------------------------
# 5. Shared identity & recursive / cyclic graphs
# ---------------------------------------------------------------------------

for proto in ALL_PROTOCOLS:
    # Shared references
    shared = [1, 2, 3]
    container = [shared, shared, {"ref": shared}]
    rt = loads(dumps(container, protocol=proto))
    assert rt == container
    assert rt[0] is rt[1] and rt[0] is rt[2]["ref"]

    # Self-referential list
    rec_list = [1]
    rec_list.append(rec_list)
    rt_list = loads(dumps(rec_list, protocol=proto))
    assert rt_list[0] == 1 and rt_list[1] is rt_list

    # Self-referential dict
    rec_dict = {"k": 10}
    rec_dict["self"] = rec_dict
    rt_dict = loads(dumps(rec_dict, protocol=proto))
    assert rt_dict["k"] == 10 and rt_dict["self"] is rt_dict

    # Mutually recursive list & dict
    a = []
    d = {"a": a}
    a.append(d)
    rt_a = loads(dumps(a, protocol=proto))
    assert rt_a[0]["a"] is rt_a

    # Recursive tuple via list
    holder = []
    rec_tuple = (holder, 99)
    holder.append(rec_tuple)
    rt_t = loads(dumps(rec_tuple, protocol=proto))
    assert rt_t[1] == 99 and rt_t[0][0] is rt_t

print("shared_and_recursive: OK")


# ---------------------------------------------------------------------------
# 6. Globals, custom classes, __slots__, __getstate__/__setstate__,
#    __reduce__/__reduce_ex__, __getnewargs__/__getnewargs_ex__
# ---------------------------------------------------------------------------


def top_level_func(x):
    return x + 1


class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def __eq__(self, other):
        return isinstance(other, Point) and (self.x, self.y) == (other.x, other.y)


class Outer:
    class Inner:
        def __init__(self, v):
            self.v = v


class Slotted:
    __slots__ = ("a", "b")

    def __init__(self, a, b):
        self.a = a
        self.b = b


class SlottedWithDict(Slotted):
    pass


class CustomState:
    def __init__(self, val, transient="drop"):
        self.val = val
        self.transient = transient

    def __getstate__(self):
        return (self.val * 10,)

    def __setstate__(self, state):
        self.val = state[0] // 10
        self.transient = "restored"


class FalsyState:
    def __init__(self, n):
        self.n = n

    def __getstate__(self):
        return 0

    def __setstate__(self, state):
        self.n = state + 42


def _make_reduced(tag):
    return ReducedBox(tag)


def _apply_box_state(obj, state):
    obj.extra = state


class ReducedBox:
    def __init__(self, tag):
        self.tag = tag
        self.items = []
        self.mapping = {}
        self.extra = None

    def append(self, x):
        self.items.append(x)

    def __setitem__(self, k, v):
        self.mapping[k] = v

    def __reduce__(self):
        return (
            _make_reduced,
            (self.tag,),
            "custom_extra",
            iter(self.items),
            iter(self.mapping.items()),
            _apply_box_state,
        )


class CustomReduceEx:
    def __init__(self, value):
        self.value = value

    def __reduce_ex__(self, protocol):
        return (CustomReduceEx, (self.value + protocol,))


class GetNewArgsObj:
    def __new__(cls, x, y=0):
        obj = super().__new__(cls)
        obj.x = x
        obj.y = y
        return obj

    def __getnewargs__(self):
        return (self.x,)


class GetNewArgsExObj:
    def __new__(cls, x, *, y=0):
        obj = super().__new__(cls)
        obj.x = x
        obj.y = y
        return obj

    def __getnewargs_ex__(self):
        return ((self.x,), {"y": self.y})


class Node:
    def __init__(self, name):
        self.name = name
        self.next = None


class MyList(list):
    def __init__(self, items=(), tag="default"):
        super().__init__(items)
        self.tag = tag


class MyDict(dict):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.meta = "m"


class MyInt(int):
    pass


# Globals
for proto in ALL_PROTOCOLS:
    for g in (
        int,
        list,
        dict,
        len,
        abs,
        math.sin,
        ValueError,
        PickleError,
        top_level_func,
        Point,
        Outer,
        Outer.Inner,
    ):
        assert loads(dumps(g, protocol=proto)) is g

    # Plain class
    pt = Point(3, 4)
    rt_pt = loads(dumps(pt, protocol=proto))
    assert type(rt_pt) is Point and rt_pt == pt

    # Nested class instance
    inner = Outer.Inner(77)
    rt_in = loads(dumps(inner, protocol=proto))
    assert type(rt_in) is Outer.Inner and rt_in.v == 77

    # Slotted class & slotted+dict subclass
    sl = Slotted(10, 20)
    if proto < 2:
        try:
            dumps(sl, protocol=proto)
            assert False, f"expected TypeError for slotted class at protocol {proto}"
        except TypeError:
            pass
    else:
        rt_sl = loads(dumps(sl, protocol=proto))
        assert type(rt_sl) is Slotted and (rt_sl.a, rt_sl.b) == (10, 20)

    swd = SlottedWithDict(1, 2)
    swd.extra = 3
    if proto >= 2:
        rt_swd = loads(dumps(swd, protocol=proto))
        assert (rt_swd.a, rt_swd.b, rt_swd.extra) == (1, 2, 3)

    # Custom __getstate__ / __setstate__
    cs = CustomState(7, "temp")
    rt_cs = loads(dumps(cs, protocol=proto))
    assert (rt_cs.val, rt_cs.transient) == (7, "restored")

    # Falsy state: omitted in proto < 2 (copyreg._reduce_ex), preserved in proto >= 2
    fs = FalsyState(9)
    rt_fs = loads(dumps(fs, protocol=proto))
    if proto < 2:
        assert not hasattr(rt_fs, "n")
    else:
        assert rt_fs.n == 42

    # 6-tuple __reduce__ (with listitems, dictitems, state_setter)
    box = ReducedBox("hello")
    box.items = [1, 2, 3]
    box.mapping = {"k": "v"}
    rt_box = loads(dumps(box, protocol=proto))
    assert (rt_box.tag, rt_box.items, rt_box.mapping, rt_box.extra) == (
        "hello",
        [1, 2, 3],
        {"k": "v"},
        "custom_extra",
    )

    # Custom __reduce_ex__
    re_obj = CustomReduceEx(100)
    rt_re = loads(dumps(re_obj, protocol=proto))
    assert rt_re.value == 100 + proto

    # __getnewargs__ & __getnewargs_ex__
    gna = GetNewArgsObj(5, 6)
    rt_gna = loads(dumps(gna, protocol=proto))
    assert (rt_gna.x, rt_gna.y) == (5, 6)

    gna_ex = GetNewArgsExObj(5, y=9)
    rt_gna_ex = loads(dumps(gna_ex, protocol=proto))
    assert (rt_gna_ex.x, rt_gna_ex.y) == (5, 9)

    # Cyclic custom instances
    n1 = Node("first")
    n2 = Node("second")
    n1.next = n2
    n2.next = n1
    rt_n1 = loads(dumps(n1, protocol=proto))
    assert rt_n1.name == "first" and rt_n1.next.name == "second" and rt_n1.next.next is rt_n1

    # Builtin subclasses
    ml = MyList([1, 2, 3], tag="custom")
    rt_ml = loads(dumps(ml, protocol=proto))
    assert type(rt_ml) is MyList and list(rt_ml) == [1, 2, 3] and rt_ml.tag == "custom"

    md = MyDict(a=1, b=2)
    md.meta = "updated"
    rt_md = loads(dumps(md, protocol=proto))
    assert type(rt_md) is MyDict and dict(rt_md) == {"a": 1, "b": 2} and rt_md.meta == "updated"

    mi = MyInt(42)
    rt_mi = loads(dumps(mi, protocol=proto))
    assert type(rt_mi) is MyInt and rt_mi == 42

    # Exceptions
    err = ValueError("boom", 42)
    rt_err = loads(dumps(err, protocol=proto))
    assert type(rt_err) is ValueError and rt_err.args == ("boom", 42)

    # functools.partial & collections
    p_fn = functools.partial(int, "ff", base=16)
    assert loads(dumps(p_fn, protocol=proto))() == 255
    dq = collections.deque([1, 2, 3], maxlen=5)
    rt_dq = loads(dumps(dq, protocol=proto))
    assert list(rt_dq) == [1, 2, 3] and rt_dq.maxlen == 5

print("custom_classes_and_protocols: OK")


# ---------------------------------------------------------------------------
# 7. dump / load with io.BytesIO, Pickler / Unpickler hooks
# ---------------------------------------------------------------------------

# Sequential dump / load on the same BytesIO stream
buf = io.BytesIO()
for idx, item in enumerate([None, 123, "abc", [1, 2], {"x": Point(1, 2)}]):
    dump(item, buf, protocol=idx)
buf.seek(0)
loaded_seq = [load(buf) for _ in range(5)]
print("seq_load:", loaded_seq[:4], (loaded_seq[4]["x"].x, loaded_seq[4]["x"].y))

# Pickler.clear_memo & .memo
bio = io.BytesIO()
p = Pickler(bio, protocol=4)
shared_s = "hello_memo"
p.dump([shared_s, shared_s])
print("memo_nonempty:", len(p.memo.copy()) > 0)
p.clear_memo()
print("memo_cleared:", len(p.memo.copy()) == 0)

# loads accepts bytearray
raw_ba = bytearray(dumps({"ok": [1, 2, 3]}, protocol=4))
print("loads_bytearray:", loads(raw_ba))


# Persistent IDs (both proto 0 ASCII PERSID and proto 1-5 BINPERSID)
class Record:
    def __init__(self, key, payload):
        self.key = key
        self.payload = payload


DB = {"r1": Record("r1", "alpha"), "r2": Record("r2", "beta")}


class DBPickler(Pickler):
    def __init__(self, file, protocol=None):
        super().__init__(file, protocol=protocol)
        self._proto = protocol

    def persistent_id(self, obj):
        if isinstance(obj, Record):
            if self._proto == 0:
                return f"Record:{obj.key}"
            return ("Record", obj.key)
        return None


class DBUnpickler(Unpickler):
    def persistent_load(self, pid):
        if isinstance(pid, str) and pid.startswith("Record:"):
            return DB[pid.split(":", 1)[1]]
        if isinstance(pid, tuple) and pid[0] == "Record":
            return DB[pid[1]]
        raise UnpicklingError(f"unsupported pid: {pid!r}")


for proto in ALL_PROTOCOLS:
    stream = io.BytesIO()
    DBPickler(stream, protocol=proto).dump([DB["r1"], DB["r2"], "plain"])
    stream.seek(0)
    res = DBUnpickler(stream).load()
    assert res[0] is DB["r1"] and res[1] is DB["r2"] and res[2] == "plain"

print("persistent_id: OK")


# Unpickler.find_class restriction hook
class RestrictedUnpickler(Unpickler):
    def find_class(self, module, name):
        if module == "builtins" and name in ("range", "slice"):
            return super().find_class(module, name)
        raise UnpicklingError(f"global '{module}.{name}' is forbidden")


allowed_data = dumps(range(5), protocol=4)
print("restricted_ok:", RestrictedUnpickler(io.BytesIO(allowed_data)).load())
blocked_data = dumps(Point(1, 2), protocol=4)
try:
    RestrictedUnpickler(io.BytesIO(blocked_data)).load()
except UnpicklingError as e:
    print("restricted_blocked:", type(e).__name__, str(e))


# ---------------------------------------------------------------------------
# 8. PickleBuffer & protocol 5 out-of-band buffers
# ---------------------------------------------------------------------------

pb_ro = PickleBuffer(b"hello world")
pb_rw = PickleBuffer(bytearray(b"mutable"))
print("pb_inband_ro:", loads(dumps(pb_ro, protocol=5)))
print("pb_inband_rw:", loads(dumps(pb_rw, protocol=5)))

# Out-of-band via buffer_callback returning None (falsy -> out-of-band)
oob_buffers = []
oob_data = dumps([pb_ro, pb_rw, 42], protocol=5, buffer_callback=oob_buffers.append)
print("pb_oob_count:", len(oob_buffers))
oob_loaded = loads(oob_data, buffers=oob_buffers)
print("pb_oob_vals:", bytes(oob_loaded[0]), bytes(oob_loaded[1]), oob_loaded[2])

# buffer_callback returning True -> kept in-band
inband_seen = []
inband_data = dumps(
    pb_ro, protocol=5, buffer_callback=lambda b: (inband_seen.append(bytes(b)), True)[1]
)
print("pb_cb_inband:", inband_seen, loads(inband_data))

# PickleBuffer release
pb_rel = PickleBuffer(b"temp")
pb_rel.release()
try:
    pb_rel.raw()
except ValueError as e:
    print("pb_released_err:", type(e).__name__)


# ---------------------------------------------------------------------------
# 9. Error handling parity
# ---------------------------------------------------------------------------


def check_err(label, fn):
    try:
        fn()
        print(label, "NO_ERROR")
    except Exception as e:
        print(label, type(e).__name__)


check_err("bad_proto_hi:", lambda: dumps(1, protocol=6))
check_err("bad_proto_99:", lambda: dumps(1, protocol=99))
check_err("bufcb_p4:", lambda: dumps(1, protocol=4, buffer_callback=lambda b: None))
check_err("pb_p4:", lambda: dumps(PickleBuffer(b"x"), protocol=4))
check_err("loads_str:", lambda: loads("not bytes"))
check_err("loads_int:", lambda: loads(123))
check_err("loads_empty:", lambda: loads(b""))
check_err("loads_bad_op:", lambda: loads(b"\xff."))
check_err("loads_bad_proto:", lambda: loads(b"\x80\x06N."))
check_err("loads_trunc_proto:", lambda: loads(b"\x80"))
check_err("loads_trunc_binint:", lambda: loads(b"J\x01\x02"))
check_err(
    "loads_trunc_frame:",
    lambda: loads(b"\x80\x04\x95\x10\x00\x00\x00\x00\x00\x00\x00N."),
)
check_err("loads_underflow_stop:", lambda: loads(b"."))
check_err("loads_underflow_reduce:", lambda: loads(b"R."))
check_err("loads_bad_memo_get:", lambda: loads(b"g99\n."))
check_err("loads_bad_memo_binget:", lambda: loads(b"h\x05."))
check_err("loads_neg_long4:", lambda: loads(b"\x8b\xff\xff\xff\xff."))
check_err("loads_unquoted_str:", lambda: loads(b"Shello\n."))
check_err("loads_no_stop:", lambda: loads(b"N"))
check_err("loads_oob_no_buffers:", lambda: loads(oob_data))
check_err("loads_oob_short_buffers:", lambda: loads(oob_data, buffers=[]))
_top_lambda = lambda x: x
check_err("dumps_top_lambda:", lambda: dumps(_top_lambda))
check_err("dumps_nested_lambda:", lambda: dumps(lambda x: x))


def _make_local():
    class Local:
        pass

    return Local


LocalClass = _make_local()
check_err("dumps_local_cls:", lambda: dumps(LocalClass))
check_err("dumps_local_inst:", lambda: dumps(LocalClass()))
check_err("pickler_bad_file:", lambda: Pickler(object()))
check_err("unpickler_bad_file:", lambda: Unpickler(object()))


# ---------------------------------------------------------------------------
# 10. Additional wire-format & protocol edge cases:
#     custom class wire parity, multi-frame >64KB, fix_imports, dispatch_table,
#     reducer_override, surrogates, encode_long/decode_long, real binary file I/O
# ---------------------------------------------------------------------------

# Custom class & graph wire-format parity with CPython 3.12
for proto in ALL_PROTOCOLS:
    print(f"wire:Point:p{proto}:", dumps(Point(3, 4), protocol=proto))
    print(f"wire:GetNewArgs:p{proto}:", dumps(GetNewArgsObj(5, 6), protocol=proto))
    if proto >= 2:
        print(f"wire:Slotted:p{proto}:", dumps(Slotted(10, 20), protocol=proto))
    if proto >= 4:
        print(f"wire:GetNewArgsEx:p{proto}:", dumps(GetNewArgsExObj(5, y=9), protocol=proto))
    sh = [1, 2]
    print(f"wire:shared:p{proto}:", dumps([sh, sh], protocol=proto))
    rl = [1]
    rl.append(rl)
    print(f"wire:reclist:p{proto}:", dumps(rl, protocol=proto))

# Multi-frame framing (> _FRAME_SIZE_TARGET = 65536) in protocols 4 & 5
big_blob = b"x" * 70000
big_ba = bytearray(b"y" * 70000)
big_str = "z" * 70000
for proto in (3, 4, 5):
    assert loads(dumps([big_blob, 1, big_str, 2], protocol=proto)) == [big_blob, 1, big_str, 2]
    rt_big_ba = loads(dumps([big_ba, 3], protocol=proto))
    assert type(rt_big_ba[0]) is bytearray and rt_big_ba == [big_ba, 3]
    # Exact wire format check on large payload framing
    print(f"large_blob_len:p{proto}:", len(dumps([1, big_blob, 2], protocol=proto)))

# fix_imports=False vs True on protocols 0, 1, 2
print("fix_imports_true:", dumps(range(3), protocol=2, fix_imports=True))
print("fix_imports_false:", dumps(range(3), protocol=2, fix_imports=False))
print(
    "load_fix_imports_true:",
    loads(b"\x80\x02c__builtin__\nxrange\nq\x00K\x03\x85q\x01Rq\x02.", fix_imports=True),
)

# Pickler.dispatch_table & reducer_override
class CustomTarget:
    def __init__(self, v):
        self.v = v


def _rebuild_target(v):
    return CustomTarget(v + 100)


bio_dt = io.BytesIO()
p_dt = Pickler(bio_dt, protocol=4)
p_dt.dispatch_table = {CustomTarget: lambda o: (_rebuild_target, (o.v,))}
p_dt.dump(CustomTarget(5))
print("dispatch_table:", loads(bio_dt.getvalue()).v)


class OverridePickler(Pickler):
    def reducer_override(self, obj):
        if isinstance(obj, CustomTarget):
            return (_rebuild_target, (obj.v * 2,))
        return NotImplemented


bio_ro = io.BytesIO()
OverridePickler(bio_ro, protocol=4).dump(CustomTarget(7))
print("reducer_override:", loads(bio_ro.getvalue()).v)

# Surrogate code points in protocols 1-5 (UTF-8 with surrogatepass)
surr = "a\ud800\udfffb"
for proto in (1, 2, 3, 4, 5):
    assert loads(dumps(surr, protocol=proto)) == surr
print("surrogates: OK")

# encode_long / decode_long
for n in (0, 1, -1, 127, -128, 128, -129, 255, -256, 32767, -32768, 10**40, -(10**40)):
    enc = pickle.encode_long(n)
    assert pickle.decode_long(enc) == n
print("encode_decode_long: OK")

# Real binary file dump / load via tempfile
with tempfile.TemporaryDirectory() as tmpdir:
    fpath = os.path.join(tmpdir, "sample.pkl")
    payload = {"nums": [1, 2, 10**30], "pt": Point(8, 9), "raw": b"\x00\xff"}
    with open(fpath, "wb") as f:
        dump(payload, f, protocol=4)
    with open(fpath, "rb") as f:
        loaded_file = load(f)
    print(
        "file_io:",
        loaded_file["nums"],
        (loaded_file["pt"].x, loaded_file["pt"].y),
        loaded_file["raw"],
    )

