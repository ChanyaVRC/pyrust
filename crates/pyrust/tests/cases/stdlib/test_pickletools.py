# CPython 3.12 parity tests for the native `pickletools` module (issue #3248).

import io
import pickle
import pickletools

print("all:", pickletools.__all__)
print("opcode_count:", len(pickletools.opcodes))
print("code2op_len:", len(pickletools.code2op))
print("PROTO_info:", pickletools.code2op["\x80"].name, pickletools.code2op["\x80"].proto)
print("STOP_info:", pickletools.code2op["."].name, pickletools.code2op["."].arg)
print("uint1_info:", pickletools.uint1.name, pickletools.uint1.n)
print("pyint_repr:", repr(pickletools.pyint), pickletools.pyint is pickletools.pylong)
print("markobject_repr:", repr(pickletools.markobject))

# genops across protocols 0 through 5
sample = [1, 2.5, True, False, None, "hello", b"bytes", (3, 4), {"a": [5, 6]}, {7, 8}]
for proto in range(6):
    data = pickle.dumps(sample, protocol=proto)
    ops = [(op.name, arg, pos) for op, arg, pos in pickletools.genops(data)]
    print(f"proto_{proto}_op_names:", [name for name, _, _ in ops])
    print(f"proto_{proto}_stop:", ops[-1][0] == "STOP")

# dis() output parity (including indentlevel, annotate, and shared memo)
for proto in [0, 1, 2, 4, 5]:
    data = pickle.dumps([1, "abc", (2, 3), {"k": 4}], protocol=proto)
    buf = io.StringIO()
    pickletools.dis(data, out=buf)
    print(f"--- dis proto {proto} ---")
    print(buf.getvalue(), end="")

# Annotated disassembly
buf_ann = io.StringIO()
pickletools.dis(pickle.dumps([1, 2], protocol=2), out=buf_ann, annotate=1)
print("--- dis annotated ---")
print(buf_ann.getvalue(), end="")

# Shared memo across two pickles in a BytesIO stream
f = io.BytesIO()
p = pickle.Pickler(f, 2)
shared = [1, 2, 3]
p.dump(shared)
p.dump(shared)
f.seek(0)
memo = {}
buf_m = io.StringIO()
pickletools.dis(f, out=buf_m, memo=memo)
pickletools.dis(f, out=buf_m, memo=memo)
print("--- dis shared memo ---")
print(buf_m.getvalue(), end="")
print("memo_keys:", sorted(memo.keys()))

# optimize() removes unused PUT/BINPUT/MEMOIZE while preserving unpickled value
for proto in range(6):
    shared_inner = ["shared"]
    obj = [shared_inner, shared_inner, {"x": [1, 2, 3], "y": (4, 5)}]
    raw = pickle.dumps(obj, protocol=proto)
    opt = pickletools.optimize(raw)
    print(f"opt_proto_{proto}_shorter_or_eq:", len(opt) <= len(raw))
    unpickled = pickle.loads(opt)
    print(f"opt_proto_{proto}_roundtrip:", unpickled == obj, unpickled[0] is unpickled[1])

# Error handling on truncated / unknown opcodes
for bad in [b"", b"\x80\x02N", b"\xff."]:
    try:
        list(pickletools.genops(bad))
        print("UNEXPECTED_OK", bad)
    except ValueError as e:
        print("bad_pickle_err:", str(e))

# Direct argument reader tests
print("read_uint1:", pickletools.read_uint1(io.BytesIO(b"\xff")))
print("read_uint2:", pickletools.read_uint2(io.BytesIO(b"\x34\x12")))
print("read_int4:", pickletools.read_int4(io.BytesIO(b"\x00\x00\x00\x80")))
print("read_uint4:", pickletools.read_uint4(io.BytesIO(b"\x00\x00\x00\x80")))
print("read_uint8:", pickletools.read_uint8(io.BytesIO(b"\xff\x00\x00\x00\x00\x00\x00\x00")))
print("read_stringnl:", pickletools.read_stringnl(io.BytesIO(b"'ab\\ncd'\n")))
print("read_string1:", pickletools.read_string1(io.BytesIO(b"\x03abc")))
print("read_bytes1:", pickletools.read_bytes1(io.BytesIO(b"\x03xyz")))
print("read_decimalnl_short:", [
    pickletools.read_decimalnl_short(io.BytesIO(b"00\n")),
    pickletools.read_decimalnl_short(io.BytesIO(b"01\n")),
    pickletools.read_decimalnl_short(io.BytesIO(b"42\n")),
])
print("read_long1:", pickletools.read_long1(io.BytesIO(b"\x02\xff\x00")))
