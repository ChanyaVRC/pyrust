# Comprehensive CPython 3.12 parity tests for `quopri` (issue #3230).

import io
import quopri
import sys


# 1. Module metadata and constants
print("in sys.modules:", "quopri" in sys.modules)
print("__all__:", quopri.__all__)
print("constants:", quopri.ESCAPE, quopri.MAXLINESIZE, quopri.HEX, quopri.EMPTYSTRING)

# 2. encodestring / decodestring matrix
samples = [
    b"",
    b"hello world",
    b"hello world\n",
    b"hello\tworld \nline2\t\n",
    b"hello\r\nworld\r\n",
    b"a=b=c_d e\tf",
    b".\n.hello\n.\r\n.",
    b"a" * 75,
    b"a" * 76,
    b"a" * 77,
    b"a" * 74 + b"=",
    b"a" * 75 + b"=",
    b"a" * 74 + b" ",
    b"a" * 160,
    bytes(range(256)),
    "caf\xe9 \u2603 \u65e5\u672c\u8a9e".encode("utf-8"),
]

for data in samples:
    for quotetabs in (False, True):
        for header in (False, True):
            enc = quopri.encodestring(data, quotetabs=quotetabs, header=header)
            dec = quopri.decodestring(enc, header=header)
            if len(data) <= 80:
                print("enc", repr(data[:20]), len(data), quotetabs, header, "->", repr(enc))
            else:
                print("enc-len", len(data), quotetabs, header, "->", len(enc), "lines:", len(enc.splitlines()))
            # Round-trip check (istext=True normalizes mixed \r\n / \n endings)
            has_mixed_eol = b"\r\n" in data and b"\n" in data.replace(b"\r\n", b"")
            if not has_mixed_eol and bytes(range(256)) != data:
                assert dec == data, (data, quotetabs, header, enc, dec)

# 3. Streaming encode / decode with BytesIO
for data, qt, hdr in [
    (b"Hello, World!\t \nLine 2 = 100%\n", False, False),
    (b"Hello, World!\t \nLine 2 = 100%\n", True, False),
    (b"Subject: hello world_test = 1", False, True),
    (b"x" * 200, False, False),
]:
    out = io.BytesIO()
    quopri.encode(io.BytesIO(data), out, qt, header=hdr)
    encoded = out.getvalue()
    dec_out = io.BytesIO()
    quopri.decode(io.BytesIO(encoded), dec_out, header=hdr)
    decoded = dec_out.getvalue()
    print("stream:", qt, hdr, repr(encoded[:40]), "len:", len(encoded), "rt:", decoded == data)

# 4. Decode edge cases (soft breaks, ==, bad escapes, lowercase hex, trailing =, header _)
decode_cases = [
    (b"hello=\nworld", False),
    (b"hello=\r\nworld", False),
    (b"a==b", False),
    (b"a=20b=2fb=2Fb", False),
    (b"a=zzb=1", False),
    (b"abc=", False),
    (b"hello_world=5Ftest", True),
    (b"hello_world=5Ftest", False),
    (b"hello   \nworld\t\t\n", False),
]
for raw, hdr in decode_cases:
    print("dec edge:", repr(raw), hdr, "->", repr(quopri.decodestring(raw, header=hdr)))

# 5. Helper functions: needsquoting, quote, ishex, unhex
print(
    "helpers:",
    quopri.needsquoting(b"a", False, False),
    quopri.needsquoting(b"=", False, False),
    quopri.needsquoting(b" ", False, False),
    quopri.needsquoting(b" ", True, False),
    quopri.needsquoting(b"\t", True, False),
    quopri.needsquoting(b"_", False, False),
    quopri.needsquoting(b"_", False, True),
    quopri.quote(b"="),
    quopri.quote(b"\x00"),
    quopri.quote(b"\xff"),
    quopri.ishex(b"0"),
    quopri.ishex(b"a"),
    quopri.ishex(b"F"),
    quopri.ishex(b"g"),
    quopri.unhex(b"3D"),
    quopri.unhex(b"ff"),
)
