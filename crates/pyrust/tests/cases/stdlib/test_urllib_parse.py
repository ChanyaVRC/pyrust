# CPython 3.12 parity tests for `urllib` and `urllib.parse` (issue #3149).

import sys
import urllib
import urllib.parse
from urllib import parse as parse_from_urllib
from urllib.parse import (
    DefragResult,
    DefragResultBytes,
    ParseResult,
    ParseResultBytes,
    SplitResult,
    SplitResultBytes,
    clear_cache,
    parse_qs,
    parse_qsl,
    quote,
    quote_from_bytes,
    quote_plus,
    unquote,
    unquote_plus,
    unquote_to_bytes,
    unwrap,
    urldefrag,
    urlencode,
    urljoin,
    urlparse,
    urlsplit,
    urlunparse,
    urlunsplit,
)

# 1. Package & submodule import identity
print("mod_id:", urllib.parse is parse_from_urllib, urllib.parse is sys.modules["urllib.parse"])
print("pkg_id:", urllib is sys.modules["urllib"])
print("all_names:", sorted(urllib.parse.__all__))

# 2. urlparse / urlunparse basic & structured attributes
u = urlparse("HTTP://user:pass@Example.COM:8080/a/b;p=1?k=v&x=2#sec")
print("urlparse_full:", tuple(u), u.scheme, u.netloc, u.path, u.params, u.query, u.fragment)
print("urlparse_props:", u.username, u.password, u.hostname, u.port)
print("urlparse_geturl:", u.geturl(), urlunparse(u))
print("isinstance_pr:", isinstance(u, ParseResult), isinstance(u, tuple))

# Default scheme & allow_fragments
print("default_scheme:", tuple(urlparse("//example.com/p", scheme="https")))
print("no_frags:", tuple(urlparse("https://example.com/p?q=1#frag", allow_fragments=False)))
print("non_param_scheme:", tuple(urlparse("mailto:user@example.com;extra")))
print("relative:", tuple(urlparse("foo/bar;p?q=1#f")))
print("c0_strip:", tuple(urlparse(" \t\r\nhttps://ex\tample.com/a\nb\rc ")))

# Auth edge cases
for sample in [
    "http://alice@host/p",
    "http://alice:@host/p",
    "http://:secret@host/p",
    "http://a@b:c@host:90/p",
    "http://host/p",
    "/relative/only",
]:
    r = urlparse(sample)
    print("auth:", sample, r.username, r.password, r.hostname, r.port)

# Port validation
for bad_port in ["http://host:abc/", "http://host:99999/", "http://host:-1/"]:
    r = urlparse(bad_port)
    try:
        _ = r.port
        print("port_err: unexpected ok", bad_port)
    except ValueError as e:
        print("port_err:", bad_port, type(e).__name__)

# IPv6 & scoped IPv6
for ipv6_url in [
    "http://[::1]:8080/path",
    "http://[2001:DB8::1]/",
    "http://[fe80::822a:a8ff:fe49:470c%tESt]:1234/keys",
    "http://user:pw@[::ffff:192.0.2.1]:443/x",
]:
    r = urlparse(ipv6_url)
    print("ipv6:", r.hostname, r.port, r.username, r.password, r.geturl())

for bad_ipv6 in ["http://[::1/", "http://::1]/"]:
    try:
        urlparse(bad_ipv6)
        print("bad_ipv6: unexpected ok", bad_ipv6)
    except ValueError as e:
        print("bad_ipv6:", bad_ipv6, str(e))

# NFKC netloc check
try:
    urlsplit("http://example\u2100.com/path")
    print("nfkc: unexpected ok")
except ValueError:
    print("nfkc: ValueError")

# Bytes urlparse / urlsplit / encode / decode
ub = urlparse(b"https://user:pw@[2001:DB8::1]:8443/a;p?q=1#f")
print("bytes_pr:", isinstance(ub, ParseResultBytes), tuple(ub))
print("bytes_props:", ub.username, ub.password, ub.hostname, ub.port, ub.geturl())
print("roundtrip_enc_dec:", ub.decode() == urlparse("https://user:pw@[2001:DB8::1]:8443/a;p?q=1#f"))
print("str_to_bytes:", u.encode() == urlparse(b"HTTP://user:pass@Example.COM:8080/a/b;p=1?k=v&x=2#sec"))

try:
    urlparse("http://example.com", scheme=b"http")
    print("mix_err: unexpected ok")
except TypeError as e:
    print("mix_err:", str(e))

# 3. urlsplit / urlunsplit
s = urlsplit("https://example.com/a;p?q=1#f")
print("urlsplit:", isinstance(s, SplitResult), tuple(s), s.geturl(), urlunsplit(s))
sb = urlsplit(b"https://example.com/a;p?q=1#f")
print("urlsplit_bytes:", isinstance(sb, SplitResultBytes), tuple(sb), sb.geturl())
print("urlunsplit_file:", urlunsplit(("file", "", "/etc/hosts", "", "")))
print("urlunsplit_mailto:", urlunsplit(("mailto", "", "a@b.com", "", "")))

# 4. urljoin
base = "http://a/b/c/d;p?q"
for rel in [
    "g:h",
    "g",
    "./g",
    "g/",
    "/g",
    "//g",
    "?y",
    "g?y",
    "#s",
    "g#s",
    "g?y#s",
    ";x",
    "g;x",
    "g;x?y#s",
    "",
    ".",
    "./",
    "..",
    "../",
    "../g",
    "../..",
    "../../",
    "../../g",
    "../../../g",
    "../../../../g",
]:
    print("urljoin:", rel, "->", urljoin(base, rel))
print("urljoin_empty_base:", urljoin("", "http://example.com/a"))
print("urljoin_bytes:", urljoin(b"http://example.com/a/b", b"../c"))
print("urljoin_nofrag:", urljoin("http://example.com/a#1", "b#2", allow_fragments=False))

# 5. urldefrag
d1 = urldefrag("https://example.com/path?q=1#frag")
d2 = urldefrag("https://example.com/path#")
d3 = urldefrag("https://example.com/path")
db = urldefrag(b"https://example.com/path#frag")
print("defrag1:", isinstance(d1, DefragResult), tuple(d1), d1.geturl())
print("defrag2:", tuple(d2), d2.geturl())
print("defrag3:", tuple(d3), d3.geturl())
print("defrag_bytes:", isinstance(db, DefragResultBytes), tuple(db), db.geturl())

# 6. quote / quote_plus / quote_from_bytes
print("quote_default:", quote("/El Niño/a b+c~d_e-f.g?"))
print("quote_safe_empty:", quote("/a/b c", safe=""))
print("quote_custom_safe:", quote("a:b/c?d", safe=":?"))
print("quote_latin1:", quote("café", encoding="latin-1"))
print("quote_ignore:", quote("café", encoding="ascii", errors="ignore"))
print("quote_bytes:", quote(b"a b/c\xff"))
print("quote_plus_basic:", quote_plus("a b+c/d~e"))
print("quote_plus_safe:", quote_plus("a b/c", safe="/"))
print("quote_plus_bytes:", quote_plus(b"a b+c"))
print("quote_from_bytes:", quote_from_bytes(b"abc def/ghi\x00\xff", safe=b"/"))

for fn_name, call in [
    ("quote_bytes_enc", lambda: quote(b"abc", encoding="utf-8")),
    ("quote_bytes_err", lambda: quote(b"abc", errors="strict")),
    ("qfb_str", lambda: quote_from_bytes("abc")),
]:
    try:
        call()
        print(fn_name, "unexpected ok")
    except TypeError as e:
        print(fn_name, str(e))

# 7. unquote / unquote_plus / unquote_to_bytes
print("unquote_basic:", unquote("abc%20def%2F%2f%7e"))
print("unquote_utf8:", unquote("caf%C3%A9%20%F0%9F%9A%80"))
print("unquote_mixed_nonascii:", unquote("héllo%20wörld%21"))
print("unquote_invalid_pct:", unquote("100%25%ZZ%2%"))
print("unquote_replace:", unquote("%ff", encoding="utf-8", errors="replace"))
print("unquote_latin1:", unquote("%e9", encoding="latin-1"))
print("unquote_bytes_in:", unquote(b"a%20b%C3%A9"))
print("unquote_plus:", unquote_plus("a+b%2Bb%20c"))
print("unquote_to_bytes_str:", unquote_to_bytes("a%20b%ff%ZZ"))
print("unquote_to_bytes_bytes:", unquote_to_bytes(b"a%20b%ff"))

# 8. urlencode
print("urlencode_dict:", urlencode({"a": "1 2", "b": "x/y+z"}))
print("urlencode_seq:", urlencode([("k", 1), ("k", 2), ("m", "a b")]))
print("urlencode_doseq_false:", urlencode({"a": [1, 2]}))
print("urlencode_doseq_true:", urlencode({"a": [1, 2], "s": "hi", "b": b"xy", "n": 42}, doseq=True))
print("urlencode_quote_via:", urlencode({"a": "hello world/1"}, quote_via=quote))
print("urlencode_bytes:", urlencode([(b"k 1", b"v/2")]))
print("urlencode_safe:", urlencode({"p": "/a/b"}, safe="/"))
try:
    urlencode("not a mapping")
    print("urlencode_err: unexpected ok")
except TypeError as e:
    print("urlencode_err:", str(e))

# 9. parse_qs / parse_qsl
print("parse_qs_basic:", parse_qs("a=1&b=2&a=3&empty=&noval"))
print("parse_qs_blank:", parse_qs("a=1&empty=&noval", keep_blank_values=True))
print("parse_qsl_basic:", parse_qsl("k=hello+world&k=a%26b&blank="))
print("parse_qsl_sep:", parse_qsl("a=1;b=2;c=3", separator=";"))
print("parse_qsl_bytes:", parse_qsl(b"a=1+2&b=%2F", keep_blank_values=True))
print("parse_qsl_bytearray:", parse_qsl(bytearray(b"x=1&y=2")))
print("parse_qs_max_ok:", parse_qs("a=1&b=2", max_num_fields=2))

for label, call in [
    ("strict_no_eq", lambda: parse_qsl("a=1&bad", strict_parsing=True)),
    ("strict_empty", lambda: parse_qsl("a=1&&b=2", strict_parsing=True)),
    ("max_fields", lambda: parse_qs("a=1&b=2&c=3", max_num_fields=2)),
    ("bad_sep", lambda: parse_qsl("a=1", separator="")),
]:
    try:
        call()
        print(label, "unexpected ok")
    except ValueError as e:
        print(label, type(e).__name__, str(e))

# 10. clear_cache & unwrap
clear_cache()
print("unwrap1:", unwrap("<URL:https://example.com/a>"))
print("unwrap2:", unwrap("  < https://example.com/b >  "))
print("unwrap3:", unwrap("https://example.com/c"))
