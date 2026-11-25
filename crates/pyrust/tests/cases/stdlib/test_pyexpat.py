"""CPython 3.12.3 parity tests for `pyexpat` and `xml.parsers.expat` (#3260)."""

import io
import pyexpat
import sys
import xml.parsers
import xml.parsers.expat
import xml.parsers.expat.errors
import xml.parsers.expat.model


# ── Module identity & metadata ───────────────────────────────────────────────
assert pyexpat.errors is xml.parsers.expat.errors
assert pyexpat.model is xml.parsers.expat.model
assert sys.modules["pyexpat.errors"] is pyexpat.errors
assert sys.modules["pyexpat.model"] is pyexpat.model
assert sys.modules["xml.parsers.expat.errors"] is pyexpat.errors
assert sys.modules["xml.parsers.expat.model"] is pyexpat.model
assert pyexpat.ExpatError is pyexpat.error
assert xml.parsers.expat.ExpatError is pyexpat.ExpatError
assert xml.parsers.expat.error is pyexpat.ExpatError
assert pyexpat.XMLParserType is xml.parsers.expat.XMLParserType
assert pyexpat.native_encoding == "UTF-8"
assert isinstance(pyexpat.EXPAT_VERSION, str) and pyexpat.EXPAT_VERSION.startswith("expat_")
assert isinstance(pyexpat.version_info, tuple) and len(pyexpat.version_info) == 3
assert isinstance(pyexpat.features, list) and len(pyexpat.features) > 0
assert pyexpat.XML_PARAM_ENTITY_PARSING_NEVER == 0
assert pyexpat.XML_PARAM_ENTITY_PARSING_UNLESS_STANDALONE == 1
assert pyexpat.XML_PARAM_ENTITY_PARSING_ALWAYS == 2

# ── errors & model submodules ────────────────────────────────────────────────
assert pyexpat.ErrorString(0) is None
assert pyexpat.ErrorString(9999) is None
assert pyexpat.ErrorString(2) == "syntax error"
assert pyexpat.ErrorString(7) == "mismatched tag"
assert pyexpat.errors.XML_ERROR_SYNTAX == "syntax error"
assert pyexpat.errors.XML_ERROR_TAG_MISMATCH == "mismatched tag"
assert pyexpat.errors.XML_ERROR_NO_ELEMENTS == "no element found"
assert pyexpat.errors.XML_ERROR_UNBOUND_PREFIX == "unbound prefix"
assert pyexpat.errors.codes[pyexpat.errors.XML_ERROR_SYNTAX] == 2
assert pyexpat.errors.messages[2] == "syntax error"
assert len(pyexpat.errors.codes) == len(pyexpat.errors.messages)
assert len(pyexpat.errors.codes) >= 40

assert pyexpat.model.XML_CTYPE_EMPTY == 1
assert pyexpat.model.XML_CTYPE_ANY == 2
assert pyexpat.model.XML_CTYPE_MIXED == 3
assert pyexpat.model.XML_CTYPE_NAME == 4
assert pyexpat.model.XML_CTYPE_CHOICE == 5
assert pyexpat.model.XML_CTYPE_SEQ == 6
assert pyexpat.model.XML_CQUANT_NONE == 0
assert pyexpat.model.XML_CQUANT_OPT == 1
assert pyexpat.model.XML_CQUANT_REP == 2
assert pyexpat.model.XML_CQUANT_PLUS == 3

# ── ParserCreate validation & defaults ───────────────────────────────────────
try:
    pyexpat.ParserCreate(namespace_separator="ab")
    assert False, "expected ValueError"
except ValueError:
    pass

p0 = pyexpat.ParserCreate()
assert p0.buffer_text is False
assert p0.buffer_size == 8192
assert p0.buffer_used == 0
assert p0.namespace_prefixes is False
assert p0.ordered_attributes is False
assert p0.specified_attributes is False
assert p0.CurrentLineNumber == 1
assert p0.CurrentColumnNumber == 0
assert p0.ErrorCode == 0
assert p0.GetBase() is None
p0.SetBase("http://example.com/doc.xml")
assert p0.GetBase() == "http://example.com/doc.xml"

# ── Basic & chunked streaming parsing ────────────────────────────────────────
events = []
p = pyexpat.ParserCreate()
p.buffer_text = True
p.StartElementHandler = lambda n, a: events.append(("start", n, a))
p.EndElementHandler = lambda n: events.append(("end", n))
p.CharacterDataHandler = lambda d: events.append(("char", d))
p.CommentHandler = lambda d: events.append(("comment", d))
p.ProcessingInstructionHandler = lambda t, d: events.append(("pi", t, d))
p.StartCdataSectionHandler = lambda: events.append(("cdata_start",))
p.EndCdataSectionHandler = lambda: events.append(("cdata_end",))
p.XmlDeclHandler = lambda v, e, s: events.append(("xmldecl", v, e, s))
p.StartDoctypeDeclHandler = lambda n, s, pu, hi: events.append(("dt_start", n, s, pu, hi))
p.EndDoctypeDeclHandler = lambda: events.append(("dt_end",))

doc_bytes = (
    b'<?xml version="1.0" encoding="utf-8" standalone="yes"?>'
    b'<!DOCTYPE root [<!ENTITY greet "Hello &amp; Hi">]>'
    b'<root a="1" b="x&lt;y"><!--note--><?pi val?>'
    b'<child>&greet; &#65;&#x42;</child>'
    b'<![CDATA[<raw>&text;]]>'
    b'<empty/>'
    b'</root>'
)
# Feed in small chunks to exercise incremental streaming
for i in range(0, len(doc_bytes), 7):
    assert p.Parse(doc_bytes[i : i + 7], False) == 1
assert p.Parse(b"", True) == 1

# Coalesce adjacent char events (chunk/entity boundaries may split CharacterDataHandler)
coalesced = []
for ev in events:
    if ev[0] == "char" and coalesced and coalesced[-1][0] == "char":
        coalesced[-1] = ("char", coalesced[-1][1] + ev[1])
    else:
        coalesced.append(ev)

assert ("xmldecl", "1.0", "utf-8", 1) in coalesced
assert ("dt_start", "root", None, None, 1) in coalesced
assert ("dt_end",) in coalesced
assert ("start", "root", {"a": "1", "b": "x<y"}) in coalesced
assert ("comment", "note") in coalesced
assert ("pi", "pi", "val") in coalesced
assert ("start", "child", {}) in coalesced
assert ("char", "Hello & Hi AB") in coalesced
assert ("end", "child") in coalesced
assert ("cdata_start",) in coalesced
assert ("char", "<raw>&text;") in coalesced
assert ("cdata_end",) in coalesced
assert ("start", "empty", {}) in coalesced
assert ("end", "empty") in coalesced
assert ("end", "root") in coalesced

# ── ParseFile & str input ────────────────────────────────────────────────────
file_events = []
pf = pyexpat.ParserCreate()
pf.StartElementHandler = lambda n, a: file_events.append((n, a))
assert pf.ParseFile(io.BytesIO(b'<doc k="v"><item/></doc>')) == 1
assert file_events == [("doc", {"k": "v"}), ("item", {})]

str_events = []
ps = pyexpat.ParserCreate()
ps.CharacterDataHandler = lambda d: str_events.append(d)
assert ps.Parse("<msg>héllo 世界</msg>", True) == 1
assert "".join(str_events) == "héllo 世界"

# ── Namespaces (namespace_prefixes=False and True) ───────────────────────────
ns_events = []
pn = pyexpat.ParserCreate(namespace_separator=" ")
pn.namespace_prefixes = True
pn.StartNamespaceDeclHandler = lambda pfx, uri: ns_events.append(("ns+", pfx, uri))
pn.EndNamespaceDeclHandler = lambda pfx: ns_events.append(("ns-", pfx))
pn.StartElementHandler = lambda n, a: ns_events.append(("start", n, a))
pn.EndElementHandler = lambda n: ns_events.append(("end", n))
pn.Parse(b'<a:root xmlns:a="urn:a" xmlns="urn:def" a:x="1" y="2"><child/></a:root>', True)
assert ns_events == [
    ("ns+", "a", "urn:a"),
    ("ns+", None, "urn:def"),
    ("start", "urn:a root a", {"urn:a x a": "1", "y": "2"}),
    ("start", "urn:def child", {}),
    ("end", "urn:def child"),
    ("end", "urn:a root a"),
    ("ns-", None),
    ("ns-", "a"),
]

ns_events2 = []
pn2 = pyexpat.ParserCreate(namespace_separator=" ")
pn2.StartElementHandler = lambda n, a: ns_events2.append(("start", n, a))
pn2.EndElementHandler = lambda n: ns_events2.append(("end", n))
pn2.Parse(b'<a:root xmlns:a="urn:a" xmlns="urn:def" a:x="1" y="2"><child/></a:root>', True)
assert ns_events2 == [
    ("start", "urn:a root", {"urn:a x": "1", "y": "2"}),
    ("start", "urn:def child", {}),
    ("end", "urn:def child"),
    ("end", "urn:a root"),
]

# ── ordered_attributes & intern ──────────────────────────────────────────────
ord_events = []
po = pyexpat.ParserCreate()
po.ordered_attributes = True
po.StartElementHandler = lambda n, a: ord_events.append((n, a))
po.Parse(b'<r z="3" a="1" m="2"/>', True)
assert ord_events == [("r", ["z", "3", "a", "1", "m", "2"])]

intern_dict = {}
pi = pyexpat.ParserCreate(intern=intern_dict)
seen_names = []
pi.StartElementHandler = lambda n, a: seen_names.append(n)
pi.Parse(b"<r><x/><x/></r>", True)
assert seen_names == ["r", "x", "x"]
assert seen_names[1] is seen_names[2]
assert "x" in intern_dict

# ── DTD declaration callbacks ────────────────────────────────────────────────
dtd_events = []
pd = pyexpat.ParserCreate()
pd.EntityDeclHandler = lambda n, ip, v, b, s, pu, nt: dtd_events.append(("ent", n, ip, v))
pd.NotationDeclHandler = lambda n, b, s, pu: dtd_events.append(("not", n, s, pu))
pd.AttlistDeclHandler = lambda e, a, t, d, r: dtd_events.append(("att", e, a, t, d, r))
pd.ElementDeclHandler = lambda n, m: dtd_events.append(("elem", n, m[0]))
pd.Parse(
    b'<!DOCTYPE r ['
    b'<!ELEMENT r EMPTY>'
    b'<!ATTLIST r kind CDATA "default">'
    b'<!ENTITY e1 "val1">'
    b'<!NOTATION gif SYSTEM "image/gif">'
    b']><r/>',
    True,
)
assert ("elem", "r", pyexpat.model.XML_CTYPE_EMPTY) in dtd_events
assert ("att", "r", "kind", "CDATA", "default", 0) in dtd_events
assert ("ent", "e1", 0, "val1") in dtd_events
assert ("not", "gif", "image/gif", None) in dtd_events

# ── Malformed XML error reporting ────────────────────────────────────────────
bad_cases = [
    (b"", 3),
    (b"<a></b>", 7),
    (b'<a x="1" x="2"/>', 8),
    (b"<a/>extra", 9),
    (b"<a>&undef;</a>", 11),
    (b"<a>&#0;</a>", 14),
    (b'  <?xml version="1.0"?><a/>', 17),
    (b"<a><![CDATA[unclosed", 20),
    (b"<u:tag/>", 27),
]
for raw, expected_code in bad_cases:
    pe = pyexpat.ParserCreate(namespace_separator=" ")
    try:
        pe.Parse(raw, True)
        assert False, f"expected ExpatError for {raw!r}"
    except pyexpat.ExpatError as exc:
        assert exc.code == expected_code, (raw, exc.code, expected_code)
        assert pe.ErrorCode == expected_code
        assert exc.lineno >= 1
        assert exc.offset >= 0

# ── Encoding support (UTF-16, ISO-8859-1) ────────────────────────────────────
enc_out = []
pu16 = pyexpat.ParserCreate()
pu16.CharacterDataHandler = lambda d: enc_out.append(d)
pu16.Parse("<t>café</t>".encode("utf-16"), True)
assert "".join(enc_out) == "café"

lat_out = []
plat = pyexpat.ParserCreate()
plat.CharacterDataHandler = lambda d: lat_out.append(d)
plat.Parse('<?xml version="1.0" encoding="iso-8859-1"?><t>caf\xe9</t>'.encode("latin-1"), True)
assert "".join(lat_out) == "café"

print("pyexpat OK")
