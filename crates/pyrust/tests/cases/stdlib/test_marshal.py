# Parity test for `marshal` stdlib module (CPython 3.12, versions 0-4, issue #3246).
import io
import math
import marshal
import types


# 1. Version attribute
print("version_int:", isinstance(marshal.version, int), marshal.version >= 4)

# 2. Round-trip across all versions 0..4 for all marshallable types
samples = [
    None,
    False,
    True,
    StopIteration,
    Ellipsis,
    0,
    1,
    -1,
    42,
    -2147483648,
    2147483647,
    2147483648,
    -2147483649,
    10**30,
    -(10**45) + 123456789,
    0.0,
    -0.0,
    1.5,
    -3.141592653589793,
    float("inf"),
    float("-inf"),
    complex(1.5, -2.5),
    b"",
    b"hello\x00world\xff",
    "",
    "ascii short",
    "a" * 300,
    "héllo 世界 🦀",
    (),
    (1, "two", (3, 4)),
    tuple(range(260)),
    [],
    [1, "a", None, True, [2, 3]],
    {},
    {"k1": 1, "k2": [1, 2], 3: (4, 5)},
    set(),
    {1, 2, "three"},
    frozenset(),
    frozenset([10, 20, "thirty"]),
]

for ver in (0, 1, 2, 3, 4):
    ok = True
    for obj in samples:
        raw = marshal.dumps(obj, ver)
        out = marshal.loads(raw)
        if out != obj or type(out) is not type(obj):
            ok = False
            print("MISMATCH:", ver, repr(obj), repr(out), type(obj), type(out))
    print(f"roundtrip_v{ver}:", ok)

# NaN roundtrip
for ver in (0, 1, 2, 3, 4):
    out_nan = marshal.loads(marshal.dumps(float("nan"), ver))
    print(f"nan_v{ver}:", math.isnan(out_nan))

# bytearray & memoryview marshal as bytes
for ver in (0, 2, 4):
    out_ba = marshal.loads(marshal.dumps(bytearray(b"abc"), ver))
    out_mv = marshal.loads(marshal.dumps(memoryview(b"xyz"), ver))
    print(f"buf_v{ver}:", out_ba == b"abc", type(out_ba) is bytes, out_mv == b"xyz", type(out_mv) is bytes)

# 3. Reference sharing / FLAG_REF in v3 and v4
shared_s = "repeated_identifier_string"
shared_list = [shared_s, shared_s, shared_s, shared_s]
d4 = marshal.dumps(shared_list, 4)
d2 = marshal.dumps(shared_list, 2)
print("v4_smaller_with_refs:", len(d4) < len(d2), marshal.loads(d4) == shared_list)

# 4. CPython binary wire-format interop (exact CPython 3.12 bytes decoded)
cpython_vectors = [
    ("none", b"N", None),
    ("false", b"F", False),
    ("true", b"T", True),
    ("stopiter", b"S", StopIteration),
    ("ellipsis", b".", Ellipsis),
    ("int_42", b"i*\x00\x00\x00", 42),
    ("int_neg", b"i\xff\xff\xff\xff", -1),
    ("bigint_1e30", b"\xec\x07\x00\x00\x00\x00\x00\x00\x00\xa97\xa73\x04M\x93e'\x03", 10**30),
    ("float_v0", b"f\x031.5", 1.5),
    ("float_v2", b"g\x00\x00\x00\x00\x00\x00\xf8?", 1.5),
    ("complex_v0", b"x\x031.5\x032.5", 1.5 + 2.5j),
    ("complex_v2", b"y\x00\x00\x00\x00\x00\x00\xf8?\x00\x00\x00\x00\x00\x00\x04@", 1.5 + 2.5j),
    ("bytes", b"s\x02\x00\x00\x00hi", b"hi"),
    ("short_ascii", b"z\x05hello", "hello"),
    ("utf8_str", b"u\x06\x00\x00\x00\xe4\xb8\x96\xe7\x95\x8c", "世界"),
    ("small_tuple_ref", b")\x02\xfa\x03abc\x72\x00\x00\x00\x00", ("abc", "abc")),
    ("dict_v2", b"{u\x01\x00\x00\x00ai\x01\x00\x00\x000", {"a": 1}),
    ("frozenset", b">\x02\x00\x00\x00i\x01\x00\x00\x00i\x02\x00\x00\x00", frozenset({1, 2})),
]
for label, raw, expected in cpython_vectors:
    decoded = marshal.loads(raw)
    print(f"wire_{label}:", decoded == expected and type(decoded) is type(expected))

# 5. CodeType marshaling & unmarshaling
def sample_func(a, /, b, *, c=10):
    y = a + b + c
    return y


co = sample_func.__code__
co_bytes = marshal.dumps(co, 4)
co2 = marshal.loads(co_bytes)
print("code_type:", isinstance(co2, types.CodeType))
print("code_name:", co2.co_name == co.co_name, co2.co_qualname == co.co_qualname)
print("code_args:", co2.co_argcount == co.co_argcount, co2.co_posonlyargcount == co.co_posonlyargcount, co2.co_kwonlyargcount == co.co_kwonlyargcount)
print("code_varnames:", co2.co_varnames == co.co_varnames)
print("code_firstlineno:", co2.co_firstlineno == co.co_firstlineno)
print("code_filename:", co2.co_filename == co.co_filename)

# 6. dump / load sequential stream via BytesIO
stream = io.BytesIO()
marshal.dump({"first": [1, 2, 3]}, stream, 4)
marshal.dump((42, "second", frozenset({7, 8})), stream, 2)
marshal.dump(None, stream, 0)
stream.seek(0)
r1 = marshal.load(stream)
r2 = marshal.load(stream)
r3 = marshal.load(stream)
print("stream_load:", r1 == {"first": [1, 2, 3]}, r2 == (42, "second", frozenset({7, 8})), r3 is None)

# 7. Error cases
class Custom:
    pass

try:
    marshal.dumps(Custom())
    print("unmarshallable: none")
except ValueError:
    print("unmarshallable: ValueError")

try:
    marshal.loads(b"")
    print("empty_loads: none")
except EOFError:
    print("empty_loads: EOFError")

try:
    marshal.loads(b"i\x01\x02")
    print("short_int: none")
except EOFError:
    print("short_int: EOFError")

try:
    marshal.loads(b"?")
    print("unknown_tag: none")
except ValueError:
    print("unknown_tag: ValueError")

# Cyclic container: version 2 raises ValueError, version 4 round-trips via FLAG_REF
cyc = []
cyc.append(cyc)
try:
    marshal.dumps(cyc, 2)
    print("cycle_v2_err: none")
except ValueError:
    print("cycle_v2_err: ValueError")
cyc_out = marshal.loads(marshal.dumps(cyc, 4))
print("cycle_v4_self_ref:", isinstance(cyc_out, list), len(cyc_out) == 1, cyc_out[0] is cyc_out)
