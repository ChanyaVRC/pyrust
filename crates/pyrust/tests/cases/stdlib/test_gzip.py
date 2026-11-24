# Comprehensive CPython 3.12 parity tests for `gzip`.

import gzip
import io
import os
import pathlib
import sys
import tempfile


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, isinstance(e, gzip.BadGzipFile), isinstance(e, OSError))


# 1. Module metadata, constants, exception hierarchy
print("in sys.modules:", "gzip" in sys.modules)
print(
    "BadGzipFile hierarchy:",
    issubclass(gzip.BadGzipFile, OSError),
    gzip.BadGzipFile.__module__,
)
print("flags:", gzip.FTEXT, gzip.FHCRC, gzip.FEXTRA, gzip.FNAME, gzip.FCOMMENT)

# 2. Decompress real CPython multi-member gzip fixture
GZ_MULTI = bytes.fromhex(
    "1f8b080000f1536502ff4bcb2c2a2e51c84dcd4d4a2de20200a7f4850a0d000000"
    "1f8b080001f1536502ff2b4e4dcecf4b51c84dcd4d4a2de2020036184b0e0e000000"
)
print("cpy multi-member:", gzip.decompress(GZ_MULTI))

# 3. One-shot compress / decompress with mtime and levels
data = b"hello gzip world!\n" * 100
for lvl in (0, 1, 6, 9):
    comp = gzip.compress(data, compresslevel=lvl, mtime=1700000000)
    assert comp[:3] == b"\x1f\x8b\x08"
    assert gzip.decompress(comp) == data
print("one-shot levels ok")
print("empty roundtrip:", gzip.decompress(gzip.compress(b"", mtime=0)))
print("bytearray roundtrip:", gzip.decompress(gzip.compress(bytearray(b"abc123"), mtime=0)))

# Concatenated members produced by our compress()
m1 = gzip.compress(b"part1-", mtime=100)
m2 = gzip.compress(b"part2-", mtime=200)
m3 = gzip.compress(b"part3", mtime=300)
print("concat members:", gzip.decompress(m1 + m2 + m3))

# 4. GzipFile in-memory (BytesIO) binary read/write/seek/tell/readline/iter
bio = io.BytesIO()
with gzip.GzipFile(filename="sample.txt", mode="wb", fileobj=bio, mtime=12345) as gf:
    n1 = gf.write(b"line 1\n")
    gf.writelines([b"line 2\n", b"line 3\n"])
    gf.flush()
    print("write info:", n1, gf.tell(), gf.readable(), gf.writable())
print("closed after exit:", gf.closed)

raw_gz = bio.getvalue()
print("magic and fname flag:", raw_gz[:4] == b"\x1f\x8b\x08\x08")

bio.seek(0)
with gzip.GzipFile(mode="rb", fileobj=bio) as gf:
    print("read info:", gf.readable(), gf.writable())
    print("read 4:", gf.read(4), "tell:", gf.tell(), "mtime:", gf.mtime)
    print("readline:", gf.readline())
    print("iter rest:", list(gf))
    gf.seek(0)
    print("readlines:", gf.readlines())
    gf.seek(7)
    print("seek 7:", gf.read(6))
    gf.seek(-7, io.SEEK_END)
    print("seek end:", gf.read())

# 5. GzipFile + gzip.open on filesystem (binary, text, append, PathLike)
with tempfile.TemporaryDirectory(prefix="pyrust_gz_") as tmpdir:
    p_bin = os.path.join(tmpdir, "data.bin.gz")
    with gzip.open(p_bin, "wb") as f:
        f.write(b"first ")
    with gzip.open(p_bin, "ab") as f:
        f.write(b"second")
    with gzip.open(pathlib.Path(p_bin), "rb") as f:
        print("fs bin append:", f.read())

    p_txt = os.path.join(tmpdir, "text.txt.gz")
    with gzip.open(p_txt, "wt", encoding="utf-8") as f:
        f.write("alpha\nbeta\n")
        f.writelines(["gamma \u30c6\u30b9\u30c8\n", "delta\n"])
    with gzip.open(p_txt, "at", encoding="utf-8") as f:
        f.write("epsilon\n")
    with gzip.open(p_txt, "rt", encoding="utf-8") as f:
        print("fs txt readline:", f.readline())
        print("fs txt rest:", list(f))
    with gzip.open(p_txt, "rt", encoding="utf-8") as f:
        print("fs txt full:", f.read())

    # gzip.open wrapping BytesIO in text mode
    tbuf = io.BytesIO()
    with gzip.open(tbuf, "wt", encoding="utf-8", newline="\n") as tf:
        tf.write("one\ntwo\nthree\n")
    tbuf.seek(0)
    with gzip.open(tbuf, "rt", encoding="utf-8") as tf:
        print("mem txt lines:", tf.readlines())

# 6. Error handling parity
print("err not gzip:", expect_exc(gzip.decompress, b"not a gzip stream at all"))
print("err truncated:", expect_exc(gzip.decompress, GZ_MULTI[:15]))
print("err bad crc:", expect_exc(gzip.decompress, GZ_MULTI[:25] + bytes([GZ_MULTI[25] ^ 0xFF]) + GZ_MULTI[26:]))
print("err bin encoding:", expect_exc(gzip.open, io.BytesIO(), "rb", encoding="utf-8"))
print("err bad mode:", expect_exc(gzip.GzipFile, fileobj=io.BytesIO(), mode="rt"))
print("err bt mode:", expect_exc(gzip.open, io.BytesIO(), "rbt"))
