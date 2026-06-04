# io.StringIO / BytesIO — writelines, line iteration, capability predicates,
# flush/isatty/fileno, closed property, BytesIO readinto/read1 (issue #2008).

import io

# ── writelines (no separators added) ─────────────────────────────────────────
s = io.StringIO()
assert s.writelines(["a", "b", "c"]) is None
assert s.getvalue() == "abc"

b = io.BytesIO()
assert b.writelines([b"x", b"y", bytearray(b"z")]) is None
assert b.getvalue() == b"xyz"

# ── line iteration: __iter__ returns self, __next__ yields lines ─────────────
sio = io.StringIO("a\nb\nc")
assert iter(sio) is sio
assert list(io.StringIO("a\nb\nc")) == ["a\n", "b\n", "c"]
assert next(iter(io.StringIO("only"))) == "only"
assert list(io.BytesIO(b"a\nb\nc")) == [b"a\n", b"b\n", b"c"]

# StopIteration at EOF
it = iter(io.StringIO(""))
try:
    next(it)
    print("FAIL: expected StopIteration")
except StopIteration:
    pass

# ── capability predicates + flush + isatty ───────────────────────────────────
for make in (lambda: io.StringIO("x"), lambda: io.BytesIO(b"x")):
    s = make()
    assert s.seekable() is True
    assert s.readable() is True
    assert s.writable() is True
    assert s.flush() is None
    assert s.isatty() is False
    assert s.closed is False

# ── fileno raises io.UnsupportedOperation (open or closed) ───────────────────
for make in (lambda: io.StringIO("x"), lambda: io.BytesIO(b"x")):
    s = make()
    try:
        s.fileno()
        print("FAIL: expected UnsupportedOperation")
    except io.UnsupportedOperation as e:
        assert str(e) == "fileno"

# ── closed property reflects state ───────────────────────────────────────────
s = io.StringIO("x")
assert s.closed is False
s.close()
assert s.closed is True

# ── BytesIO.readinto fills the buffer and returns the count ──────────────────
b = io.BytesIO(b"abcde")
buf = bytearray(3)
assert b.readinto(buf) == 3
assert buf == bytearray(b"abc")
assert b.tell() == 3

b = io.BytesIO(b"ab")
buf = bytearray(5)
assert b.readinto(buf) == 2
assert bytes(buf) == b"ab\x00\x00\x00"

# readinto rejects a read-only / non-bytes-like target
try:
    io.BytesIO(b"ab").readinto(b"xx")
    print("FAIL: expected TypeError on read-only readinto target")
except TypeError:
    pass

# ── BytesIO.read1 behaves like read for an in-memory stream ──────────────────
assert io.BytesIO(b"abcdef").read1(2) == b"ab"
assert io.BytesIO(b"abcdef").read1() == b"abcdef"
assert io.BytesIO(b"abcdef").read1(-1) == b"abcdef"

# ── writelines type-checks each element ──────────────────────────────────────
try:
    io.StringIO().writelines([1])
    print("FAIL: expected TypeError (StringIO writelines non-str)")
except TypeError:
    pass
try:
    io.BytesIO().writelines([1])
    print("FAIL: expected TypeError (BytesIO writelines non-bytes)")
except TypeError:
    pass

# ── closed-stream predicates raise ValueError ────────────────────────────────
for make in (lambda: io.StringIO("x"), lambda: io.BytesIO(b"x")):
    c = make()
    c.close()
    for op in (c.seekable, c.readable, c.writable, c.isatty):
        try:
            op()
            print("FAIL: expected ValueError on closed predicate")
        except ValueError:
            pass
    # iteration on a closed stream raises ValueError
    try:
        list(c)
        print("FAIL: expected ValueError iterating closed stream")
    except ValueError:
        pass

print("io methods ok")
