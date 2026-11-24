"""CPython 3.12 parity tests for re hardening (issue #3155)."""

import re

# ── 1. bytes and bytearray support ────────────────────────────────────────────
m = re.search(b"(?P<word>[a-z]+)-(\\d+)", b"pre abc-123 post")
print("bytes search:", m.group(0), m.group(1), m.group("word"), m.groups(), m.groupdict())
print("bytes groupindex:", m.re.groupindex)

# bytearray subject returns bytes slices
ba = bytearray(b"hello 42 world 99")
m_ba = re.search(b"(\\d+)", ba)
print("ba match:", type(m_ba.group(0)).__name__, m_ba.group(0), m_ba.span())
print("ba findall:", re.findall(b"\\d+", ba))
print("ba findall groups:", re.findall(b"(a)?(b)", bytearray(b"b ab")))
print("ba sub:", type(re.sub(b"\\d+", b"X", ba)).__name__, re.sub(b"\\d+", b"X", ba))
print("ba subn:", re.subn(b"\\d+", lambda m: b"[" + m.group(0) + b"]", ba))
print("ba split:", re.split(b"\\s+", ba))
print("ba finditer:", [x.group(0) for x in re.finditer(b"\\d+", ba)])
print("bytes fullmatch:", re.fullmatch(b"abc", b"abc").group(0), re.fullmatch(b"abc", b"abcd"))
print("bytes escape:", re.escape(b"a.b[c]*"))

# IGNORECASE on bytes only folds ASCII, not non-ASCII Latin-1 letters
print("bytes ic ascii:", bool(re.match(b"abc", b"ABC", re.I)))
print("bytes ic non-ascii:", bool(re.match(b"\xe9", b"\xc9", re.I)))

# ASCII word/digit/space semantics on bytes
print("bytes \\w non-ascii:", re.findall(b"\\w+", b"ab\xe9cd"))
print("bytes \\b:", re.findall(b"\\b\\w+\\b", b"foo bar_1 baz"))

# Mixing str and bytes raises TypeError
for label, fn in [
    ("str-pat-bytes-sub", lambda: re.search("a", b"a")),
    ("str-pat-ba-sub", lambda: re.match("a", bytearray(b"a"))),
    ("bytes-pat-str-sub", lambda: re.search(b"a", "a")),
    ("bytes-sub-str-repl", lambda: re.sub(b"a", "x", b"a")),
    ("str-sub-bytes-repl", lambda: re.sub("a", b"x", "a")),
]:
    try:
        fn()
        print(label, "NO ERROR")
    except TypeError:
        print(label, "TypeError")

# UNICODE flag incompatible with bytes pattern
try:
    re.compile(b"a", re.UNICODE)
    print("bytes-unicode NO ERROR")
except ValueError:
    print("bytes-unicode ValueError")


# ── 2. Match.expand() ─────────────────────────────────────────────────────────
m = re.match(r"(?P<first>\w+) (\w+)(?: (\d+))?", "hello world")
print("expand basic:", m.expand(r"\2, \1!"))
print("expand named/num:", m.expand(r"\g<first>-\g<2>-\g<0>"))
print("expand optional:", repr(m.expand(r"[\3]")))
print("expand escapes:", repr(m.expand(r"\n\t\r\f\v\a\b\\")))
print("expand octal:", repr(m.expand(r"\101\0")))

bm = re.match(b"(?P<k>[a-z]+)=(\\d+)", b"age=30")
print("bytes expand:", bm.expand(b"\\g<k>:\\2:\\1\\n"))

try:
    m.expand(b"\\1")
except TypeError:
    print("expand mix str/bytes TypeError")

try:
    bm.expand("\\1")
except TypeError:
    print("expand mix bytes/str TypeError")

try:
    m.expand(r"\9")
except re.error:
    print("expand bad numeric group re.error")

try:
    m.expand(r"\g<9>")
except re.error:
    print("expand bad g-num re.error")

try:
    m.expand(r"\g<nonexistent>")
except IndexError:
    print("expand bad g-name IndexError")

try:
    m.expand(r"\q")
except re.error:
    print("expand bad escape re.error")


# ── 3. \xHH, \uXXXX, \UXXXXXXXX escapes ──────────────────────────────────────
print("hex str:", re.findall(r"\x41\x42", "AB CD AB"))
print("hex class range:", re.findall(r"[\x41-\x43]+", "ZABCY"))
print("unicode4:", re.findall(r"\u00e9\u03b1", "x\u00e9\u03b1y"))
print("unicode4 class:", re.findall(r"[\u03b1-\u03b3]+", "a\u03b1\u03b2\u03b3z"))
print("unicode8:", re.findall(r"\U0001f600", "hi \U0001f600!"))
print("hex bytes:", re.findall(b"\\x41\\x42", b"AB CD AB"))
print("hex bytes class:", re.findall(b"[\\x00-\\x0f]+", b"\x01\x0a\xff\x05"))

for label, pat in [
    ("bytes-u4", b"\\u0041"),
    ("bytes-u8", b"\\U00000041"),
    ("short-hex", r"\x4"),
    ("short-u4", r"\u004"),
    ("short-u8", r"\U0001f60"),
]:
    try:
        re.compile(pat)
        print(label, "NO ERROR")
    except re.error:
        print(label, "re.error")


# ── 4. Possessive quantifiers & atomic groups (Python 3.11+) ─────────────────
# Possessive quantifiers do not backtrack
print("a++a on aaa:", re.match(r"a++a", "aaa"))
print("a+a on aaa:", re.match(r"a+a", "aaa").group(0))
print("a*+a on aaa:", re.match(r"a*+a", "aaa"))
print("a?+a on a:", re.match(r"a?+a", "a"))
print("a?+b on ab:", re.match(r"a?+b", "ab").group(0))
print("a?+b on b:", re.match(r"a?+b", "b").group(0))
print("a{1,3}+a on aaa:", re.match(r"a{1,3}+a", "aaa"))
print("a{1,2}+a on aaa:", re.match(r"a{1,2}+a", "aaa").group(0))
print("a++b on aaab:", re.match(r"a++b", "aaab").group(0))
print("(a*)++ on aa:", re.match(r"(a*)++", "aa").groups())

# Atomic groups (?>...)
print("atomic a|ab + c:", re.match(r"(?>a|ab)c", "abc"))
print("atomic ab|a + c:", re.match(r"(?>ab|a)c", "abc").group(0))
print("atomic a+ + a:", re.match(r"(?>a+)a", "aaa"))
print("atomic a+ + b:", re.match(r"(?>a+)b", "aaab").group(0))

# Capture rollback on failed atomic group
m_rb = re.match(r"(?>(a+))ab|(a+)b", "aab")
print("atomic rollback:", m_rb.group(0), m_rb.groups())

# Fixed-width atomic group inside lookbehind
print("lookbehind atomic:", re.search(r"(?<=(?>ab))c", "abc").group(0))

# Multiple repeat errors
for label, pat in [("star-pos-q", r"a*+?"), ("plus-pos-q", r"a++?"), ("q-pos-p", r"a?++")]:
    try:
        re.compile(pat)
        print(label, "NO ERROR")
    except re.error:
        print(label, "re.error")

print("re hardening ok")
