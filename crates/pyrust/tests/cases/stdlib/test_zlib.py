# Comprehensive CPython 3.12 parity tests for `zlib`.

import array
import binascii
import sys
import zlib


def expect_exc_type(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, isinstance(e, zlib.error))


# 1. Module metadata, constants, exception hierarchy
print("in sys.modules:", "zlib" in sys.modules)
print("error hierarchy:", issubclass(zlib.error, Exception), zlib.error.__module__)
print(
    "constants:",
    zlib.MAX_WBITS,
    zlib.DEFLATED,
    zlib.DEF_MEM_LEVEL,
    zlib.DEF_BUF_SIZE,
    zlib.Z_NO_COMPRESSION,
    zlib.Z_BEST_SPEED,
    zlib.Z_BEST_COMPRESSION,
    zlib.Z_DEFAULT_COMPRESSION,
    zlib.Z_DEFAULT_STRATEGY,
    zlib.Z_FILTERED,
    zlib.Z_HUFFMAN_ONLY,
    zlib.Z_RLE,
    zlib.Z_FIXED,
    zlib.Z_NO_FLUSH,
    zlib.Z_PARTIAL_FLUSH,
    zlib.Z_SYNC_FLUSH,
    zlib.Z_FULL_FLUSH,
    zlib.Z_FINISH,
    zlib.Z_BLOCK,
    zlib.Z_TREES,
)

# 2. crc32 and adler32
for sample in (b"", b"a", b"abc", b"123456789", b"The quick brown fox jumps over the lazy dog"):
    print("crc32:", zlib.crc32(sample), zlib.crc32(sample) == binascii.crc32(sample))
    print("adler32:", zlib.adler32(sample))

# Incremental checksums
c1 = zlib.crc32(b"hello ")
c2 = zlib.crc32(b"world", c1)
print("crc32 incr:", c2 == zlib.crc32(b"hello world"), c2)

a1 = zlib.adler32(b"hello ")
a2 = zlib.adler32(b"world", a1)
print("adler32 incr:", a2 == zlib.adler32(b"hello world"), a2)

# Checksum with bytearray, array, negative/overflow seed values
print("crc32 bytearray:", zlib.crc32(bytearray(b"abc")) == zlib.crc32(b"abc"))
print("adler32 array:", zlib.adler32(array.array("B", b"abc")) == zlib.adler32(b"abc"))
print("crc32 neg seed:", zlib.crc32(b"abc", -1))
print("adler32 mask seed:", zlib.adler32(b"abc", 0x100000001) == zlib.adler32(b"abc", 1))
print("crc32 bad type:", expect_exc_type(zlib.crc32, "abc"))
print("adler32 bad type:", expect_exc_type(zlib.adler32, "abc"))

# 3. Decompress real CPython-generated wire-format streams
Z_STD = bytes.fromhex("789ccb48cdc9c95728cf2fca495154c818991c00449b5c45")
Z_RAW = bytes.fromhex("2b4a2c5748494dcb492c49552848acccc94f4c51281a15c32a0600")
Z_GZ = bytes.fromhex(
    "1f8b080000000000000a4bafca2c50282f4a2c28484d512848acccc94f4c51481f1982001bab0505fc000000"
)
Z_DICT = bytes.fromhex("78bb607710594b265aa542627a62669e2200dbcb129a")
Z_STORED = bytes.fromhex("7801011700e8ff73746f72656420626c6f636b2074657374203132333435678b07bc")

print("cpy std:", zlib.decompress(Z_STD) == b"hello world! " * 20)
print("cpy raw:", zlib.decompress(Z_RAW, -15) == b"raw deflate payload " * 15)
print("cpy gz:", zlib.decompress(Z_GZ, 16 + 15) == b"gzip wrapped payload " * 12)
print("cpy auto std:", zlib.decompress(Z_STD, 32 + 15) == b"hello world! " * 20)
print("cpy auto gz:", zlib.decompress(Z_GZ, 32 + 15) == b"gzip wrapped payload " * 12)
print("cpy wbits=0:", zlib.decompress(Z_STD, 0) == b"hello world! " * 20)
print("cpy stored:", zlib.decompress(Z_STORED) == b"stored block test 12345")

# Decompress CPython zdict stream
zdict = b"common prefix dictionary words hello world"
d_dict = zlib.decompressobj(zdict=zdict)
print("cpy zdict:", d_dict.decompress(Z_DICT) + d_dict.flush() == b"common prefix dictionary words hello world again!")

# Byte-by-byte streaming decompression of real CPython streams
for label, raw_stream, wb, expected in [
    ("std", Z_STD, 15, b"hello world! " * 20),
    ("raw", Z_RAW, -15, b"raw deflate payload " * 15),
    ("gz", Z_GZ, 16 + 15, b"gzip wrapped payload " * 12),
    ("auto", Z_GZ, 32 + 15, b"gzip wrapped payload " * 12),
]:
    dec = zlib.decompressobj(wb)
    parts = []
    for i in range(len(raw_stream)):
        parts.append(dec.decompress(raw_stream[i : i + 1]))
    parts.append(dec.flush())
    print("byte-by-byte", label, b"".join(parts) == expected, dec.eof, dec.unused_data, dec.unconsumed_tail)

# 4. Roundtrip compress/decompress across levels, wbits, strategies, and payload sizes
payloads = [
    b"",
    b"x",
    b"hello world",
    b"A" * 5000,
    (b"The quick brown fox jumps over the lazy dog. " * 200),
    bytes(range(256)) * 30,
    # Pseudo-random incompressible payload (triggers stored/fixed fallback)
    bytes(((i * 73 + 41) ^ (i >> 3)) & 0xFF for i in range(4096)),
]

for level in (-1, 0, 1, 6, 9):
    for p in payloads:
        c = zlib.compress(p, level)
        assert zlib.decompress(c) == p
print("levels roundtrip ok")

# Compression ratio check on repetitive text
rep = b"compression test line with repeated words! " * 300
c_rep = zlib.compress(rep, 6)
print("actually compresses:", len(c_rep) < len(rep) // 4, zlib.decompress(c_rep) == rep)

# wbits modes in compress/decompress
for wb in (-15, -9, 9, 15, 16 + 15):
    c = zlib.compress(rep, level=6, wbits=wb)
    assert zlib.decompress(c, wbits=wb) == rep
print("wbits compress/decompress ok")

# Strategies
for strat in (
    zlib.Z_DEFAULT_STRATEGY,
    zlib.Z_FILTERED,
    zlib.Z_HUFFMAN_ONLY,
    zlib.Z_RLE,
    zlib.Z_FIXED,
):
    co = zlib.compressobj(level=6, strategy=strat)
    out = co.compress(rep[:1000]) + co.compress(rep[1000:]) + co.flush()
    assert zlib.decompress(out) == rep
print("strategies ok")

# 5. Streaming compressobj: multi-chunk, Z_SYNC_FLUSH, Z_FULL_FLUSH, copy(), zdict
co = zlib.compressobj(level=6)
c_part1 = co.compress(b"chunk one " * 50) + co.flush(zlib.Z_SYNC_FLUSH)
co_branch = co.copy()
c_main = c_part1 + co.compress(b"main branch " * 40) + co.flush(zlib.Z_FINISH)
c_alt = c_part1 + co_branch.compress(b"alt branch " * 40) + co_branch.flush(zlib.Z_FINISH)
print(
    "compressobj copy:",
    zlib.decompress(c_main) == b"chunk one " * 50 + b"main branch " * 40,
    zlib.decompress(c_alt) == b"chunk one " * 50 + b"alt branch " * 40,
)

# Full flush
co_ff = zlib.compressobj()
s1 = co_ff.compress(b"first part " * 20) + co_ff.flush(zlib.Z_FULL_FLUSH)
s2 = co_ff.compress(b"second part " * 20) + co_ff.flush(zlib.Z_FINISH)
print("full flush:", zlib.decompress(s1 + s2) == b"first part " * 20 + b"second part " * 20)

# zdict roundtrip (zlib and raw)
for wb in (15, -15):
    co_d = zlib.compressobj(wbits=wb, zdict=zdict)
    cd = co_d.compress(b"common prefix dictionary words and more text") + co_d.flush()
    do_d = zlib.decompressobj(wbits=wb, zdict=zdict)
    res = do_d.decompress(cd) + do_d.flush()
    assert res == b"common prefix dictionary words and more text"
print("zdict roundtrip ok")

# 6. Streaming decompressobj: max_length, unconsumed_tail, unused_data, eof, copy()
big_plain = b"0123456789abcdef" * 100
big_comp = zlib.compress(big_plain)
do = zlib.decompressobj()
chunks = []
out = do.decompress(big_comp, 37)
chunks.append(out)
print("max_length first len:", len(out), "eof:", do.eof, "has_tail:", len(do.unconsumed_tail) > 0)
do_copy = do.copy()
while do.unconsumed_tail and not do.eof:
    chunks.append(do.decompress(do.unconsumed_tail, 53))
chunks.append(do.flush())
print("max_length full:", b"".join(chunks) == big_plain, do.eof, do.unused_data, do.unconsumed_tail)

# Verify copied decompressor finishes identically
copy_chunks = [out]
while do_copy.unconsumed_tail and not do_copy.eof:
    copy_chunks.append(do_copy.decompress(do_copy.unconsumed_tail, 64))
copy_chunks.append(do_copy.flush())
print("decompressobj copy:", b"".join(copy_chunks) == big_plain, do_copy.eof, do_copy.unused_data)

# Unused trailing data & extra calls after EOF
do_trail = zlib.decompressobj()
print("trail:", do_trail.decompress(big_comp + b"TAIL_BYTES") == big_plain, do_trail.eof, do_trail.unused_data, do_trail.unconsumed_tail)
print("after eof:", do_trail.decompress(b"_MORE"), do_trail.unused_data, do_trail.unconsumed_tail)

# 7. Error handling parity
print("err bad data:", expect_exc_type(zlib.decompress, b"not valid zlib"))
print("err truncated:", expect_exc_type(zlib.decompress, Z_STD[:-2]))
print("err bad checksum:", expect_exc_type(zlib.decompress, Z_STD[:-1] + bytes([Z_STD[-1] ^ 1])))
print("err compress bad level:", expect_exc_type(zlib.compress, b"abc", 99))
print("err compress bad wbits:", expect_exc_type(zlib.compress, b"abc", -1, 99))
print("err decompress bad wbits:", expect_exc_type(zlib.decompress, Z_STD, 99))
print("err decompress bad bufsize:", expect_exc_type(zlib.decompress, Z_STD, 15, -1))
print("err compressobj bad level:", expect_exc_type(zlib.compressobj, 99))
print("err compressobj bad wbits:", expect_exc_type(zlib.compressobj, wbits=99))
print("err decompressobj bad wbits:", expect_exc_type(zlib.decompressobj, 99))
print("err decompressobj neg maxlen:", expect_exc_type(zlib.decompressobj().decompress, b"x", -1))
print("err decompressobj zero flush:", expect_exc_type(zlib.decompressobj().flush, 0))
print("err need dict:", expect_exc_type(zlib.decompress, Z_DICT))

co_done = zlib.compressobj()
co_done.flush()
print("err compress after finish:", expect_exc_type(co_done.compress, b"a"))
print("err flush after finish:", expect_exc_type(co_done.flush))
print("err copy after finish:", expect_exc_type(co_done.copy))

# 8. Native module surface (issue #3324): names, classes, metadata
print("public names:", [n for n in dir(zlib) if not n.startswith("_")])
print("dunder version:", zlib.__version__, zlib.__name__)
print(
    "no exported stream classes:",
    hasattr(zlib, "Compress"),
    hasattr(zlib, "Decompress"),
    hasattr(zlib, "_Compress"),
    hasattr(zlib, "_Decompress"),
)
print(
    "error class:",
    zlib.error.__name__,
    zlib.error.__qualname__,
    zlib.error.__module__,
    zlib.error.__mro__ == (zlib.error, Exception, BaseException, object),
    zlib.error.__doc__,
)
err = zlib.error("boom", 1)
print("error instance:", err.args, str(zlib.error("boom")), repr(zlib.error("boom")))
for fn in (zlib.crc32, zlib.adler32, zlib.compress, zlib.decompress, zlib.compressobj, zlib.decompressobj):
    print("function:", fn.__name__, fn.__module__)

co_t = zlib.compressobj()
do_t = zlib.decompressobj()
print(
    "stream types:",
    type(co_t).__name__,
    type(co_t).__qualname__,
    type(co_t).__module__,
    type(do_t).__name__,
    type(do_t).__qualname__,
    type(do_t).__module__,
    type(co_t) is type(zlib.compressobj()),
    type(do_t) is type(zlib.decompressobj()),
    type(co_t) is type(do_t),
)
print(
    "stream repr:",
    repr(co_t).startswith("<zlib.Compress object at 0x"),
    repr(do_t).startswith("<zlib.Decompress object at 0x"),
)
print("stream docs:", type(co_t).__doc__, type(do_t).__doc__)
print(
    "stream attrs:",
    [n for n in dir(co_t) if not n.startswith("__")],
    [n for n in dir(do_t) if not n.startswith("__")],
)
print(
    "dunder copy attrs:",
    all(hasattr(o, n) for o in (co_t, do_t) for n in ("__copy__", "__deepcopy__")),
)

# Stream types cannot be instantiated directly
print("instantiate Compress:", expect_exc_type(type(co_t)))
print("instantiate Decompress:", expect_exc_type(type(do_t)))
for stream_type in (type(co_t), type(do_t)):
    try:
        stream_type()
    except TypeError as e:
        print("instantiate msg:", e)

# Decompress read-only attributes
for attr in ("eof", "unused_data", "unconsumed_tail"):
    try:
        setattr(do_t, attr, 1)
    except AttributeError as e:
        print("readonly", attr, e)
print("fresh decompressobj state:", do_t.eof, do_t.unused_data, do_t.unconsumed_tail)

# Streams are not picklable but are copyable
import copy
import pickle

for obj in (co_t, do_t):
    try:
        pickle.dumps(obj)
    except TypeError as e:
        print("pickle:", e)
print("copy.copy:", type(copy.copy(co_t)).__name__, type(copy.copy(do_t)).__name__)
print("copy.deepcopy:", type(copy.deepcopy(co_t)).__name__, type(copy.deepcopy(do_t)).__name__)
print("copy independence:", copy.copy(co_t) is not co_t, copy.deepcopy(do_t) is not do_t)
print("__copy__:", type(co_t.__copy__()).__name__, type(do_t.__copy__()).__name__)
print("__deepcopy__:", type(co_t.__deepcopy__({})).__name__, type(do_t.__deepcopy__({})).__name__)

# Copies of a compressor diverge independently
cc = zlib.compressobj()
head = cc.compress(b"shared prefix " * 30)
cc_a = copy.copy(cc)
cc_b = copy.deepcopy(cc)
ra = head + cc_a.compress(b"AAAA" * 30) + cc_a.flush()
rb = head + cc_b.compress(b"BBBB" * 30) + cc_b.flush()
print(
    "diverging copies:",
    zlib.decompress(ra) == b"shared prefix " * 30 + b"AAAA" * 30,
    zlib.decompress(rb) == b"shared prefix " * 30 + b"BBBB" * 30,
)

# Decompress copy preserves eof / unused_data / unconsumed_tail
dd = zlib.decompressobj()
dd.decompress(zlib.compress(b"abc") + b"TAIL")
dd_copy = copy.copy(dd)
print("decompress copy state:", dd_copy.eof, dd_copy.unused_data, dd_copy.unconsumed_tail)
dd_part = zlib.decompressobj()
dd_part.decompress(zlib.compress(b"x" * 500), 7)
dd_part_copy = dd_part.copy()
print("decompress copy tail:", dd_part_copy.eof, dd_part_copy.unconsumed_tail == dd_part.unconsumed_tail)

# 9. Bytes-like inputs and argument conversion
sample = b"The quick brown fox jumps over the lazy dog"
for label, conv in (
    ("bytes", bytes),
    ("bytearray", bytearray),
    ("memoryview", memoryview),
    ("array", lambda b: array.array("B", b)),
):
    data = conv(sample)
    comp = zlib.compress(data)
    print(
        "bytes-like",
        label,
        zlib.crc32(data) == zlib.crc32(sample),
        zlib.adler32(data) == zlib.adler32(sample),
        comp == zlib.compress(sample),
        zlib.decompress(conv(comp)) == sample,
        zlib.compressobj().compress(data) == zlib.compressobj().compress(sample),
        zlib.decompressobj().decompress(conv(comp)) == sample,
    )


class IntLike:
    def __init__(self, n):
        self.n = n

    def __index__(self):
        return self.n


print(
    "__index__ args:",
    zlib.crc32(b"abc", IntLike(5)) == zlib.crc32(b"abc", 5),
    zlib.adler32(b"abc", IntLike(7)) == zlib.adler32(b"abc", 7),
)
print("__index__ level:", zlib.decompress(zlib.compress(b"abc", IntLike(9))), zlib.decompress(zlib.compress(b"abc", True)))
print("bool seed:", zlib.crc32(b"abc", True) == zlib.crc32(b"abc", 1), zlib.adler32(b"abc", False) == zlib.adler32(b"abc", 0))
print("big seed:", zlib.crc32(b"abc", 2**64 + 5) == zlib.crc32(b"abc", 5), zlib.adler32(b"abc", -(2**64) - 1) == zlib.adler32(b"abc", -1))
print("checksum range:", 0 <= zlib.crc32(b"abc", -1) < 2**32, 0 <= zlib.adler32(b"abc", -1) < 2**32)
print("checksum empty:", zlib.crc32(b""), zlib.adler32(b""), zlib.crc32(b"", 123), zlib.adler32(b"", 123))
print("keyword args ok:", zlib.decompress(zlib.compress(sample, level=1, wbits=15), wbits=15, bufsize=1) == sample)
print("bufsize zero:", zlib.decompress(zlib.compress(sample), 15, 0) == sample)
co_kw = zlib.compressobj(
    level=1, method=zlib.DEFLATED, wbits=15, memLevel=9, strategy=zlib.Z_FILTERED, zdict=b"quick brown"
)
payload = co_kw.compress(sample) + co_kw.flush()
do_kw = zlib.decompressobj(wbits=15, zdict=b"quick brown")
print("compressobj kwargs:", do_kw.decompress(payload) == sample)
co_fl = zlib.compressobj()
body = co_fl.compress(sample)
print("flush modes:", co_fl.flush(zlib.Z_NO_FLUSH), len(co_fl.flush(zlib.Z_SYNC_FLUSH)) > 0, len(co_fl.flush(zlib.Z_FULL_FLUSH)) > 0)
print("flush after finish no_flush:", co_done.flush(zlib.Z_NO_FLUSH))

# Argument conversion errors (types and messages must match CPython)
print("crc32 str:", expect_exc_type(zlib.crc32, "text"))
print("crc32 float seed:", expect_exc_type(zlib.crc32, b"abc", 1.5))
print("crc32 None:", expect_exc_type(zlib.crc32, None))
print("compress level float:", expect_exc_type(zlib.compress, b"abc", 1.5))
print("compress level huge:", expect_exc_type(zlib.compress, b"abc", 2**40))
print("compress data str:", expect_exc_type(zlib.compress, "abc"))
print("decompress bufsize huge:", expect_exc_type(zlib.decompress, Z_STD, 15, 2**70))
print("decompress data str:", expect_exc_type(zlib.decompress, "abc"))
print("compressobj zdict None:", expect_exc_type(zlib.compressobj, zdict=None))
print("compressobj zdict str:", expect_exc_type(zlib.compressobj, zdict="x"))
print("compressobj gzip zdict:", expect_exc_type(zlib.compressobj, wbits=31, zdict=b"x"))
print("compressobj raw zdict:", expect_exc_type(zlib.compressobj, wbits=-15, zdict=b"x")[0])
print("compressobj bad method:", expect_exc_type(zlib.compressobj, 6, 9))
print("compressobj bad memLevel:", expect_exc_type(zlib.compressobj, memLevel=10))
print("compressobj bad strategy:", expect_exc_type(zlib.compressobj, strategy=9))
print("decompressobj zdict str:", expect_exc_type(zlib.decompressobj, zdict="x"))
print("decompressobj zdict None:", expect_exc_type(zlib.decompressobj, zdict=None))
print("decompressobj zdict bytearray:", expect_exc_type(zlib.decompressobj, zdict=bytearray(b"ab"))[0])
print("flush bad mode type:", expect_exc_type(zlib.compressobj().flush, 1.5))
print("flush bad mode value:", expect_exc_type(zlib.compressobj().flush, 99))
print("flush huge length:", expect_exc_type(zlib.decompressobj().flush, 2**70))
print("flush negative length:", expect_exc_type(zlib.decompressobj().flush, -1))
print("decompress bad max_length type:", expect_exc_type(zlib.decompressobj().decompress, b"x", "a"))
print(
    "positional-only checks:",
    expect_exc_type(zlib.crc32, data=b"abc")[0],
    expect_exc_type(zlib.compressobj().compress, data=b"abc")[0],
)
for call in (
    lambda: zlib.crc32(),
    lambda: zlib.crc32(b"a", 1, 2),
    lambda: zlib.compress(),
    lambda: zlib.decompress(),
    lambda: zlib.compressobj(1, 2, 3, 4, 5, b"", 7),
    lambda: zlib.decompressobj(1, b"", 3),
    lambda: zlib.compress(b"a", foo=1),
):
    print("arity/keyword errors:", expect_exc_type(call)[0])

# Header validation order matches zlib: the checksum is checked before the method
for bad in (b"junk", b"\x78\x00", b"\x79\x9c", b"\x00\x00", b"\x1f\x8b\x09\x00"):
    for wb in (15, 47):
        try:
            zlib.decompress(bad, wb)
        except zlib.error as e:
            print("header error:", bad, wb, e)

# Error messages raised through the streaming interface
d_bad = zlib.decompressobj()
try:
    d_bad.decompress(b"\x78\x9c\xff\xff\xff\xff")
except zlib.error as e:
    print("stream error:", type(e).__name__, e)
for exc_call in (
    lambda: zlib.decompress(Z_STD[:-2]),
    lambda: zlib.decompress(b"x"),
    lambda: zlib.decompress(Z_DICT),
    lambda: zlib.compress(b"abc", 42),
    lambda: zlib.compress(b"abc", 1, 99),
    lambda: zlib.decompress(Z_STD, 99),
    lambda: co_done.compress(b"a"),
    lambda: co_done.flush(),
    lambda: co_done.copy(),
):
    try:
        exc_call()
    except Exception as e:
        print("message:", type(e).__name__, e)

# zlib.error is a plain Exception subclass catchable as such
try:
    zlib.decompress(b"bad")
except Exception as e:
    print("caught as Exception:", type(e) is zlib.error, isinstance(e, zlib.error))
