# CPython 3.12 parity tests for the native `html` package (issue #3446).
# `escape` / `unescape` edge cases: argument handling, str subclasses,
# non-str inputs, numeric reference ranges, legacy / multi-codepoint entities.

import html
import html.entities
import html.parser
import sys
from html import escape, unescape


def run(fn, *args, **kwargs):
    try:
        r = fn(*args, **kwargs)
        return (type(r).__name__, ascii(r))
    except BaseException as e:
        return (type(e).__name__, str(e)[:100])


class S(str):
    pass


class RS(str):
    def replace(self, old, new, count=-1):
        return "R(" + old + ")"


class Truthy:
    def __init__(self, v):
        self.v = v

    def __bool__(self):
        return self.v


# 1. Module metadata
print("meta:", html.__name__, html.__package__, html.__all__, ascii(html.__doc__))
print("sys.modules:", sys.modules["html"] is html, sys.modules["html.entities"] is html.entities)
print("submodules:", html.entities is sys.modules["html.entities"], sys.modules["html.parser"] is html.parser)
print("entities:", html.entities.html5 is html._html5, html.entities.__all__)
print("private:", sorted(n for n in ("_invalid_charrefs", "_invalid_codepoints", "_replace_charref", "_charref", "_re", "_html5") if hasattr(html, n)))
print("invalid_charrefs:", len(html._invalid_charrefs), html._invalid_charrefs[0], ascii(html._invalid_charrefs[0x80]), ascii(html._invalid_charrefs[0x81]), ascii(html._invalid_charrefs[0x0D]))
print("invalid_codepoints:", len(html._invalid_codepoints), sorted(html._invalid_codepoints)[:12], 0x10FFFF in html._invalid_codepoints)
print("charref:", html._charref.pattern)
print("from-import:", escape is html.escape, unescape is html.unescape)

# 2. escape
for s in ["", "plain", "&", "<>", '"', "'", "a&b<c>d\"e'f", "&amp;&lt;&gt;&quot;&#x27;", "日本語<&>", "é'\"", "&&&&", "x" * 1000 + "&"]:
    print("escape:", ascii(s[:40]), ascii(escape(s)[:60]), ascii(escape(s, False)[:60]), ascii(escape(s, quote=0)[:60]))
print("escape_quote_vals:", [escape("'\"<", q) for q in (True, False, 0, 1, None, "", "x", [], [0], Truthy(True), Truthy(False))])
print("escape_identity:", (lambda t: escape(t) is t)("nothing here"), (lambda t: escape(t, False) is t)("it's"))
print("escape_kwargs:", escape(s="<'>"), escape("<'>", quote=False), escape(s="<'>", quote=False))

# 3. escape: argument and type errors
print("escape_err:", run(escape), run(escape, "a", 1, 2), run(escape, "a", foo=1), run(escape, "a", s="b"), run(escape, quote=1))
print("escape_nonstr:", run(escape, 5), run(escape, None), run(escape, b"x"), run(escape, ["<"]))
print("escape_subclass:", run(escape, S("<a&b>")), run(escape, S("plain")), run(escape, RS("x")), run(escape, RS("x"), False))

# 4. unescape: fast paths and identity
for s in ["", "abc", "no entities", "a b c", "&", "&&", "a&", "&;", "& amp;", "&\t", "&\n", "&\x0c", "&<", "&#", "&#x", "&#;", "&#x;", "&#xg;", "&#a;"]:
    r = unescape(s)
    print("fast:", ascii(s), ascii(r), r is s)

# 5. unescape: named references
for s in [
    "&amp;", "&amp", "&lt;", "&lt", "&gt;", "&gt", "&quot;", "&quot", "&apos;", "&apos", "&AMP;", "&AMP",
    "&copy;", "&copy", "&COPY", "&nbsp", "&nbsp;", "&notit;", "&notin;", "&notin", "&not", "&not;",
    "&ampfoo", "&amp;foo", "&ampamp;", "&amp&amp", "&lt&gt", "&ltgt;", "&notanentity;", "&notanentity",
    "&foo", "&foo;", "&123;", "&1", "&é;", "&é", "&aelig;", "&aeligx", "&AElig", "&AEligfoo;",
    "&NotEqualTilde;", "&acE;", "&ThickSpace;", "&bne;", "&nvlt;", "&fjlig;", "&ngE;", "&NotSquareSubset;",
    "&CounterClockwiseContourIntegral;", "&CounterClockwiseContourIntegral", "&DoubleLongLeftRightArrow;",
    "&" + "a" * 32 + ";", "&" + "a" * 33 + ";", "&" + "a" * 40, "&" + "é" * 32 + ";", "&" + "é" * 33 + ";",
    "&euro;&euro&Euro;&EURO;", "&Aacute;&Aacute&aacute&aacutex;", "&times&timesb&timesbar;&timesb;",
    "&para&paraxyz;&parallel;&paralle;", "&sect&sect;&sectx", "&yen&yenn", "&sup2&sup3;&sup1x", "&frac12&frac34;&frac14x",
    "a&amp;b&amp;c", "&amp;&amp;&amp;", "&&amp;", "&amp;&", "&;amp;", "&amp ;", "&amp;;", "&lt;&#x3c;&#60;",
    "&#x26;amp;", "&amp;#x26;", "&amp;amp;", "&alpha;&beta;&gamma;&Omega;", "&hearts;&spades;&clubs;&diams;",
    "tab&amp\tsep", "nl&amp\nsep", "ff&amp\x0csep", "sp&amp sep", "lt&amp<sep", "amp&amp&sep", "hash&amp#sep",
]:
    print("named:", ascii(s), ascii(unescape(s)))

# 6. unescape: numeric references
for s in [
    "&#65;", "&#65", "&#065;", "&#00065;", "&#0000000000000000000065;", "&#x41;", "&#X41;", "&#x41", "&#X41", "&#x0041;",
    "&#x000000000000000041;", "&#xAbC;", "&#x1F600;", "&#128512;", "&#128512", "&#9731;&#9731", "&#65;&#66;&#67;",
    "&#65 ", "&#65x", "&#65;;", "&#x41g", "&#x41;;", "&#0x41;", "&#xx41;", "&#X;", "&#12a;", "&#1;2;", "&#a1;",
    "&#9;", "&#10;", "&#12;", "&#13;", "&#32;", "&#127;", "&#x7f;", "&#x1f;", "&#x1e;", "&#x0e;", "&#x08;", "&#x07;",
    "&#x0b;", "&#11;", "&#x0c;", "&#x0a;", "&#x09;",
    "&#0;", "&#00;", "&#x0;", "&#x00;", "&#0", "&#x0", "&#x0d;", "&#13", "&#xd;", "&#X0D;",
    "&#xD7FF;", "&#xD800;", "&#xd800", "&#55295;", "&#55296;", "&#57343;", "&#57344;", "&#xDFFF;", "&#xE000;",
    "&#xfdcf;", "&#xfdd0;", "&#xfdef;", "&#xfdf0;", "&#64976;", "&#65007;", "&#65008;",
    "&#xfffd;", "&#xfffe;", "&#xffff;", "&#x10000;", "&#x1fffe;", "&#x1ffff;", "&#x2fffe;", "&#x10fffd;", "&#x10fffe;",
    "&#x10ffff;", "&#x110000;", "&#1114111;", "&#1114112;", "&#xffffffff;", "&#x100000000;", "&#4294967295;", "&#4294967296;",
    "&#99999999;", "&#99999999999999999999999;", "&#x99999999999999999999999;", "&#18446744073709551616;",
    "&#18446744073709551617;", "&#x10000000000000000;", "&#" + "9" * 100 + ";", "&#x" + "f" * 100 + ";",
    "&#" + "0" * 100 + "65;", "&#x" + "0" * 100 + "41;", "&#" + "0" * 700 + "65;", "&#" + "1" * 700 + ";",
    "&#" + "1" * 4300 + ";", "&#x" + "f" * 5000 + ";", "&#x" + "0" * 5000 + "41;",
]:
    print("numeric:", ascii(s[:30]), len(s), ascii(unescape(s)[:30]))
for n in range(0x7E, 0xA2):
    ref = "&#%d;" % n
    print("c1:", ref, ascii(unescape(ref)), ascii(unescape("&#x%x" % n)))
print("c1_all:", ascii("".join(unescape("&#%d;" % n) for n in range(0x80, 0xA0))))
print("c1_hex_upper:", ascii(unescape("&#X80;&#X9F;&#X9f;")))

# 7. unescape: sweep of every code point class, checked against the spec rules
bad = 0
for cp in list(range(0, 0x120)) + [0xD7FF, 0xD800, 0xDBFF, 0xDC00, 0xDFFF, 0xE000, 0xFDCF, 0xFDD0, 0xFDEF, 0xFDF0, 0xFFFD, 0xFFFE, 0xFFFF, 0x10000, 0x1FFFE, 0x10FFFD, 0x10FFFE, 0x10FFFF, 0x110000, 0x7FFFFFFF]:
    d = unescape("&#%d;" % cp)
    h = unescape("&#x%X;" % cp)
    l = unescape("&#x%x" % cp)
    if not (d == h == l):
        bad += 1
    print("sweep:", cp, ascii(d))
print("sweep_consistent:", bad)

# 8. unescape: mixed text, ordering, and repeated passes
print("mixed:", ascii(unescape("<a href=\"x?a=1&amp;b=2&copy=3\">&lt;&#60;&#x3C;&#X3c;&gt;</a> & &amp &#38 &#x26")))
print("mixed2:", ascii(unescape("&euro;&#8364;&#x20AC;€&euro")), ascii(unescape("日本&amp;語&#26085;&#x672C;")))
print("mixed3:", ascii(unescape("&amp;lt;")), ascii(unescape(unescape("&amp;lt;"))), ascii(unescape("&amp;amp;amp;")))
print("long:", len(unescape("&amp;" * 5000)), len(unescape("a&lt;" * 3000 + "&")), len(unescape("&" * 5000)))
print("surrogates_in:", ascii(unescape("\ud800&amp;\udc00")), ascii(unescape("&\ud800;")), ascii(unescape("&amp\udfff")))
print("astral_in:", ascii(unescape("\U0001f600&amp;\U0001f600")), ascii(unescape("&\U0001f600;")), ascii(unescape("&amp\U0001f600")), ascii(unescape("&not\U0001f600")))
print("roundtrip:", all(unescape(escape(s)) == s for s in ["<a>", "a & b", "\"'", "&amp;", "&#65;", "&lt;&gt;", "é<日>"]))

# 9. unescape: argument and type errors, subclasses, non-str
print("unescape_err:", run(unescape), run(unescape, "a", "b"), run(unescape, foo="a"), run(unescape, "a", s="b"), run(unescape, s="&lt;"))
print("unescape_nonstr:", run(unescape, 5), run(unescape, None), run(unescape, b"a&"), run(unescape, b"ab"), run(unescape, bytearray(b"a")), run(unescape, ["a"]), run(unescape, ["&"]), run(unescape, ("&amp;",)), run(unescape, {"&"}), run(unescape, 1.5))
print("unescape_subclass:", run(unescape, S("a&amp;b")), run(unescape, S("ab")), run(unescape, S("a&;b")), run(unescape, S("&#65;")), run(unescape, S("")))
sub = S("keep")
print("unescape_subclass_identity:", unescape(sub) is sub, type(unescape(S("&amp;"))).__name__, type(unescape(S("&;"))).__name__)

# 10. _replace_charref and the compiled pattern
import re

m = html._charref.search("x &amp; y")
print("charref_search:", m.group(0), m.group(1), m.span(), type(m).__name__)
print("charref_sub:", ascii(html._charref.sub(html._replace_charref, "&lt;&#65;&#x42;&notit;&unknown;")))
print("replace_charref:", [html._replace_charref(html._charref.match(t)) for t in ["&amp;", "&#65;", "&#x41", "&notit;", "&bogus", "&#0;", "&#xD800;", "&#x0b;", "&#x80;"]])
print("charref_flags:", html._charref.flags == re.compile(html._charref.pattern).flags, html._re is re)

# 11. html.parser interop
class P(html.parser.HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.out = []

    def handle_starttag(self, tag, attrs):
        self.out.append(("start", tag, attrs))

    def handle_data(self, data):
        self.out.append(("data", data))


p = P()
p.feed("<a href='x?a=1&amp;b=2&#38;c=&copy;'>t &lt;&#x41;&notit; &amp</a>")
p.close()
print("parser:", p.out)
print("parser_module:", html.parser.HTMLParser.__module__, html.parser.__name__)

# 12. escape/unescape stress for consistency
s = "".join(chr(c) for c in range(0x20, 0x7F)) * 20
print("stress:", len(escape(s)), unescape(escape(s)) == s, escape(unescape(escape(s))) == escape(s))
