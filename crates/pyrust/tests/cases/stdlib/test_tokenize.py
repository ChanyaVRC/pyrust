import io
import os
import tempfile
import token
import tokenize


def dump_tokens(src):
    toks = list(tokenize.generate_tokens(io.StringIO(src).readline))
    return [
        (token.tok_name[t.type], token.tok_name[t.exact_type], t.string, t.start, t.end)
        for t in toks
    ]


# Basic expressions, operators, and exact_type
for sample in [
    "",
    "# comment only",
    "x = 1",
    "x = 1 + 2 * 3 ** 4 // 5 % 6\n",
    "a := (b != c) and (d <= e >= f) or (g << 1 >> 2)\n",
    "def f(x: int) -> list[int]:\n    return [x, ...]\n",
    "1 + \\\n 2\n",
    "(\n  1 +\n  2\n)\n",
    "0x1_a + 0b1010 + 0o77 + 3.14e-2 + 2.5j\n",
    '"""line1\nline2"""\n',
    'f"a{{b}}{x!r:10}"\n',
    'f"{x=}"\n',
    'f"{x:>{w}}"\n',
    'if True:\n    x = 1\n    if False:\n        y = 2',
]:
    print("SAMPLE:", repr(sample))
    print("  ", dump_tokens(sample))

# TokenInfo repr
first = next(tokenize.generate_tokens(io.StringIO("x = 1").readline))
print("repr:", repr(first))
print("exact_type_name:", first.exact_type == token.NAME)
eq_tok = list(tokenize.generate_tokens(io.StringIO("x == 1").readline))[1]
print("exact_type_eq:", eq_tok.type == token.OP, eq_tok.exact_type == token.EQEQUAL)

# Bytes tokenize() with ENCODING token
b_toks = list(tokenize.tokenize(io.BytesIO(b"# coding: utf-8\nx = 1\n").readline))
print("bytes_first:", token.tok_name[b_toks[0].type], b_toks[0].string, b_toks[0].start, b_toks[0].end)
print("bytes_count:", len(b_toks))

# detect_encoding
print("enc_default:", tokenize.detect_encoding(io.BytesIO(b"x = 1\n").readline))
print("enc_bom:", tokenize.detect_encoding(io.BytesIO(b"\xef\xbb\xbfx = 1\n").readline))
print("enc_cookie1:", tokenize.detect_encoding(io.BytesIO(b"# coding: latin-1\nx = 1\n").readline))
print("enc_cookie2:", tokenize.detect_encoding(io.BytesIO(b"#!/usr/bin/env python3\n# -*- coding: utf-8 -*-\nx = 1\n").readline))
print("enc_bom_utf8:", tokenize.detect_encoding(io.BytesIO(b"\xef\xbb\xbf# coding: utf-8\nx = 1\n").readline))

try:
    tokenize.detect_encoding(io.BytesIO(b"\xef\xbb\xbf# coding: latin-1\n").readline)
    print("bom_conflict: FAIL")
except SyntaxError:
    print("bom_conflict: SyntaxError")

try:
    tokenize.detect_encoding(io.BytesIO(b"# coding: nonexistent-codec-xyz\n").readline)
    print("bad_enc: FAIL")
except SyntaxError:
    print("bad_enc: SyntaxError")

# untokenize roundtrip (5-tuple and 2-tuple and bytes)
rt_src = "def add(a, b):\n    # sum\n    return a + b\n"
toks = list(tokenize.generate_tokens(io.StringIO(rt_src).readline))
print("untokenize_rt:", tokenize.untokenize(toks) == rt_src)

fstr_src = 's = f"val={{ {x!r} }}"\n'
ftoks = list(tokenize.generate_tokens(io.StringIO(fstr_src).readline))
print("untokenize_fstr:", tokenize.untokenize(ftoks) == fstr_src)

btoks = list(tokenize.tokenize(io.BytesIO(b"x = 1 + 2\n").readline))
print("untokenize_bytes:", tokenize.untokenize(btoks))

compat_pairs = [(token.NAME, "x"), (token.OP, "="), (token.NUMBER, "42")]
print("untokenize_2tuple:", tokenize.untokenize(compat_pairs).strip())

# TokenError on unclosed paren and unclosed triple-quoted string
try:
    list(tokenize.generate_tokens(io.StringIO("x = (1 + 2\n").readline))
    print("unclosed_paren: FAIL")
except tokenize.TokenError:
    print("unclosed_paren: TokenError")

try:
    list(tokenize.generate_tokens(io.StringIO('"""unclosed\n').readline))
    print("unclosed_str: FAIL")
except tokenize.TokenError:
    print("unclosed_str: TokenError")

try:
    list(tokenize.generate_tokens(io.StringIO("if True:\n    x = 1\n  y = 2\n").readline))
    print("bad_dedent: FAIL")
except IndentationError:
    print("bad_dedent: IndentationError")

# tokenize.open with encoding cookie
fd, path = tempfile.mkstemp(suffix=".py")
os.close(fd)
try:
    with open(path, "wb") as f:
        f.write(b"# coding: utf-8\nmsg = 'caf\xc3\xa9'\n")
    with tokenize.open(path) as tf:
        print("tokenize_open:", repr(tf.read()))
finally:
    os.unlink(path)
