import io
import os
import tempfile
import token
import tokenize


def run(label, fn):
    try:
        print(label, "->", fn())
    except BaseException as exc:
        print(label, "EXC", type(exc).__name__, exc.args)


def toks(src, full=False):
    out = []
    for t in tokenize.generate_tokens(io.StringIO(src).readline):
        out.append(tuple(t) if full else (token.tok_name[t.type], t.string))
    return out


# --- public surface -------------------------------------------------------
print("all:", tokenize.__all__[:4], tokenize.__all__[-6:], len(tokenize.__all__))
print("all-covers-token:", set(token.__all__) <= set(tokenize.__all__))
for name in token.__all__:
    assert getattr(tokenize, name) == getattr(token, name), name
print("exact:", tokenize.EXACT_TOKEN_TYPES is token.EXACT_TOKEN_TYPES)
print("consts:", tokenize.ENCODING, tokenize.FSTRING_START, tokenize.FSTRING_MIDDLE,
      tokenize.FSTRING_END, tokenize.COMMENT, tokenize.NL, tokenize.N_TOKENS)
print("tabsize:", tokenize.tabsize)
print("exceptions:", tokenize.TokenError.__mro__[1].__name__,
      tokenize.StopTokenizing.__mro__[1].__name__,
      tokenize.TokenError.__module__, tokenize.StopTokenizing.__module__)
print("classes:", tokenize.TokenInfo.__module__, tokenize.Untokenizer.__module__,
      tokenize.tokenize.__name__, tokenize.detect_encoding.__name__)

# --- TokenInfo ------------------------------------------------------------
ti = tokenize.TokenInfo(tokenize.NAME, "x", (1, 0), (1, 1), "x")
print(repr(ti))
print(ti, ti.type, ti.string, ti.start, ti.end, ti.line)
print(tokenize.TokenInfo._fields, isinstance(ti, tuple), len(ti))
print(ti == (1, "x", (1, 0), (1, 1), "x"), tuple(ti))
print(repr(ti._replace(string="y")))
print(ti.exact_type == tokenize.NAME)
plus = tokenize.TokenInfo(tokenize.OP, "+", (1, 0), (1, 1), "+")
print(plus.exact_type == tokenize.PLUS, repr(plus))
odd = tokenize.TokenInfo(tokenize.OP, "$$", (1, 0), (1, 2), "$$")
print(odd.exact_type == tokenize.OP)
run("repr-bad-type", lambda: repr(tokenize.TokenInfo(999, "x", (1, 0), (1, 1), "x")))
print([c.__name__ for c in tokenize.TokenInfo.__mro__])
a, b, c, d, e = ti
print(a, b, c, d, e)
print(tokenize.TokenInfo(*ti) == ti)

# --- detect_encoding ------------------------------------------------------
def detect(data):
    return tokenize.detect_encoding(io.BytesIO(data).readline)


for label, data in [
    ("default", b"x = 1\n"),
    ("empty", b""),
    ("bom", b"\xef\xbb\xbfx = 1\n"),
    ("bom-only", b"\xef\xbb\xbf"),
    ("cookie1", b"# coding: latin-1\nx = 1\n"),
    ("cookie-emacs", b"# -*- coding: utf-8 -*-\nx = 1\n"),
    ("cookie-vim", b"# vim: set fileencoding=Latin_1 :\n"),
    ("cookie-line2", b"#!/usr/bin/env python3\n# -*- coding: cp1252 -*-\nx = 1\n"),
    ("cookie-line2-blank", b"\n# coding: latin-1\n"),
    ("cookie-line2-crlf", b"\r\n# coding: latin-1\n"),
    ("cookie-line2-ws", b"   \n\x0c# coding: cp1252\n"),
    ("cookie-not-line2", b"x=1\n# coding: latin-1\n"),
    ("cookie-utf8-variants", b"# coding: UTF_8\n"),
    ("cookie-utf8-sig", b"# coding: utf-8-sig\n"),
    ("cookie-iso", b"# coding: iso-8859-1-foo\n"),
    ("cookie-ascii", b"# coding: ASCII\n"),
    ("cookie-empty-name", b"# coding:\n"),
    ("cookie-eq", b"# coding=latin-1\n"),
    ("not-comment", b"x = 1  # coding: latin-1\n"),
    ("bom-utf8-cookie", b"\xef\xbb\xbf# coding: utf-8\nx = 1\n"),
    ("bom-utf8-cookie2", b"\xef\xbb\xbf\n# coding: UTF-8\n"),
]:
    run("detect-" + label, lambda data=data: detect(data))

for label, data in [
    ("bom-latin1", b"\xef\xbb\xbf# coding: latin-1\nx = 1\n"),
    ("unknown", b"# coding: nonexistent-codec-xyz\n"),
    ("bad-utf8", b"\xff\xfe\n"),
    ("bad-utf8-line2", b"\n\xff\n"),
]:
    run("detect-" + label, lambda data=data: detect(data))


class Named(io.BytesIO):
    name = "foo.py"


run("detect-named-unknown", lambda: tokenize.detect_encoding(Named(b"# coding: nope\n").readline))
run("detect-named-bad", lambda: tokenize.detect_encoding(Named(b"\xff\n").readline))
run("detect-named-bom", lambda: tokenize.detect_encoding(Named(b"\xef\xbb\xbf# coding: latin-1\n").readline))
run("detect-stopiteration", lambda: tokenize.detect_encoding(iter([b"x\n"]).__next__))
run("detect-noargs", lambda: tokenize.detect_encoding())
run("detect-str-readline", lambda: tokenize.detect_encoding(io.StringIO("x = 1\n").readline))
run("detect-int-readline", lambda: tokenize.detect_encoding(lambda: 5))
run("detect-not-callable", lambda: tokenize.detect_encoding(5))

calls = []


def counting(lines):
    it = iter(lines)

    def readline():
        calls.append(1)
        return next(it, b"")

    return readline


run("detect-calls", lambda: (tokenize.detect_encoding(counting([b"x = 1\n", b"y = 2\n", b"z = 3\n"])), len(calls)))
calls.clear()
run("detect-calls-blank", lambda: (tokenize.detect_encoding(counting([b"\n", b"\n", b"z = 3\n"])), len(calls)))

# --- tokenize.open ---------------------------------------------------------
fd, path = tempfile.mkstemp(suffix=".py")
os.close(fd)
try:
    for label, data in [
        ("utf8", b"msg = 'caf\xc3\xa9'\r\nx = 1\n"),
        ("bom", b"\xef\xbb\xbf# hi\nx = '\xc3\xa9'\n"),
        ("latin1", b"# coding: latin-1\nx = '\xe9'\n"),
        ("cp1252", b"#!/bin/python\n# -*- coding: cp1252 -*-\nx = '\x80'\n"),
    ]:
        with open(path, "wb") as f:
            f.write(data)
        with tokenize.open(path) as tf:
            print("open-" + label, tf.mode, tf.encoding, repr(tf.read()))
        with tokenize.open(path) as tf:
            print("open-lines-" + label, tf.readlines())
    with open(path, "wb") as f:
        f.write(b"# coding: nope\nx = 1\n")
    try:
        tokenize.open(path)
        print("open-bad: FAIL")
    except SyntaxError as exc:
        text = str(exc)
        print("open-bad:", text.split("'")[0], text.endswith(": nope"), type(exc).__name__)
    run("open-noargs", lambda: tokenize.open())
    with open(path, "wb") as f:
        f.write(b"x = 1\n")
    with open(path, "rb") as f:
        print("tokenize-file:", [t.string for t in tokenize.tokenize(f.readline)][:3])
finally:
    os.unlink(path)
try:
    tokenize.open(path)
    print("open-missing: FAIL")
except FileNotFoundError:
    print("open-missing: FileNotFoundError")

# --- generate_tokens: layout ------------------------------------------------
SAMPLES = [
    "",
    "# only a comment",
    "x = 1",
    "x = 1\n",
    "\n\n",
    "x\n\n\n",
    "def f(a, b):\n    # sum\n    return a + b  # trailing\n",
    "if True:\n    x = 1\n    if False:\n        y = 2\nz = 3\n",
    "if True:\n    x = 1\n\n    # comment\n\n    y = 2\n",
    "class A:\n\tdef f(self):\n\t\treturn 1\n",
    "x = (1 +\n     2)\n",
    "x = [\n    1,\n    2,\n]\n",
    "d = {\n    'a': 1,\n\n    # c\n    'b': 2}\n",
    "x = 1 + \\\n    2\n",
    "x = 1 \\\n  + 2 \\\n  + 3\n",
    "a = 1\r\nb = 2\r\n",
    "if x:\n    pass\n\n\n  # c\ny\n",
    "def f():\n    pass\n# c\nx\n",
    "lambda: (yield)\n",
    "async def f(): await x\n",
    "x = 1;\n",
    "\x0cx = 1\n",
]
for sample in SAMPLES:
    print("SAMPLE", repr(sample))
    print("  ", toks(sample))
print(toks("def f(x):\n    return x\n", full=True))

# --- operators ----------------------------------------------------------------
ops = "== != <= >= << >> ** // += -= *= /= %= &= |= ^= <<= >>= **= //= @= @ -> := ... ~ ^ | & % . , : ; ( ) [ ] { } < > = + - * / <> $ ? `"
for t in tokenize.generate_tokens(io.StringIO(ops + "\n").readline):
    print(t.string, token.tok_name[t.type], token.tok_name[t.exact_type])

# --- numbers --------------------------------------------------------------------
for src in [
    "0xff + 0b1010 + 0o17 + 1_000 + 0xDEAD_beef\n",
    "1.5e-3j + .5 + 5. + 1e10 + 1E+5 + 0j + 1_0.0_1e1_0J\n",
    "x = 0_0 + 00 + 007 + 09 + 0e0\n",
    "x = 1a + 12abc + 1jj + 1j2 + 1e + 1e5e\n",
    "x = 0xfor 1if 2\n",
    "1.real; 1 .real; 1..real\n",
]:
    print(repr(src), toks(src))
for src in ["x = 1_\n", "x = 1__0\n", "x = 1_a\n", "x = 1.0_\n", "x = 1e+\n", "x = 0x\n",
            "x = 0xg\n", "x = 0x1__2\n", "x = 0b2\n", "x = 0b12\n", "x = 0o8\n", "x = 0o\n", "x = 0b\n"]:
    run(repr(src), lambda src=src: toks(src))

# --- strings ----------------------------------------------------------------------
for src in [
    "x = 'a' \"b\" '''c''' \"\"\"d\"\"\"\n",
    "x = r'\\d' + b'\\xff' + u'\\u1234' + R'x' + Rb'y' + bR'z' + rB'w' + BR'v'\n",
    "x = '''line1\nline2\n'''\ny = 2\n",
    'x = """a\n\n  b"""\n',
    "x = 'a\\\nb'\n",
    "x = 'it\\'s' + \"say \\\"hi\\\"\"\n",
    "x = '' + \"\" + '''''' + \"\"\"\"\"\"\n",
    "'''doc'''\n",
    "x = 'a' 'b'\n",
    "x = 'caf\u00e9' + '\u4e2d\u6587'\n",
]:
    print(repr(src), toks(src, full=True))

# --- f-strings ----------------------------------------------------------------------
for src in [
    'x = f"a={x!r:.2f}"\n',
    'f"a{{b}}{x!r:10}"\n',
    'f"{x=}"\n',
    'f"{x:>{w}}"\n',
    "f\"{f'{1}'}\"\n",
    'x = f"{a}" f"{b}" "c"\n',
    "x = f'''\n{a}\nmid\n{b!s}\n'''\n",
    'x = rf"\\d{x}" + fr"{y}\\n" + Rf"{z}" + F"{w}"\n',
    'f"{x:{y}.{z}}"\n',
    'f"{d[\'k\']} {a if b else c} {(lambda: 1)()}"\n',
    'f"{x:%Y-%m-%d}"\n',
    'f"{{}}"\n',
    'f""\n',
    'f"{x}" if y else f"{z}"\n',
]:
    print(repr(src), toks(src))

# --- non-ASCII and odd characters ------------------------------------------------------
for src in ["caf\u00e9 = 1\n", "x = \u20ac\n", "a\u20acb = 2\n", "\u4e2d\u6587 = 3\n", "x = $\n", "x = ?\n"]:
    print(repr(src), toks(src))

# --- errors -----------------------------------------------------------------------
ERROR_SOURCES = [
    "x = 'abc\n",
    "x = 'abc",
    'x = "abc\n',
    "x = b'abc\n",
    "x = rb'abc\n",
    "x = u'abc\n",
    "x = 'a\\\nb\n",
    "x = '\\\n",
    "'",
    "x = 'a\n'b'\n",
    "x = '''abc\n",
    "x = '''abc",
    'x = """abc\ndef\n',
    "'a'\n'''b\n",
    "if 1:\n    x = '''a\n",
    "x = r'''abc\n",
    "x = f'abc\n",
    "x = f'''abc\n",
    "x = f'''abc\ndef",
    "x = (1,\n",
    "x = (1,\n2,\n",
    "x = (1,\n2,",
    "x = [1,\n",
    "x = {1:\n",
    "(\n",
    "foo(1,\n  2\n",
    "if 1:\n    (a\n",
    "x = (\n\ty\n",
    "x = 1 \\\n",
    "x = 1 \\",
    "x = 1 + \\\n",
    "x\\\n",
    "  \\\n",
    "x = (\\\n",
    "x = 1 \\ \n",
    "x = 1 \\ y\n",
    "x = \\\\\n",
    "a\nb\x00\n",
    "a\x00",
    "x = \x01\n",
    "x\x7f\n",
    "x = 1 \x1b 2\n",
]
for src in ERROR_SOURCES:
    run(repr(src), lambda src=src: toks(src))

for src in [
    "if 1:\n    x\n  y\n",
    "if 1:\n    x\n   y\n",
    "if 1:\n        x\n    y\n",
    "if 1:\n    if 2:\n        x\n      y\n",
    "if 1:\n\tx\n  y\n",
    "if 1:\n\t\tx\n\ty\n",
    "  x\n y\n",
    "if 1:\n    x\n\n  y\n",
    "if 1:\n    x\n  # c\n  y\n",
    "if 1:\n    x\n\x0c  y\n",
    "if 1:\n\tx\n        y\n",
    "if 1:\n        x\n\ty\n",
    "if 1:\n    x\n\ty\n",
    "if 1:\n  x\n\ty\n",
    "if 1:\n \tx\n        y\n",
    "if 1:\n        x\n\t\ty\n",
]:
    run(repr(src), lambda src=src: toks(src))

# Tabs and spaces that are consistent do not raise.
for src in ["if 1:\n\tx\n\t y\n", "if 1:\n    x\n    \ty\n", "if 1:\n \tx\n \t y\n", "if 1:\n\x0c    x\n",
            "\tx\n", "   \n\tx\n", "if 1:\n\tx\nz\n"]:
    run(repr(src), lambda src=src: toks(src))

try:
    list(tokenize.generate_tokens(io.StringIO("x = (1,\n").readline))
except tokenize.TokenError as exc:
    print("TokenError:", exc.args, repr(exc))
try:
    list(tokenize.generate_tokens(io.StringIO("if 1:\n    x\n  y\n").readline))
except IndentationError as exc:
    print("IndentationError:", exc.args, exc.msg, exc.lineno, exc.offset, exc.filename, repr(exc.text))
try:
    list(tokenize.generate_tokens(io.StringIO("if 1:\n\tx\n        y\n").readline))
except TabError as exc:
    print("TabError:", exc.args, exc.lineno, exc.offset)

# Tokens before an error are still produced, then the error, then nothing.
gen = tokenize.generate_tokens(io.StringIO("x = 1\ny = 'abc\nz = 3\n").readline)
got = []
try:
    for t in gen:
        got.append(t.string)
except tokenize.TokenError as exc:
    got.append(("ERR", exc.args))
print("partial:", got, list(gen))

gen = tokenize.generate_tokens(io.StringIO("a = 1 $ 2 \x01\n").readline)
got = []
try:
    for t in gen:
        got.append(t.string)
except tokenize.TokenError as exc:
    got.append(("ERR", exc.args))
print("partial2:", got)

# --- tokenize() (bytes) ------------------------------------------------------------------
def btoks(data):
    return [(token.tok_name[t.type], t.string, t.start, t.end) for t in tokenize.tokenize(io.BytesIO(data).readline)]


print(btoks(b"x = 1\n"))
print(btoks(b"\xef\xbb\xbfx = 1\n"))
print(btoks(b"# coding: latin-1\nx = '\xe9'\ny = '\xe8'\n"))
print(btoks(b"# -*- coding: cp1252 -*-\nx = '\x80'\n"))
print(btoks(b""))
print(btoks(b"# coding: ascii\nx = 1\n")[0])
print(btoks(b"a = 1\nb = 2\nx = '\xff'\n")[-5:])
run("bytes-bad-first-line", lambda: btoks(b"x = '\xff'\n"))
run("bytes-unknown-enc", lambda: btoks(b"# coding: nope\n"))
run("str-to-tokenize", lambda: list(tokenize.tokenize(io.StringIO("x = 1\n").readline)))
run("bytes-to-generate", lambda: list(tokenize.generate_tokens(io.BytesIO(b"x = 1\n").readline)))
run("tokenize-noargs", lambda: tokenize.tokenize())
run("generate-noargs", lambda: tokenize.generate_tokens())


def lines_then(items):
    it = iter(items)
    return lambda: next(it)


run("later-str-in-bytes-mode", lambda: list(tokenize.tokenize(lines_then([b"a\n", b"b\n", "c\n", b""]))))
run("later-bytes-in-str-mode", lambda: [t.string for t in tokenize.generate_tokens(lines_then(["a\n", b"b\n"]))])
run("readline-int", lambda: list(tokenize.generate_tokens(lambda: 5)))
run("readline-none", lambda: list(tokenize.generate_tokens(lambda: None)))
print("readline-stopiteration:", [t.type for t in tokenize.generate_tokens(iter(["x = 1\n"]).__next__)])
print("readline-stopiteration-bytes:", [t.type for t in tokenize.tokenize(iter([b"x = 1\n"]).__next__)])


def boom():
    raise ZeroDivisionError("boom")


run("readline-raises", lambda: list(tokenize.generate_tokens(boom)))

# --- laziness ---------------------------------------------------------------------------
def tracking(lines):
    log = []
    it = iter(lines)

    def readline():
        log.append("call")
        return next(it, "")

    return readline, log


readline, log = tracking(["a = 1\n", "b = 2\n", "c = 3\n"])
gen = tokenize.generate_tokens(readline)
print("lazy-created:", len(log), type(gen).__name__)
print("lazy-first:", next(gen).string, len(log))
for _ in range(3):
    next(gen)
print("lazy-line1-done:", len(log))
print("lazy-line2:", next(gen).string, len(log))
rest = list(gen)
print("lazy-rest:", len(rest), len(log))
print("exhausted:", list(gen))
run("exhausted-next", lambda: next(gen))
print("iter-is-self:", iter(gen) is gen)

readline, log = tracking([b"a = 1\n", b"b = 2\n", b"c = 3\n"])
gen = tokenize.tokenize(readline)
print("lazy-bytes-created:", len(log))
first = next(gen)
print("lazy-bytes-first:", token.tok_name[first.type], first.string, len(log))
print("lazy-bytes-second:", next(gen).string, len(log))

readline, log = tracking([b"# c\n", b"# coding: latin-1\n", b"x = 1\n"])
gen = tokenize.tokenize(readline)
print("lazy-bytes-cookie:", next(gen).string, len(log))

gen = tokenize.generate_tokens(lambda: 1 / 0)
print("lazy-no-call-until-next:", type(gen).__name__)
run("lazy-first-next", lambda: next(gen))

# --- untokenize ------------------------------------------------------------------------------
ROUND_TRIP = [
    "def add(a, b):\n    # sum\n    return a + b\n",
    "x = (1 +\n     2)\n",
    "x   =   1   # spaced\n",
    "if True:\n    x = 1\n\n    y = 2\nz = 3\n",
    "x = 1 + \\\n    2\n",
    "class A:\n\tdef f(self):\n\t\treturn 1\n",
    's = f"val={{ {x!r} }}"\n',
    'x = f"a{{b}}{y!r:>{w}} \\N{DIGIT ONE}"\n',
    'x = f"{a}" f"{b}" "c" "d"\n',
    "f'{{{x}}}'\nf'{x:{{}}}'\n",
    "x = f'''\n{a}\nmid {b}\n'''\n",
    "a = 1\r\nb = 2\r\n",
    "x = 'a' 'b' \"c\"\n",
    "x = 1;\n",
    "",
    "# c",
    "x",
]
for src in ROUND_TRIP:
    tl = list(tokenize.generate_tokens(io.StringIO(src).readline))
    full = tokenize.untokenize(tl)
    compat = tokenize.untokenize([t[:2] for t in tl])
    print("untok", repr(src), full == src, repr(full), repr(compat))

bt = list(tokenize.tokenize(io.BytesIO(b"# coding: latin-1\nx = '\xe9'\n").readline))
print("untok-bytes:", repr(tokenize.untokenize(bt)), repr(tokenize.untokenize([t[:2] for t in bt])))
bt = list(tokenize.tokenize(io.BytesIO(b"\xef\xbb\xbfx = 1\n").readline))
print("untok-bytes-bom:", repr(tokenize.untokenize(bt)))
print("untok-2tuple:", repr(tokenize.untokenize([(token.NAME, "x"), (token.OP, "="), (token.NUMBER, "42")])))
print("untok-2tuple-list:", repr(tokenize.untokenize([[token.NAME, "x"], [token.OP, "+"], [token.NAME, "y"]])))
print("untok-gen:", repr(tokenize.untokenize(t for t in tokenize.generate_tokens(io.StringIO("a = b\n").readline))))
print("untok-empty:", repr(tokenize.untokenize([])))
run("untok-1tuple", lambda: tokenize.untokenize([(1,)]))
run("untok-3tuple", lambda: tokenize.untokenize([(1, "x", 3)]))
run("untok-order", lambda: tokenize.untokenize([(1, "x", (2, 0), (2, 1), ""), (1, "y", (1, 0), (1, 1), "")]))
run("untok-backslash", lambda: repr(tokenize.untokenize([(1, "x", (1, 0), (1, 1), ""), (1, "y", (3, 2), (3, 3), "")])))
run("untok-noiter", lambda: tokenize.untokenize(5))
run("untok-noargs", lambda: tokenize.untokenize())
run("untok-bad-token", lambda: tokenize.untokenize([(1, 5, (1, 0), (1, 1), "")]))
print("untok-stops-at-endmarker:", repr(tokenize.untokenize(
    [(token.NAME, "a", (1, 0), (1, 1), ""), (token.ENDMARKER, "", (2, 0), (2, 0), ""),
     (token.NAME, "b", (3, 0), (3, 1), "")])))

# Untokenizer class
ut = tokenize.Untokenizer()
print(vars(ut))
print(ut.escape_brackets("a{b}\\N{x}{{y}}"), ut.escape_brackets(""), ut.escape_brackets("}{"))
ut.add_whitespace((1, 3))
print(vars(ut))
run("ut-add-ws-before", lambda: ut.add_whitespace((1, 1)))
run("ut-noargs", lambda: tokenize.Untokenizer().add_whitespace())
toks5 = list(tokenize.generate_tokens(io.StringIO("def f(a,\n      b):\n    return a+b\n").readline))
ut = tokenize.Untokenizer()
print(repr(ut.untokenize(toks5[:5])), vars(ut))
ut = tokenize.Untokenizer()
ut.compat((tokenize.NAME, "x"), iter([(tokenize.NAME, "y")]))
print(vars(ut))


class Loud(tokenize.Untokenizer):
    def add_whitespace(self, start):
        self.tokens.append("<ws%s>" % (start,))
        self.prev_row, self.prev_col = start


loud = Loud()
print(repr(loud.untokenize(list(tokenize.generate_tokens(io.StringIO("a = b\n").readline)))))

# --- patched module globals are honoured ----------------------------------------------------------
saved = tokenize.tok_name
patched = dict(saved)
patched[1] = "IDENT"
tokenize.tok_name = patched
print(repr(tokenize.TokenInfo(1, "x", (1, 0), (1, 1), "x")))
tokenize.tok_name = saved
print(repr(tokenize.TokenInfo(1, "x", (1, 0), (1, 1), "x")))
