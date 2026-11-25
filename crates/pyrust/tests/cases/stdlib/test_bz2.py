# Comprehensive CPython 3.12 parity tests for `bz2` (issue #3224).

import bz2
import io
import os
import pathlib
import sys
import tempfile


def expect_exc_type(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return type(e).__name__


# 1. Module metadata and public surface
print("in sys.modules:", "bz2" in sys.modules)
print("all sorted:", sorted(bz2.__all__))
print("BZ2File module:", bz2.BZ2File.__module__)

# 2. Decompress hardcoded CPython-generated BZip2 streams (single, empty, multi-stream)
BZ2_HELLO = bytes.fromhex(
    "425a6839314159265359128d93e10000279f8060041000104000100624d08020004052a8006814c0006699a689c2"
    "78984dc9c2613a4ed3d4c26a4f93e4ed3a4fc5dc914e142404a364f840"
)
BZ2_MULTI = bytes.fromhex(
    "425a6839314159265359171d54b60000049180400022039c00200022068f53420c9884a30d074fc5dc914e142405"
    "c7552d80425a68393141592653599a62fa950000031180400022029c80200022069a3d420c98849e0702f1772453"
    "850909a62fa950"
)
BZ2_EMPTY = bytes.fromhex("425a683917724538509000000000")

print("cpy hello:", bz2.decompress(BZ2_HELLO) == b"Hello, BZip2 world! " * 10)
print("cpy multi:", bz2.decompress(BZ2_MULTI))
print("cpy empty:", bz2.decompress(BZ2_EMPTY))
print("empty bytes:", bz2.decompress(b""))
print("trailing garbage:", bz2.decompress(BZ2_HELLO + b"not-bz2-trailing") == b"Hello, BZip2 world! " * 10)

# 3. Roundtrip compression across all levels 1..=9 and payload shapes
payloads = [
    b"",
    b"a",
    b"The quick brown fox jumps over the lazy dog." * 25,
    bytes(range(256)) * 16,
]
for level in range(1, 10):
    for p in payloads:
        c = bz2.compress(p, compresslevel=level)
        assert c.startswith(b"BZh")
        assert bz2.decompress(c) == p
print("levels 1..9 roundtrip ok")

# Multi-stream roundtrip via compress + concat
multi_rt = bz2.compress(b"chunk-A ") + bz2.compress(b"chunk-B ") + bz2.compress(b"chunk-C")
print("multi roundtrip:", bz2.decompress(multi_rt))

# Bytes-like inputs (bytearray, memoryview)
print(
    "bytes-like:",
    bz2.decompress(bytearray(bz2.compress(bytearray(b"ba-data")))) == b"ba-data",
    bz2.decompress(memoryview(bz2.compress(memoryview(b"mv-data")))) == b"mv-data",
)

# 4. Streaming BZ2Compressor
comp = bz2.BZ2Compressor(5)
parts = []
for chunk in [b"part one ", b"part two " * 40, b"", b"part three"]:
    parts.append(comp.compress(chunk))
parts.append(comp.flush())
streamed_bz2 = b"".join(parts)
print(
    "streaming comp ok:",
    bz2.decompress(streamed_bz2) == b"part one " + b"part two " * 40 + b"part three",
)
print("comp after flush compress:", expect_exc_type(comp.compress, b"x"))
print("comp after flush flush:", expect_exc_type(comp.flush))

# 5. Streaming BZ2Decompressor (eof, needs_input, unused_data, max_length, byte-by-byte)
dec = bz2.BZ2Decompressor()
print("dec init:", dec.eof, dec.needs_input, dec.unused_data)
out_chunks = []
out1 = dec.decompress(BZ2_HELLO + b"TRAILER", max_length=12)
print("dec max_length=12:", len(out1), out1, dec.eof, dec.needs_input, dec.unused_data)
out_chunks.append(out1)
while not dec.eof:
    chunk = dec.decompress(b"", max_length=17)
    out_chunks.append(chunk)
print(
    "dec drained:",
    b"".join(out_chunks) == b"Hello, BZip2 world! " * 10,
    dec.eof,
    dec.needs_input,
    dec.unused_data,
)
print("dec after eof:", expect_exc_type(dec.decompress, b""))

# Byte-by-byte streaming feed
dec_slow = bz2.BZ2Decompressor()
slow_out = bytearray()
for i in range(len(BZ2_HELLO)):
    slow_out.extend(dec_slow.decompress(BZ2_HELLO[i : i + 1]))
print(
    "dec byte-by-byte:",
    bytes(slow_out) == b"Hello, BZip2 world! " * 10,
    dec_slow.eof,
    dec_slow.needs_input,
)

# 6. BZ2File with BytesIO (write, read, readline, readlines, iter, seek, tell, multi-stream)
buf = io.BytesIO()
with bz2.BZ2File(buf, "wb", compresslevel=6) as bf:
    print("bf write props:", bf.writable(), bf.readable(), bf.closed)
    n = bf.write(b"line 1\nline 2\n")
    bf.writelines([b"line 3\n", b"line 4"])
    print("bf write len/tell:", n, bf.tell())
print("bf closed after ctx:", bf.closed)

buf.seek(0)
with bz2.BZ2File(buf, "rb") as bf:
    print("bf read props:", bf.readable(), bf.writable(), bf.seekable(), bf.closed)
    print("bf readline 1:", bf.readline())
    print("bf readline 2:", bf.readline(4))
    print("bf tell:", bf.tell())
    bf.seek(0)
    print("bf readlines:", bf.readlines())
    bf.seek(-6, io.SEEK_END)
    print("bf seek end:", bf.read())
    bf.seek(0)
    print("bf iter:", list(bf))

# Multi-stream BZ2File
with bz2.BZ2File(io.BytesIO(BZ2_MULTI), "rb") as bf:
    print("bf multi-stream:", bf.read())

# 7. Filesystem BZ2File & bz2.open (binary + text modes + append + exclusive + PathLike)
with tempfile.TemporaryDirectory(prefix="pyrust_bz2_") as tmpdir:
    p_bin = os.path.join(tmpdir, "sample.bz2")
    with bz2.open(pathlib.Path(p_bin), "wb", compresslevel=3) as f:
        f.write(b"first stream\n")
    with bz2.open(p_bin, "ab") as f:
        f.write(b"second stream\n")
    with bz2.open(p_bin, "rb") as f:
        print("fs bin multi:", f.read())

    # Exclusive create ('xb') fails if file exists
    print("fs xb exists:", expect_exc_type(bz2.open, p_bin, "xb"))
    p_excl = os.path.join(tmpdir, "excl.bz2")
    with bz2.open(p_excl, "xb") as f:
        f.write(b"exclusive data")
    with bz2.open(p_excl, "rb") as f:
        print("fs xb read:", f.read())

    # Text mode ('wt', 'at', 'rt', 'xt') with unicode & newline handling
    p_txt = os.path.join(tmpdir, "text.bz2")
    with bz2.open(p_txt, "wt", encoding="utf-8", newline="\n") as f:
        f.write("hello \u30c6\u30b9\u30c8\nsecond line\n")
    with bz2.open(p_txt, "at", encoding="utf-8", newline="\n") as f:
        f.write("third line\n")
    with bz2.open(p_txt, "rt", encoding="utf-8") as f:
        print("fs text lines:", list(f))
    with bz2.open(p_txt, "rt", encoding="utf-8") as f:
        print("fs text read(5):", f.read(5))
        print("fs text rest:", f.read())

    # Text mode with BytesIO
    tbuf = io.BytesIO()
    with bz2.open(tbuf, "wt", encoding="utf-8", newline="\n") as f:
        f.write("alpha\nbeta\n")
    tbuf.seek(0)
    with bz2.open(tbuf, "rt", encoding="utf-8") as f:
        print("mem text:", f.readlines())

# 8. Error handling parity
print("err bad level 0:", expect_exc_type(bz2.BZ2Compressor, 0))
print("err bad level 10:", expect_exc_type(bz2.BZ2Compressor, 10))
print("err bad data:", expect_exc_type(bz2.decompress, b"not bzip2 data"))
print("err truncated:", expect_exc_type(bz2.decompress, BZ2_HELLO[:-4]))
print("err corrupt crc:", expect_exc_type(bz2.decompress, BZ2_HELLO[:-1] + bytes([BZ2_HELLO[-1] ^ 0xFF])))
print("err open bad mode tb:", expect_exc_type(bz2.open, io.BytesIO(), "rtb"))
print("err open enc in bin:", expect_exc_type(bz2.open, io.BytesIO(), "rb", encoding="utf-8"))
print("err file bad mode:", expect_exc_type(bz2.BZ2File, io.BytesIO(), "z"))
print("err type str:", expect_exc_type(bz2.compress, "not bytes"))
