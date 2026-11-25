# Comprehensive CPython 3.12 parity tests for `lzma` (issue #3224).

import io
import lzma
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


# 1. Module metadata, constants, exception hierarchy, is_check_supported
print("in sys.modules:", "lzma" in sys.modules)
print("all sorted:", sorted(lzma.__all__))
print(
    "exception:",
    issubclass(lzma.LZMAError, Exception),
    lzma.LZMAFile.__module__,
)
print(
    "formats:",
    lzma.FORMAT_AUTO,
    lzma.FORMAT_XZ,
    lzma.FORMAT_ALONE,
    lzma.FORMAT_RAW,
)
print(
    "checks:",
    lzma.CHECK_NONE,
    lzma.CHECK_CRC32,
    lzma.CHECK_CRC64,
    lzma.CHECK_SHA256,
    lzma.CHECK_ID_MAX,
    lzma.CHECK_UNKNOWN,
)
print(
    "filters:",
    lzma.FILTER_LZMA1,
    lzma.FILTER_LZMA2,
    lzma.FILTER_DELTA,
    lzma.FILTER_X86,
    lzma.FILTER_IA64,
    lzma.FILTER_ARM,
    lzma.FILTER_ARMTHUMB,
    lzma.FILTER_SPARC,
    lzma.FILTER_POWERPC,
)
print(
    "mf/mode/preset:",
    lzma.MF_HC3,
    lzma.MF_HC4,
    lzma.MF_BT2,
    lzma.MF_BT3,
    lzma.MF_BT4,
    lzma.MODE_FAST,
    lzma.MODE_NORMAL,
    lzma.PRESET_DEFAULT,
    lzma.PRESET_EXTREME,
)
print(
    "is_check_supported:",
    [lzma.is_check_supported(i) for i in range(17)],
)

# 2. Decompress hardcoded CPython-generated XZ, ALONE, multi-stream, and filtered streams
XZ_CRC64 = bytes.fromhex(
    "fd377a585a000004e6d6b4460200210116000000742fe5a3e000e5001f5d00241949986f16028928145f56158875"
    "b80dbbfd1c85c290b57bf4e81724000000005d29a776c9d1762f00013be601000000a976c0b2b1c467fb02000000"
    "0004595a"
)
XZ_CRC32 = bytes.fromhex(
    "fd377a585a0000016922de360200210116000000742fe5a301000f48656c6c6f2c20585a20435243333221003070"
    "486c00012410e944d9cc9042990d010000000001595a"
)
XZ_SHA256 = bytes.fromhex(
    "fd377a585a00000ae1fb0ca10200210116000000742fe5a301001048656c6c6f2c20585a20534841323536210000"
    "00002bd4f4c61c5bb0f0e6a3be97b06e7a0406d6045db3a736aa82e15ee66131f831000141119deb54a3189b4b9a"
    "01000000000a595a"
)
XZ_NONE = bytes.fromhex(
    "fd377a585a000000ff12d9410200210116000000742fe5a301000e48656c6c6f2c20585a204e4f4e452100000001"
    "1f0f24a6637d06729e7a010000000000595a"
)
XZ_EMPTY = bytes.fromhex(
    "fd377a585a000004e6d6b446000000001cdf44211fb6f37d010000000004595a"
)
XZ_MULTI = bytes.fromhex(
    "fd377a585a000004e6d6b4460200210116000000742fe5a301000b787a2070617274206f6e6520000847af2cb48c"
    "dcfe0001240ca618d8d81fb6f37d010000000004595afd377a585a000004e6d6b4460200210116000000742fe5a3"
    "01000a787a20706172742074776f0000792020eca7596ad70001230bc21bfd091fb6f37d010000000004595a"
)
ALONE_HELLO = bytes.fromhex(
    "5d00008000ffffffffffffffff00241949986f160287b4cfe5e1eaef574458068d03250c3d2f3ee102fec0a1f24f"
    "fffff1fb0000"
)
ALONE_EMPTY = bytes.fromhex("5d00008000ffffffffffffffff0083fffbffffc0000000")
XZ_DELTA = bytes.fromhex(
    "fd377a585a000004e6d6b44602010301002101104c85a707e003ff000d5d0000009522bf8c2ebc02ca85cf000000"
    "000000c489c78db51fd500012980080000005e18a497b1c467fb020000000004595a"
)

print("cpy xz crc64:", lzma.decompress(XZ_CRC64) == b"Hello, XZ CRC64 world! " * 10)
print("cpy xz crc32:", lzma.decompress(XZ_CRC32))
print("cpy xz sha256:", lzma.decompress(XZ_SHA256))
print("cpy xz none:", lzma.decompress(XZ_NONE))
print("cpy xz empty:", lzma.decompress(XZ_EMPTY))
print("cpy xz multi:", lzma.decompress(XZ_MULTI))
print("cpy alone auto:", lzma.decompress(ALONE_HELLO) == b"Hello, LZMA ALONE world! " * 10)
print("cpy alone explicit:", lzma.decompress(ALONE_HELLO, format=lzma.FORMAT_ALONE) == b"Hello, LZMA ALONE world! " * 10)
print("cpy alone empty:", lzma.decompress(ALONE_EMPTY))
print("cpy xz delta:", lzma.decompress(XZ_DELTA) == bytes(range(256)) * 4)
print("cpy trailing garbage:", lzma.decompress(XZ_CRC32 + b"extra-junk"))

# 3. Roundtrip across formats, checks, presets, and filter chains
sample = b"LZMA compression parity payload! " * 40
for chk in (lzma.CHECK_NONE, lzma.CHECK_CRC32, lzma.CHECK_CRC64, lzma.CHECK_SHA256):
    c = lzma.compress(sample, format=lzma.FORMAT_XZ, check=chk, preset=1)
    assert c.startswith(b"\xfd7zXZ\x00")
    dec = lzma.LZMADecompressor(format=lzma.FORMAT_XZ)
    out = dec.decompress(c)
    assert out == sample and dec.eof and dec.check == chk
print("xz checks roundtrip ok")

for p in (0, 1, 3, 6, 1 | lzma.PRESET_EXTREME):
    c_xz = lzma.compress(sample, format=lzma.FORMAT_XZ, preset=p)
    assert lzma.decompress(c_xz) == sample
    c_alone = lzma.compress(sample, format=lzma.FORMAT_ALONE, preset=p)
    assert lzma.decompress(c_alone, format=lzma.FORMAT_AUTO) == sample
    assert lzma.decompress(c_alone, format=lzma.FORMAT_ALONE) == sample
print("presets xz/alone roundtrip ok")

# Empty payload roundtrip
print("empty xz rt:", lzma.decompress(lzma.compress(b"", format=lzma.FORMAT_XZ)))
print("empty alone rt:", lzma.decompress(lzma.compress(b"", format=lzma.FORMAT_ALONE)))

# Custom XZ filter chain (Delta + LZMA2)
xz_filt = [{"id": lzma.FILTER_DELTA, "dist": 2}, {"id": lzma.FILTER_LZMA2, "preset": 1}]
c_filt = lzma.compress(bytes(range(256)) * 4, format=lzma.FORMAT_XZ, filters=xz_filt)
print("xz custom filter rt:", lzma.decompress(c_filt) == bytes(range(256)) * 4)

# FORMAT_RAW with FILTER_LZMA1 and FILTER_LZMA2
raw_f1 = [{"id": lzma.FILTER_LZMA1, "dict_size": 65536, "lc": 3, "lp": 0, "pb": 2}]
c_raw1 = lzma.compress(sample, format=lzma.FORMAT_RAW, filters=raw_f1)
print("raw lzma1 rt:", lzma.decompress(c_raw1, format=lzma.FORMAT_RAW, filters=raw_f1) == sample)

raw_f2 = [{"id": lzma.FILTER_LZMA2, "dict_size": 65536}]
c_raw2 = lzma.compress(sample, format=lzma.FORMAT_RAW, filters=raw_f2)
print("raw lzma2 rt:", lzma.decompress(c_raw2, format=lzma.FORMAT_RAW, filters=raw_f2) == sample)

# Bytes-like inputs
print(
    "bytes-like:",
    lzma.decompress(bytearray(lzma.compress(bytearray(b"ba-lzma")))) == b"ba-lzma",
    lzma.decompress(memoryview(lzma.compress(memoryview(b"mv-lzma")))) == b"mv-lzma",
)

# 4. Streaming LZMACompressor
comp = lzma.LZMACompressor(format=lzma.FORMAT_XZ, check=lzma.CHECK_SHA256, preset=1)
parts = [comp.compress(b"seg1 "), comp.compress(b"seg2 " * 30), comp.flush()]
streamed_xz = b"".join(parts)
dec_chk = lzma.LZMADecompressor()
print(
    "streaming comp:",
    dec_chk.decompress(streamed_xz) == b"seg1 " + b"seg2 " * 30,
    dec_chk.check == lzma.CHECK_SHA256,
    dec_chk.eof,
)
print("comp after flush compress:", expect_exc_type(comp.compress, b"x"))
print("comp after flush flush:", expect_exc_type(comp.flush))

# 5. Streaming LZMADecompressor (check, eof, needs_input, unused_data, max_length, byte-by-byte)
ld = lzma.LZMADecompressor()
print("ld init:", ld.check, ld.eof, ld.needs_input, ld.unused_data)
p1 = ld.decompress(XZ_CRC64 + b"TAIL", max_length=15)
print("ld max_length=15:", len(p1), p1, ld.check, ld.eof, ld.needs_input, ld.unused_data)
buf_out = bytearray(p1)
while not ld.eof:
    buf_out.extend(ld.decompress(b"", max_length=23))
print(
    "ld drained:",
    bytes(buf_out) == b"Hello, XZ CRC64 world! " * 10,
    ld.check,
    ld.eof,
    ld.needs_input,
    ld.unused_data,
)
print("ld after eof:", expect_exc_type(ld.decompress, b""))

# Byte-by-byte streaming feed (XZ + ALONE via FORMAT_AUTO)
for label, blob, expected in [
    ("xz", XZ_CRC32, b"Hello, XZ CRC32!"),
    ("alone", ALONE_HELLO, b"Hello, LZMA ALONE world! " * 10),
]:
    d = lzma.LZMADecompressor(format=lzma.FORMAT_AUTO)
    acc = bytearray()
    for i in range(len(blob)):
        acc.extend(d.decompress(blob[i : i + 1]))
    print("slow auto", label, bytes(acc) == expected, d.eof, d.needs_input)

# 6. LZMAFile with BytesIO (binary read/write/seek/tell/lines/multi-stream)
mbio = io.BytesIO()
with lzma.LZMAFile(mbio, "wb", preset=1) as lf:
    print("lf write props:", lf.writable(), lf.readable(), lf.closed)
    n = lf.write(b"row1\nrow2\n")
    lf.writelines([b"row3\n", b"row4"])
    print("lf write len/tell:", n, lf.tell())
print("lf closed after ctx:", lf.closed)

mbio.seek(0)
with lzma.LZMAFile(mbio, "rb") as lf:
    print("lf read props:", lf.readable(), lf.writable(), lf.seekable(), lf.closed)
    print("lf readline 1:", lf.readline())
    print("lf readline 2:", lf.readline(3))
    print("lf tell:", lf.tell())
    lf.seek(0)
    print("lf readlines:", lf.readlines())
    lf.seek(-4, io.SEEK_END)
    print("lf seek end:", lf.read())
    lf.seek(0)
    print("lf iter:", list(lf))

with lzma.LZMAFile(io.BytesIO(XZ_MULTI), "rb") as lf:
    print("lf multi-stream:", lf.read())

# 7. Filesystem LZMAFile & lzma.open (binary + text modes + append + exclusive + PathLike)
with tempfile.TemporaryDirectory(prefix="pyrust_lzma_") as tmpdir:
    p_bin = os.path.join(tmpdir, "sample.xz")
    with lzma.open(pathlib.Path(p_bin), "wb", preset=1) as f:
        f.write(b"first xz stream\n")
    with lzma.open(p_bin, "ab", preset=1) as f:
        f.write(b"second xz stream\n")
    with lzma.open(p_bin, "rb") as f:
        print("fs bin multi:", f.read())

    print("fs xb exists:", expect_exc_type(lzma.open, p_bin, "xb"))
    p_excl = os.path.join(tmpdir, "excl.xz")
    with lzma.open(p_excl, "xb", preset=1) as f:
        f.write(b"exclusive xz")
    with lzma.open(p_excl, "rb") as f:
        print("fs xb read:", f.read())

    p_txt = os.path.join(tmpdir, "text.xz")
    with lzma.open(p_txt, "wt", encoding="utf-8", newline="\n", preset=1) as f:
        f.write("hello \u65e5\u672c\u8a9e\nline two\n")
    with lzma.open(p_txt, "at", encoding="utf-8", newline="\n", preset=1) as f:
        f.write("line three\n")
    with lzma.open(p_txt, "rt", encoding="utf-8") as f:
        print("fs text lines:", list(f))
    with lzma.open(p_txt, "rt", encoding="utf-8") as f:
        print("fs text read(5):", f.read(5))
        print("fs text rest:", f.read())

# 8. Error handling parity
print("err empty:", expect_exc_type(lzma.decompress, b""))
print("err bad xz:", expect_exc_type(lzma.decompress, b"not xz data", format=lzma.FORMAT_XZ))
print("err truncated xz:", expect_exc_type(lzma.decompress, XZ_CRC64[:-5]))
print("err corrupt crc:", expect_exc_type(lzma.decompress, XZ_CRC32[:-20] + bytes([XZ_CRC32[-20] ^ 0xFF]) + XZ_CRC32[-19:]))
print("err memlimit tiny:", expect_exc_type(lzma.decompress, XZ_CRC64, memlimit=100))
print("err comp format auto:", expect_exc_type(lzma.LZMACompressor, format=lzma.FORMAT_AUTO))
print("err comp alone check:", expect_exc_type(lzma.LZMACompressor, format=lzma.FORMAT_ALONE, check=lzma.CHECK_CRC64))
print("err comp raw no filters:", expect_exc_type(lzma.LZMACompressor, format=lzma.FORMAT_RAW))
print("err comp preset+filters:", expect_exc_type(lzma.LZMACompressor, preset=1, filters=[{"id": lzma.FILTER_LZMA2}]))
print("err decomp auto filters:", expect_exc_type(lzma.LZMADecompressor, filters=[{"id": lzma.FILTER_LZMA2}]))
print("err decomp raw memlimit:", expect_exc_type(lzma.LZMADecompressor, format=lzma.FORMAT_RAW, memlimit=1000, filters=[{"id": lzma.FILTER_LZMA2}]))
print("err open bad mode tb:", expect_exc_type(lzma.open, io.BytesIO(), "rtb"))
print("err open enc in bin:", expect_exc_type(lzma.open, io.BytesIO(), "rb", encoding="utf-8"))
print("err file read+preset:", expect_exc_type(lzma.LZMAFile, io.BytesIO(), "r", preset=1))
print("err file read+check:", expect_exc_type(lzma.LZMAFile, io.BytesIO(), "r", check=lzma.CHECK_CRC64))
print("err type str:", expect_exc_type(lzma.compress, "not bytes"))
