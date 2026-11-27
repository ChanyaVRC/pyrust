# CPython 3.12.3 parity for the native `quopri` module (issue #3454).

import binascii
import io
import os
import sys
import tempfile
import quopri


def show(f, *a, **k):
    try:
        r = f(*a, **k)
        print("  ->", repr(r))
    except BaseException as e:
        print("  !!", type(e).__name__, e, e.args)


# 1. Module metadata ---------------------------------------------------------
print("__all__:", quopri.__all__)
print("consts:", quopri.ESCAPE, quopri.MAXLINESIZE, quopri.HEX, quopri.EMPTYSTRING)
print("types:", type(quopri.ESCAPE).__name__, type(quopri.MAXLINESIZE).__name__)
print("binascii funcs:", quopri.a2b_qp is binascii.a2b_qp, quopri.b2a_qp is binascii.b2a_qp)
for name in ["needsquoting", "quote", "encode", "encodestring", "decode",
             "decodestring", "ishex", "unhex", "main"]:
    print(name, callable(getattr(quopri, name)), hasattr(quopri, name))
print("public:", sorted(n for n in dir(quopri) if not n.startswith("_")))
print("doc:", quopri.__doc__)
print("name:", quopri.__name__)

# 2. encodestring / decodestring matrix --------------------------------------
samples = [
    b"",
    b"a",
    b"hello world",
    b"hello world\n",
    b"hello world \n",
    b"hello\tworld \nline2\t\n",
    b"hello\r\nworld\r\n",
    b"hello \r\nworld\t\r\n",
    b"mixed\r\nand\nendings\r\n",
    b"a=b=c_d e\tf",
    b".",
    b".\n.hello\n.\r\n.",
    b"trail ",
    b"trail\t",
    b"a" * 74,
    b"a" * 75,
    b"a" * 76,
    b"a" * 77,
    b"a" * 151,
    b"a" * 152,
    b"a" * 153,
    b"a" * 75 + b"=",
    b"a" * 74 + b"=",
    b"a" * 73 + b"=",
    b"a" * 74 + b" ",
    b"a" * 75 + b" ",
    b"a" * 75 + b" \n",
    b"a" * 75 + b"\n" + b"b" * 77,
    b"a" * 70 + b"\xff\xfe\xfd",
    b"x" * 74 + b"\r\n" + b"y" * 76 + b"\r\n",
    b"\r",
    b"\r\r\n",
    b"\n\n",
    b"_ _",
    b"a_b c_d",
    b"\x00\x01\x02\x7f\x80",
    bytes(range(256)),
    bytes(range(256)) * 3,
    "caf\xe9 ☃".encode("utf-8"),
]
for data in samples:
    for quotetabs in (False, True):
        for header in (False, True):
            enc = quopri.encodestring(data, quotetabs=quotetabs, header=header)
            ref = binascii.b2a_qp(data, quotetabs=quotetabs, header=header)
            dec = quopri.decodestring(enc, header=header)
            print(
                "enc", len(data), data[:12], quotetabs, header, "->", enc,
                enc == ref, dec,
            )
    # positional arguments
    print("pos", quopri.encodestring(data, True, True), quopri.encodestring(data, 1, 0))
    print("dflt", quopri.encodestring(data), quopri.decodestring(quopri.encodestring(data)))

# 3. Decode edge cases --------------------------------------------------------
decode_cases = [
    b"",
    b"plain",
    b"hello=\nworld",
    b"hello=\r\nworld",
    b"hello=\rworld",
    b"hello=\rworld\nmore",
    b"hello=\r",
    b"hello=\n",
    b"hello=   \nworld",
    b"hello=  \r\nworld",
    b"hello  \nworld",
    b"hello \t \r\nworld",
    b"a==b",
    b"a===b",
    b"a=3Db",
    b"a=3db",
    b"a=3dB=3Db",
    b"a=20b=2fb=2Fb",
    b"a=zzb=1",
    b"a=1",
    b"a=1z",
    b"a=z1",
    b"a=g0",
    b"abc=",
    b"abc=4",
    b"abc=41",
    b"=",
    b"==",
    b"=\n=\n",
    b"hello_world=5Ftest",
    b"_leading and trailing_",
    b"=C3=A9=E2=98=83",
    b"soft=\nbreak=\r\nhere=\n\nend",
    b"tab\tend\t\n",
    b"line1\nline2\r\nline3",
    b"=00=FF=ff=Fe",
    bytes(range(256)),
]
for raw in decode_cases:
    for hdr in (False, True):
        print("dec", raw[:30], hdr, "->", quopri.decodestring(raw, header=hdr),
              quopri.decodestring(raw, hdr) == binascii.a2b_qp(raw, header=hdr))

# 4. Input types -----------------------------------------------------------
for conv in (bytes, bytearray, memoryview):
    data = conv(b"caf\xe9 =_ \n")
    print("encodestring", conv.__name__, quopri.encodestring(data), quopri.encodestring(data, True, True))
    enc = conv(b"caf=E9 =3D_=20\n")
    print("decodestring", conv.__name__, quopri.decodestring(enc), quopri.decodestring(enc, True))
print("str decode:")
show(quopri.decodestring, "a=3Db_c")
show(quopri.decodestring, "a=3Db_c", header=True)
show(quopri.decodestring, "caf\xe9")
print("str encode:")
show(quopri.encodestring, "text")
show(quopri.encodestring, 12)
show(quopri.encodestring, None)
show(quopri.decodestring, 12)
show(quopri.decodestring, None)
print("truthy flags:")
show(quopri.encodestring, b"a b\t", "yes", [])
show(quopri.encodestring, b"a b_\t", None, 2.5)
show(quopri.decodestring, b"a_b", "x")
print("arity:")
show(quopri.encodestring)
show(quopri.encodestring, b"x", True, True, True)
show(quopri.encodestring, b"x", foo=1)
show(quopri.encodestring, b"x", s=b"y")
show(quopri.decodestring)
show(quopri.decodestring, b"x", True, True)
show(quopri.decodestring, b"x", bad=1)
show(quopri.encode, io.BytesIO(b"a"), io.BytesIO())
show(quopri.encode)
show(quopri.decode, io.BytesIO(b"a"))
show(quopri.needsquoting, b"a")
show(quopri.quote)
show(quopri.ishex)
show(quopri.unhex)
show(quopri.main, 1)

# 5. Streaming encode / decode ------------------------------------------------
streams = [
    (b"Hello, World!\t \nLine 2 = 100%\n", False, False),
    (b"Hello, World!\t \nLine 2 = 100%\n", True, False),
    (b"Subject: hello world_test = 1", False, True),
    (b"Subject: hello world_test = 1", True, True),
    (b"x" * 200, False, False),
    (b"x" * 200 + b"\r\n" + b"y" * 80 + b"\r\n", False, False),
    (b"", False, False),
    (bytes(range(256)), True, True),
]
for data, qt, hdr in streams:
    out = io.BytesIO()
    r = quopri.encode(io.BytesIO(data), out, qt, header=hdr)
    encoded = out.getvalue()
    out2 = io.BytesIO()
    r2 = quopri.decode(io.BytesIO(encoded), out2, header=hdr)
    print("stream", data[:16], qt, hdr, r, r2, encoded, out2.getvalue() == data)
out = io.BytesIO()
quopri.encode(io.BytesIO(b"a b \n"), out, True, True)
print("positional header:", out.getvalue())
out = io.BytesIO()
quopri.decode(io.BytesIO(b"a_b=20"), out, True)
print("positional decode header:", out.getvalue())


class Source:
    def __init__(self, data):
        self.data = data
        self.reads = 0

    def read(self, *args):
        self.reads += 1
        print("  Source.read", args)
        return self.data

    def readline(self):
        raise AssertionError("readline must not be used with binascii")


class Sink:
    def __init__(self):
        self.chunks = []

    def write(self, data):
        print("  Sink.write", data)
        self.chunks.append(data)
        return len(data)


src, sink = Source(b"a=b \nc\xff"), Sink()
print("custom encode:", quopri.encode(src, sink, True), src.reads, sink.chunks)
src, sink = Source(b"a=3Db=\nc=FF"), Sink()
print("custom decode:", quopri.decode(src, sink), src.reads, sink.chunks)
src, sink = Source(bytearray(b"caf\xe9 x")), Sink()
quopri.encode(src, sink, False, True)
print("custom bytearray:", sink.chunks)
src, sink = Source(memoryview(b"a_b=20")), Sink()
quopri.decode(src, sink, True)
print("custom memoryview:", sink.chunks)
show(quopri.encode, Source("str"), Sink(), False)
show(quopri.decode, Source(5), Sink())
show(quopri.encode, object(), Sink(), False)
show(quopri.encode, Source(b"x"), object(), False)

# 6. Helpers ------------------------------------------------------------------
print("needsquoting:")
for c in [b"a", b"A", b"=", b" ", b"\t", b"_", b"~", b"\x7f", b"\x00", b"\xff",
          b"\n", b"\r", b"!", b".", b"", b" \t", b"ab", b"=a", b"_a"]:
    print(" ", c, [quopri.needsquoting(c, qt, hd) for qt in (False, True) for hd in (False, True)])
show(quopri.needsquoting, b" ", "tabs", "hdr")
show(quopri.needsquoting, b"_", "tabs", "hdr")
show(quopri.needsquoting, b"a", "tabs", "hdr")
show(quopri.needsquoting, "a", False, False)
show(quopri.needsquoting, 97, False, False)
show(quopri.needsquoting, bytearray(b"a"), False, False)
show(quopri.needsquoting, memoryview(b"a"), False, False)

print("quote:")
for c in [b"=", b"\x00", b"\xff", b" ", b"a", b"\n", b"\x7f"]:
    print(" ", c, quopri.quote(c))
show(quopri.quote, b"")
show(quopri.quote, b"ab")
show(quopri.quote, "a")
show(quopri.quote, 97)
show(quopri.quote, bytearray(b"a"))


class MyBytes(bytes):
    pass


show(quopri.quote, MyBytes(b"="))
show(quopri.needsquoting, MyBytes(b"="), False, False)
show(quopri.ishex, MyBytes(b"f"))

print("ishex:")
for c in [b"0", b"9", b"a", b"f", b"g", b"A", b"F", b"G", b"/", b":", b"`", b"@", b"", b"1z", b"ff", b"5x", b" "]:
    print(" ", c, quopri.ishex(c))
show(quopri.ishex, "a")
show(quopri.ishex, 97)
show(quopri.ishex, None)

print("unhex:")
for s in [b"", b"0", b"3D", b"3d", b"ff", b"FF", b"ABCDEF", b"abcdef", b"0123456789",
          b"7FFFFFFFFFFFFFFF", b"FFFFFFFFFFFFFFFF", b"FFFFFFFFFFFFFFFFFFFFFFFFFFFF",
          b"123456789ABCDEF0123456789ABCDEF0123456789ABCDEF",
          bytearray(b"1f"), memoryview(b"2a"), [0x31, 0x32], (65, 66), range(48, 50)]:
    print(" ", s if not isinstance(s, memoryview) else "memoryview", quopri.unhex(s))
for s in [b"g", b"3G", b" ", b"'", b'"', b"\\", b"\t", b"\n", b"\r", b"\x00", b"\xff", b"\x7f", b"3D\xe9"]:
    show(quopri.unhex, s)
show(quopri.unhex, "ab")
show(quopri.unhex, 5)
show(quopri.unhex, None)
show(quopri.unhex, [300])
show(quopri.unhex, [-1])
show(quopri.unhex, [1.5])
show(quopri.unhex, ["a"])
show(quopri.unhex, [None])

# 7. Fallback (no binascii) path ----------------------------------------------
saved = (quopri.b2a_qp, quopri.a2b_qp)
quopri.b2a_qp = None
quopri.a2b_qp = None
try:
    fb_samples = [
        b"",
        b"a",
        b"hello world\n",
        b"hello world \n",
        b"hello\tworld \nline2\t\n",
        b"hello\r\nworld\r\n",
        b"a=b=c_d e\tf",
        b".",
        b".\n.hello\n.\r\n.",
        b"trail ",
        b"trail\t",
        b"a" * 75,
        b"a" * 76,
        b"a" * 77,
        b"a" * 152,
        b"a" * 150 + b"\xff\xff" + b"b" * 70,
        b"a" * 74 + b" ",
        b"x\n\n\ny",
        b"no newline",
        b"_ _\n",
        bytes(range(256)),
    ]
    for data in fb_samples:
        for quotetabs in (False, True):
            for header in (False, True):
                enc = quopri.encodestring(data, quotetabs, header)
                print("fb-enc", len(data), data[:12], quotetabs, header, "->", enc)
                out = io.BytesIO()
                quopri.encode(io.BytesIO(data), out, quotetabs, header)
                print("fb-encode", out.getvalue() == enc)
                dec = io.BytesIO()
                quopri.decode(io.BytesIO(enc), dec, header)
                print("fb-rt", dec.getvalue())
    for raw in decode_cases:
        for hdr in (False, True):
            print("fb-dec", raw[:30], hdr, "->", quopri.decodestring(raw, hdr),
                  quopri.decodestring(raw, header=hdr))

    class Lines:
        def __init__(self, lines):
            self.lines = list(lines)

        def readline(self):
            print("  Lines.readline")
            return self.lines.pop(0) if self.lines else b""

        def read(self):
            raise AssertionError("read must not be used without binascii")

    sink = Sink()
    quopri.encode(Lines([b"a b \n", b"c=d\n", b"tail "]), sink, False, True)
    print("fb custom encode:", sink.chunks)
    sink = Sink()
    quopri.decode(Lines([b"a_b=\n", b"c=3D=20  \n", b"=4", b"1"]), sink, True)
    print("fb custom decode:", sink.chunks)
finally:
    quopri.b2a_qp, quopri.a2b_qp = saved
print("restored:", quopri.b2a_qp is binascii.b2a_qp, quopri.a2b_qp is binascii.a2b_qp)

# 8. binascii b2a_qp / a2b_qp parity -------------------------------------------
print("binascii:")
for data in [b"", b"x" * 75, b"x" * 76 + b"\r\n", b"a \nb", b".\r\n.", b"\x00\r", b"tab\t",
             b"line \r\nfoo\r\n", b"a" * 74 + b"\xff", b"a" * 73 + b"\xffz", b"a" * 75 + b"\xff"]:
    for qt in (False, True):
        for it in (False, True):
            for hd in (False, True):
                print(" b2a", data[:10], len(data), qt, it, hd,
                      binascii.b2a_qp(data, quotetabs=qt, istext=it, header=hd))
show(binascii.b2a_qp, "text")
show(binascii.a2b_qp, 5)
show(binascii.a2b_qp, "caf\xe9")
show(binascii.a2b_qp, "ascii=3D")

# 9. main() -------------------------------------------------------------------
tmp = tempfile.mkdtemp()
path = os.path.join(tmp, "input.txt")
with open(path, "wb") as fh:
    fh.write(b"caf\xe9 = tab\there \nlast line")
enc_path = os.path.join(tmp, "encoded.txt")
with open(enc_path, "wb") as fh:
    fh.write(b"caf=E9 =3D soft=\nbreak\n")


class FakeStream:
    """A minimal text stream with a binary `.buffer` (no TextIOWrapper needed)."""

    def __init__(self):
        self.text = ""
        self.buffer = io.BytesIO()

    def write(self, s):
        self.text += s
        return len(s)

    def flush(self):
        pass


def run_main(argv):
    out = FakeStream()
    err = FakeStream()
    stdin = FakeStream()
    stdin.buffer = io.BytesIO(b"from stdin = \t\ncaf\xe9=\n")
    saved = (sys.argv, sys.stdin, sys.stdout, sys.stderr)
    sys.argv = ["quopri"] + argv
    sys.stdin, sys.stdout, sys.stderr = stdin, out, err
    try:
        try:
            quopri.main()
            status = None
        except SystemExit as e:
            status = e.code
        return status, out.buffer.getvalue() + out.text.encode(), err.text + err.buffer.getvalue().decode()
    finally:
        sys.argv, sys.stdin, sys.stdout, sys.stderr = saved


def scrub(text):
    return text.replace(tmp, "<TMP>").replace("\\", "/")


for argv in (
    [path],
    ["-t", path],
    ["-d", enc_path],
    [path, path],
    ["-t", "-d", path],
    ["-x", path],
    ["-t"],
    ["-d", "-"],
    ["-", path],
    [os.path.join(tmp, "missing.txt")],
    [path, os.path.join(tmp, "missing.txt"), enc_path],
):
    status, out_data, err_text = run_main(argv)
    argv_s = [scrub(a) for a in argv]
    # The OSError text is platform specific; keep only the stable prefix.
    err_text = scrub(err_text)
    if "can't open (" in err_text:
        err_text = err_text.split("(")[0] + "(...)"
    print("main", argv_s, status, out_data, err_text)

os.remove(path)
os.remove(enc_path)
os.rmdir(tmp)

print("done")
