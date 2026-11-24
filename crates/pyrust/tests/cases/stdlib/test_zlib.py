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
