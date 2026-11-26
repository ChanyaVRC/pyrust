# str methods on strings holding lone UTF-16 surrogates (U+D800..U+DFFF),
# e.g. text produced by bytes.decode(..., "surrogateescape") (issue #3315).
#
# pyrust stores a lone surrogate as its 3-byte CESU-8 sequence; any code path
# that decoded it into a Rust `char` hit undefined behaviour and aborted the
# whole process.  Every result is printed through ascii() so the output is
# plain ASCII (printing a surrogate directly raises in CPython).

import email
import io
import ntpath
import pathlib


def show(label, fn):
    try:
        result = fn()
    except Exception as exc:
        print(label, "->", type(exc).__name__, ascii(str(exc)))
    else:
        print(label, "->", ascii(result))


S = "\udce9"
T = "ab\udce9cd"
U = "  \ud800 x\udfff  "
PAIR = "\ud83d\ude00"
E = "\xe9"  # e-acute
EU = "\xc9"  # E-acute
SIGMA = chr(0x3A3)
IDEO_SPACE = chr(0x3000)
HANGUL = chr(0xD55C) + "\udce9" + chr(0xAE00)
W = "\udce9 " + IDEO_SPACE + "\udc80\t\udcff"

print("== reproducer (issue #3315)")
for name, fn in [
    ("strip", lambda s: s.strip()),
    ("lstrip", lambda s: s.lstrip()),
    ("rstrip", lambda s: s.rstrip()),
    ("split", lambda s: s.split()),
    ("rsplit", lambda s: s.rsplit()),
    ("splitlines", lambda s: s.splitlines()),
    ("casefold", lambda s: s.casefold()),
    ("translate", lambda s: s.translate({})),
    ("title", lambda s: s.title()),
    ("swapcase", lambda s: s.swapcase()),
    ("capitalize", lambda s: s.capitalize()),
]:
    show(name, lambda: fn(S))
show(
    "email get_payload(decode=True)",
    lambda: email.message_from_bytes(
        b"Content-Type: text/plain; charset=latin-1\n\n\xe9"
    ).get_payload(decode=True),
)

print("== strip family")
show("U.strip()", lambda: U.strip())
show("U.lstrip()", lambda: U.lstrip())
show("U.rstrip()", lambda: U.rstrip())
show("W.strip()", lambda: W.strip())
show("strip(None)", lambda: U.strip(None))
show("strip one ascii", lambda: S.strip("x"))
show("strip ascii set", lambda: T.strip("abcd"))
show("strip surrogate", lambda: S.strip("\udce9"))
show("strip mixed set", lambda: T.strip("ad\udce9cb"))
show("lstrip mixed", lambda: T.lstrip("ab\udce9"))
show("rstrip mixed", lambda: T.rstrip("dc\udce9"))
show("strip long ascii set", lambda: S.strip("abcdefghijklmnop"))
show("strip long set with surrogate", lambda: S.strip("abcdefghijk\udce9lmnop"))
show("strip long non-ascii set", lambda: T.strip("abcdefghijk" + E + "lmnop"))
show("strip repeated", lambda: "\udce9\udce9x\udce9".strip("\udce9"))
show("strip e/surrogate", lambda: ("\udce9" + E + "x" + E + "\udce9").strip(E + "\udce9"))
show("strip C0 separators", lambda: "\x1c a \x1f".strip())
show("strip empty chars", lambda: S.strip(""))

print("== split family")
show("S.split()", lambda: S.split())
show("T.split()", lambda: T.split())
show("U.split()", lambda: U.split())
show("U.rsplit()", lambda: U.rsplit())
show("W.split()", lambda: W.split())
show("W.rsplit()", lambda: W.rsplit())
show("W.split(None, 1)", lambda: W.split(None, 1))
show("W.rsplit(None, 1)", lambda: W.rsplit(None, 1))
show("U.split(None, 1)", lambda: U.split(None, 1))
show("U.rsplit(None, 1)", lambda: U.rsplit(None, 1))
show("ideographic rsplit", lambda: ("a" + IDEO_SPACE + "b" + IDEO_SPACE + "c").rsplit(None, 1))
show("C0 separators split", lambda: "a\x1cb\x1dc\x1ed\x1fe".split())
show("split maxsplit 0", lambda: " a b c ".split(None, 0))
show("rsplit maxsplit 0", lambda: " a b c ".rsplit(None, 0))
show("blank split maxsplit 0", lambda: "   ".split(None, 0))
show("empty rsplit", lambda: "".rsplit(None, 2))
show("split sep surrogate", lambda: T.split("\udce9"))
show("rsplit sep surrogate", lambda: T.rsplit("\udce9"))
show("split sep surrogate max", lambda: T.split("\udce9", 1))
show("rsplit sep surrogate max", lambda: T.rsplit("\udce9", 1))
show("split sep ascii", lambda: T.split("c"))
show("split multi-codepoint sep", lambda: T.split("b\udce9c"))
show("split repeated surrogates", lambda: "\udce9\udce9\udce9".split("\udce9"))
show("split empty sep", lambda: T.split(""))
show("splitlines", lambda: "a\udce9\nb\udce9\r\nc".splitlines())
show("splitlines keepends", lambda: "a\udce9\nb\udce9\r\nc".splitlines(True))

print("== case mapping")
for label, text in [
    ("S", S),
    ("T", T),
    ("HANGUL", HANGUL),
    ("PAIR", PAIR),
    ("mixed", E + "\udce9" + EU),
    ("words", "ab\udce9cd ef"),
    ("lead", "\udce9ABC"),
    ("lead accented", "\udce9" + EU + "BC"),
]:
    show(label + ".upper()", lambda: text.upper())
    show(label + ".lower()", lambda: text.lower())
    show(label + ".casefold()", lambda: text.casefold())
    show(label + ".title()", lambda: text.title())
    show(label + ".swapcase()", lambda: text.swapcase())
    show(label + ".capitalize()", lambda: text.capitalize())
show("final sigma before surrogate", lambda: ("A" + SIGMA + "\udce9").lower())
show("sigma after surrogate", lambda: ("\udce9" + SIGMA + "a").lower())
show("lone sigma then surrogate", lambda: (SIGMA + "\udce9").lower())
show("sigma between", lambda: ("A" + SIGMA + "\udce9B").lower())
show("sigma runs", lambda: (SIGMA + SIGMA + "\udce9" + SIGMA + SIGMA).lower())
show("dotted I", lambda: (chr(0x130) + "\udce9").lower())
show("dotted I casefold", lambda: (chr(0x130) + "\udce9").casefold())
show("sharp s upper", lambda: ("\xdf\udce9\xdf").upper())
show("sharp s title", lambda: ("\xdf\udce9\xdf").title())
show("sharp s capitalize", lambda: ("\xdf\udce9\xdf").capitalize())
show("sharp s casefold", lambda: ("\udce9\xdf").casefold())
show("digraph title", lambda: (chr(0x1C6) + "\udce9" + chr(0x1C6)).title())
show("digraph capitalize", lambda: (chr(0x1C6) + "\udce9").capitalize())
show("digraph after lead", lambda: ("\udce9" + chr(0x1C6)).capitalize())

print("== predicates")
for label, text in [("S", S), ("T", T), ("aSb", "a\udce9b"), ("1S", "1\udce9"), (" S", " \udce9")]:
    show(
        label + " is*",
        lambda: [
            text.isalpha(),
            text.isalnum(),
            text.isdigit(),
            text.isdecimal(),
            text.isnumeric(),
            text.isspace(),
            text.islower(),
            text.isupper(),
            text.istitle(),
            text.isprintable(),
            text.isascii(),
            text.isidentifier(),
        ],
    )
show("istitle A S B", lambda: "A\udce9B".istitle())
show("istitle Ab S Cd", lambda: "Ab\udce9Cd".istitle())
show("istitle S A", lambda: "\udce9A".istitle())

print("== searching")
show("find", lambda: T.find("\udce9"))
show("rfind", lambda: T.rfind("\udce9"))
show("index", lambda: T.index("\udce9"))
show("rindex", lambda: T.rindex("\udce9"))
show("count", lambda: T.count("\udce9"))
show("count empty", lambda: T.count(""))
show("find start", lambda: T.find("c", 1))
show("find window", lambda: T.find("c", 3, 5))
show("find negative", lambda: T.find("d", -2))
show("rfind window", lambda: T.rfind("c", 1, -1))
show("count window", lambda: T.count("c", 1, 4))
show("index start", lambda: T.index("d", 2))
show("find miss", lambda: T.find("x"))
show("index miss", lambda: T.index("x"))
show("find empty", lambda: S.find(""))
show("rfind empty", lambda: S.rfind(""))
show("index empty", lambda: S.index(""))
show("rindex empty", lambda: S.rindex(""))
show("find empty start", lambda: T.find("", 3))
show("rfind empty window", lambda: T.rfind("", 1, 4))
show("rfind empty start", lambda: T.rfind("", 3))
show("find empty past end", lambda: S.find("", 2))
show("count empty window", lambda: T.count("", 1, 3))
show("count overlapping", lambda: "\udce9\udce9\udce9".count("\udce9\udce9"))
show("rindex window", lambda: "x\udce9y".rindex("\udce9", 0, 2))
show("index negative", lambda: "x\udce9y".index("y", -1))
show("pair find", lambda: PAIR.find("\ude00"))
show("contains", lambda: ["\udce9" in T, "c" in T, "" in S, "\udc80" in T])

print("== prefix / suffix / partition")
show("startswith", lambda: T.startswith("ab\udce9"))
show("endswith", lambda: T.endswith("\udce9cd"))
show("startswith start", lambda: T.startswith("\udce9", 2))
show("startswith window", lambda: T.startswith("\udce9", 2, 3))
show("endswith window", lambda: T.endswith("\udce9", 0, 3))
show("endswith tuple", lambda: T.endswith(("x", "\udce9cd")))
show("startswith tuple start", lambda: "x\udce9y".startswith(("\udce9",), 1))
show("startswith after surrogate", lambda: T.startswith("c", 3))
show("endswith negative end", lambda: T.endswith("b", 0, -3))
show("endswith empty start", lambda: "x\udce9y".endswith("", 3))
show("removeprefix", lambda: T.removeprefix("ab\udce9"))
show("removesuffix", lambda: T.removesuffix("\udce9cd"))
show("removeprefix whole", lambda: S.removeprefix("\udce9"))
show("partition", lambda: T.partition("\udce9"))
show("rpartition", lambda: T.rpartition("\udce9"))
show("partition ascii", lambda: T.partition("c"))
show("partition miss", lambda: S.partition("x"))
show("partition empty", lambda: S.partition(""))

print("== replace")
show("replace surrogate", lambda: T.replace("\udce9", "X"))
show("replace with surrogate", lambda: T.replace("c", "\udc80"))
show("replace delete", lambda: T.replace("\udce9", ""))
show("replace empty", lambda: T.replace("", "-"))
show("replace empty count", lambda: T.replace("", "-", 2))
show("replace empty count 0", lambda: S.replace("", "-", 0))
show("replace empty count 1", lambda: S.replace("", "-", 1))
show("replace empty surrogate", lambda: S.replace("", "\udc80"))
show("replace empty on empty", lambda: "".replace("", "x"))
show("replace empty non-ascii", lambda: (E + E).replace("", "-"))
show("replace overlapping", lambda: "\udce9\udce9\udce9".replace("\udce9\udce9", "x"))
show("pair replace empty", lambda: PAIR.replace("", "|"))

print("== layout")
show("center", lambda: S.center(5))
show("center fill", lambda: S.center(6, "*"))
show("center surrogate fill", lambda: S.center(4, "\udc80"))
show("center fill on ascii", lambda: "x".center(3, "\udce9"))
show("center bad fill", lambda: "x".center(3, "ab"))
show("hangul center", lambda: HANGUL.center(7, "\udc80"))
show("ljust", lambda: T.ljust(8, E))
show("ljust surrogate fill", lambda: S.ljust(3, "\udc80"))
show("rjust", lambda: T.rjust(8))
show("rjust accented fill", lambda: S.rjust(3, E))
show("center narrow", lambda: S.center(0))
show("zfill", lambda: S.zfill(4))
show("zfill sign", lambda: "-\udce9".zfill(4))
show("zfill plus", lambda: "+\udce9".zfill(5))
show("zfill trailing sign", lambda: "\udce9-".zfill(4))
show("zfill narrow", lambda: T.zfill(2))
show("zfill wide", lambda: T.zfill(10))
show("zfill empty", lambda: "".zfill(3))
show("expandtabs", lambda: "\udce9\tx\ty".expandtabs())
show("expandtabs 4", lambda: "\udce9\tx\ty".expandtabs(4))
show("expandtabs reset", lambda: "\udce9\r\tx".expandtabs(4))
show("expandtabs mixed", lambda: (E + "\t\udce9\t\n\tx").expandtabs(3))
show("expandtabs 0", lambda: "\udce9\tx".expandtabs(0))

print("== join")
show("join list", lambda: "-".join([S, T]))
show("surrogate sep", lambda: S.join(["a", "b", "c"]))
show("surrogate sep str", lambda: S.join("xyz"))
show("join str iterable", lambda: "-".join(T))
show("join str iterable surrogate sep", lambda: "\udc80".join(T))
show("join str iterable empty sep", lambda: "".join(T))
show("join mixed", lambda: "\udce9".join([E, "\udc80"]))

print("== translate / maketrans")
show("translate ascii", lambda: T.translate({ord("a"): "x"}))
show("translate surrogate key", lambda: T.translate({0xDCE9: "Z"}))
show("translate delete", lambda: T.translate({0xDCE9: None}))
show("translate to surrogate int", lambda: T.translate({0xDCE9: 0xDC80}))
show("translate ascii to surrogate int", lambda: T.translate({ord("a"): 0xDC80}))
show("translate ascii to surrogate str", lambda: T.translate({ord("a"): "\udc80"}))
show("translate bool", lambda: T.translate({0xDCE9: True}))
show("translate out of range", lambda: T.translate({0xDCE9: 0x110000}))
show("translate accented", lambda: (E + "\udce9").translate({0xE9: "E"}))
show("maketrans 2-arg", lambda: str.maketrans("\udce9", "x"))
show("maketrans dict str key", lambda: str.maketrans({"\udce9": "x"}))
show("maketrans dict long key", lambda: str.maketrans({"\udce9\udce9": 1}))
show("maketrans 3-arg", lambda: str.maketrans("a\udce9", "\udc80b", "\udcff"))
show("maketrans unequal", lambda: str.maketrans("ab", "\udce9"))
show("maketrans translate", lambda: T.translate(str.maketrans("a\udce9", "\udc80b", "c")))


class SurrogateTable:
    def __getitem__(self, key):
        if key == 0xDCE9:
            return "Z"
        if key == ord("a"):
            return 0xDC80
        raise LookupError(key)


show("translate mapping", lambda: T.translate(SurrogateTable()))

print("== format")
show("str.format", lambda: "{}".format(S))
show("format width", lambda: "{:>5}".format(S))
show("format center", lambda: "{:*^7}".format(T))
show("format precision 2", lambda: "{:.2}".format(T))
show("format precision 3", lambda: "{:.3}".format(T))
show("format width precision", lambda: "{:>8.3}".format(T))
show("format pair precision", lambda: "{:.1}".format(PAIR))
show("format pair center", lambda: "{:^6}".format(PAIR))
show("format !r", lambda: "{!r}".format(S))
show("format !a", lambda: "{!a}".format(S))
show("format !s", lambda: "{0!s:^9}".format(T))
show("format positional reuse", lambda: "{0}{0}".format(S))
show("format template surrogate", lambda: "\udce9{}".format(1))
show("format surrogate fill str", lambda: "{:\udce9^5}".format("a"))
show("format surrogate fill int", lambda: "{:\udce9>4}".format(12))
show("format surrogate fill float", lambda: "{:\udce9<6.2f}".format(1.5))
show("format surrogate fill =", lambda: "{:\udce9=+6}".format(5))
show("format() surrogate fill", lambda: format("a", "\udce9^5"))
show("format() surrogate type str", lambda: format("a", "\udce9"))
show("format() surrogate type int", lambda: format(5, "\udce9"))
show("format() trailing surrogate", lambda: format("a", "\udce95"))
show("format() grouping surrogate", lambda: format(5, ",\udce9"))
show("format() width surrogate type", lambda: format("ab", "5\udce9"))
show("format() c surrogate", lambda: format(0xDCE9, "c"))
show("format() c surrogate centered", lambda: format(0xDCE9, "^3c"))
show("format() width", lambda: format(S, "<4"))
show("format() precision", lambda: format(T, ".1"))
show("__format__", lambda: S.__format__(">3"))
show("f-string width", lambda: f"{S:5}|")
show("f-string repr", lambda: f"{S!r}")
show("f-string ascii", lambda: f"{T!a}")
show("f-string precision", lambda: f"{T:.3}")
show("f-string surrogate fill", lambda: f"{T:\udc80>8}")

print("== printf")
show("%s", lambda: "%s|" % S)
show("%5s", lambda: "%5s|" % S)
show("%-5s", lambda: "%-5s|" % S)
show("%.2s", lambda: "%.2s|" % T)
show("%.3s", lambda: "%.3s|" % T)
show("%-6.4s", lambda: "%-6.4s|" % T)
show("%5s pair", lambda: "%5s|" % PAIR)
show("%r", lambda: "%r" % S)
show("%a", lambda: "%a" % S)
show("%r list", lambda: "%r" % [S])
show("%(key)s", lambda: "%(a)s" % {"a": S})
show("template surrogate", lambda: "%s\udce9" % "a")
show("%c int", lambda: "%c" % 0xDCE9)
show("%c str", lambda: "%c" % S)
show("%3c", lambda: "%3c|" % S)
show("%-4c", lambda: "%-4c|" % S)
show("%c too long", lambda: "%c" % T)
show("%c negative", lambda: "%c" % (65 - 2**32))
show("%c out of range", lambda: "%c" % 0x110000)

print("== encode")
for args in [
    ("utf-8", "surrogateescape"),
    ("utf-8", "surrogatepass"),
    ("utf-8",),
    ("utf-8", "strict"),
    ("ascii", "surrogateescape"),
    ("latin-1", "surrogateescape"),
    ("cp1252", "surrogateescape"),
    ("utf-8", "replace"),
    ("utf-8", "ignore"),
    ("utf-8", "backslashreplace"),
    ("utf-8", "xmlcharrefreplace"),
    ("utf-8", "namereplace"),
    ("utf-16", "surrogatepass"),
    ("utf-16-le", "surrogatepass"),
    ("utf-32", "surrogatepass"),
    ("ascii",),
    ("unicode_escape",),
    ("raw_unicode_escape",),
]:
    show("T.encode" + ascii(args), lambda: T.encode(*args))
show("high surrogate escape", lambda: "\ud800".encode("utf-8", "surrogateescape"))
show("high surrogate pass", lambda: "\ud800".encode("utf-8", "surrogatepass"))
show("latin-1 strict", lambda: "a\ud800".encode("latin-1"))
show("cp1252 replace", lambda: "a\ud800".encode("cp1252", "replace"))
show("cp1252 escape run", lambda: "\udcff\udc81x".encode("cp1252", "surrogateescape"))
show("cp1252 mixed run", lambda: "\udcff\x81".encode("cp1252", "surrogateescape"))
show("cp1252 surrogatepass", lambda: "\udc80".encode("cp1252", "surrogatepass"))
show("utf-16 strict", lambda: "a\ud800".encode("utf-16"))
show("pair utf-16 pass", lambda: PAIR.encode("utf-16", "surrogatepass"))
show("pair utf-8 pass", lambda: PAIR.encode("utf-8", "surrogatepass"))
show(
    "surrogateescape round trip",
    lambda: b"caf\xe9 au lait".decode("utf-8", "surrogateescape").encode("utf-8", "surrogateescape"),
)

print("== indexing / slicing / iteration")
show("T[2]", lambda: T[2])
show("T[-1]", lambda: T[-1])
show("T[1:3]", lambda: T[1:3])
show("T[2:]", lambda: T[2:])
show("T[::-1]", lambda: T[::-1])
show("T[::2]", lambda: T[::2])
show("T[1::2]", lambda: T[1::2])
show("T[::-2]", lambda: T[::-2])
show("T[4:0:-1]", lambda: T[4:0:-1])
show("PAIR[::-1]", lambda: PAIR[::-1])
show("astral reversed", lambda: (chr(0x1F600) + "\udce9" + E)[::-1])
show("PAIR[0], PAIR[1]", lambda: (PAIR[0], PAIR[1]))
show("list", lambda: list(T))
show("reversed", lambda: list(reversed(T)))
show("tuple", lambda: tuple(PAIR))
show("ord map", lambda: list(map(ord, T)))
show("len", lambda: (len(S), len(T), len(PAIR), len(HANGUL)))
show("ord/chr", lambda: (ord(S), chr(0xDCE9) == S))
show("compare", lambda: (S < chr(0x10000), S > chr(0xFFFF), S > chr(0xD7FF)))
show("sorted", lambda: sorted(["\udce9", chr(0xFFFF), chr(0x10000), "a", "\ud800"]))
show("equality / hash", lambda: (T == "ab\udce9cd", hash(S) == hash("\udce9")))
show("max/min", lambda: (max(T), min(T)))
show("concat / repeat", lambda: (S + T, S * 3))
show("repr", lambda: repr(T))
show("ascii", lambda: ascii(T))
show("str of list", lambda: str([S, T]))


class SurrogateRepr:
    def __repr__(self):
        return "x\udce9"


show("ascii() of custom repr", lambda: ascii(SurrogateRepr()))

print("== numeric constructors")
show("int", lambda: int(S))
show("int mixed", lambda: int("1\udce9"))
show("int base 16", lambda: int(S, 16))
show("int base 0", lambda: int(" \udce9 ", 0))
show("int long", lambda: int("\udce9" + "1" * 300))
show("float", lambda: float(S))
show("float mixed", lambda: float("1.5\udce9"))
show("float padded", lambda: float(" \udce9"))
show("complex", lambda: complex(S))
show("complex parenthesised", lambda: complex("(1+\udce9j)"))

print("== containers / io / paths")
show("dict from 2-codepoint strs", lambda: dict([S + "x", "\udce9\udc80"]))
show("dict bad length", lambda: dict([T]))
show("StringIO read", lambda: io.StringIO(T).read())
show("StringIO read(3)", lambda: io.StringIO(T).read(3))
show("StringIO readlines", lambda: io.StringIO(T + "\n" + S).readlines())
show("StringIO iter", lambda: list(io.StringIO(T + "\nz")))
show("StringIO readline", lambda: io.StringIO("\udce9\n\udc80").readline())
show("StringIO readline(1)", lambda: io.StringIO("\udce9\n\udc80").readline(1))


def stringio_write():
    f = io.StringIO(T)
    f.seek(2)
    f.write("\udc80")
    return f.getvalue(), f.tell()


def stringio_truncate():
    f = io.StringIO(T)
    f.truncate(3)
    return f.getvalue()


def stringio_tell():
    f = io.StringIO(T)
    return f.read(2), f.tell(), f.read(), f.tell()


show("StringIO write", stringio_write)
show("StringIO truncate", stringio_truncate)
show("StringIO tell", stringio_tell)
show("ntpath.splitroot drive", lambda: ntpath.splitroot("C:/a/" + T))
show("ntpath.splitroot unc", lambda: ntpath.splitroot("//srv/share/" + T))
show("ntpath.splitroot device", lambda: ntpath.splitroot("//?/UNC/srv/share/x"))
show("ntpath.splitroot surrogate drive", lambda: ntpath.splitroot("\udce9:x"))
show("ntpath.splitroot accented drive", lambda: ntpath.splitroot(E + ":/x"))
show("ntpath.splitdrive", lambda: ntpath.splitdrive("//?/UNC/" + T + "/s/x"))
show("PureWindowsPath parts", lambda: pathlib.PureWindowsPath("C:/a/" + T).parts)
show("PureWindowsPath unc", lambda: pathlib.PureWindowsPath("//srv/share/" + T).parts)
show("PureWindowsPath relative", lambda: pathlib.PureWindowsPath(T).parts)

print("== surrogateescape text")
decoded = b"caf\xe9 au lait".decode("utf-8", "surrogateescape")
show("decoded", lambda: decoded)
show("decoded strip", lambda: b"\xe9".decode("ascii", "surrogateescape").strip())
show("decoded split", lambda: decoded.split())
show("decoded title", lambda: decoded.title())
show("decoded upper", lambda: b"\xe9\xe8".decode("utf-8", "surrogateescape").upper())

print("== email")
latin1 = b"Content-Type: text/plain; charset=latin-1\n\n\xe9"
show("decode=True", lambda: email.message_from_bytes(latin1).get_payload(decode=True))
show("charset", lambda: email.message_from_bytes(latin1).get_content_charset())
show("as_bytes", lambda: email.message_from_bytes(latin1).as_bytes())
show(
    "8bit utf-8 decode=True",
    lambda: email.message_from_bytes(
        b"Content-Type: text/plain; charset=utf-8\n"
        b"Content-Transfer-Encoding: 8bit\n\n\xc3\xa9\xe9\n"
    ).get_payload(decode=True),
)
show(
    "get_payload()",
    lambda: email.message_from_bytes(
        b"Subject: caf\xe9\nContent-Type: text/plain; charset=latin-1\n\nna\xefve caf\xe9\n"
    ).get_payload(),
)
show(
    "header str",
    lambda: str(
        email.message_from_bytes(b"Subject: caf\xe9\n\nbody\n")["Subject"]
    ),
)
