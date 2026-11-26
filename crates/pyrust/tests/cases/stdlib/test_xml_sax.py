"""CPython 3.12.3 parity tests for `xml.sax.*` (#3260)."""

import io
import xml.sax
import xml.sax.expatreader
import xml.sax.handler
import xml.sax.saxutils
import xml.sax.xmlreader


# ── Constants & exceptions ───────────────────────────────────────────────────
assert xml.sax.default_parser_list == ["xml.sax.expatreader"]
assert xml.sax.handler.feature_namespaces == "http://xml.org/sax/features/namespaces"
assert xml.sax.handler.feature_namespace_prefixes == "http://xml.org/sax/features/namespace-prefixes"
assert xml.sax.handler.property_lexical_handler == "http://xml.org/sax/properties/lexical-handler"
assert len(xml.sax.handler.all_features) == 6
assert len(xml.sax.handler.all_properties) == 6

e1 = xml.sax.SAXException("boom", ValueError("v"))
assert str(e1) == "boom"
assert e1.getMessage() == "boom"
assert isinstance(e1.getException(), ValueError)
assert issubclass(xml.sax.SAXParseException, xml.sax.SAXException)
assert issubclass(xml.sax.SAXNotRecognizedException, xml.sax.SAXException)
assert issubclass(xml.sax.SAXNotSupportedException, xml.sax.SAXException)
assert issubclass(xml.sax.SAXReaderNotAvailable, xml.sax.SAXNotSupportedException)

# ── saxutils escape / unescape / quoteattr ───────────────────────────────────
assert xml.sax.saxutils.escape("<a & b>") == "&lt;a &amp; b&gt;"
assert xml.sax.saxutils.escape('<a"b>', {'"': "&quot;"}) == "&lt;a&quot;b&gt;"
assert xml.sax.saxutils.unescape("&lt;a &amp; b&gt;") == "<a & b>"
assert xml.sax.saxutils.unescape("&lt;&quot;&gt;", {"&quot;": '"'}) == '<">'
assert xml.sax.saxutils.quoteattr("hello") == '"hello"'
assert xml.sax.saxutils.quoteattr('say "hi"') == "'say \"hi\"'"
assert xml.sax.saxutils.quoteattr('it\'s "both"') == '"it\'s &quot;both&quot;"'
assert xml.sax.saxutils.quoteattr("a\nb\tc\rd") == '"a&#10;b&#9;c&#13;d"'

# ── Basic parseString & ContentHandler ───────────────────────────────────────
class Recorder(xml.sax.ContentHandler):
    def __init__(self):
        super().__init__()
        self.events = []

    def startDocument(self):
        self.events.append(("startDoc",))

    def endDocument(self):
        self.events.append(("endDoc",))

    def startElement(self, name, attrs):
        self.events.append(("start", name, dict(attrs.items())))

    def endElement(self, name):
        self.events.append(("end", name))

    def characters(self, content):
        if self.events and self.events[-1][0] == "chars":
            self.events[-1] = ("chars", self.events[-1][1] + content)
        else:
            self.events.append(("chars", content))

    def processingInstruction(self, target, data):
        self.events.append(("pi", target, data))


rec = Recorder()
xml.sax.parseString(b'<root k="1"><?p d?><a>hi &amp; bye</a><b/></root>', rec)
assert rec.events == [
    ("startDoc",),
    ("start", "root", {"k": "1"}),
    ("pi", "p", "d"),
    ("start", "a", {}),
    ("chars", "hi & bye"),
    ("end", "a"),
    ("start", "b", {}),
    ("end", "b"),
    ("end", "root"),
    ("endDoc",),
]

# ── Namespace-aware SAX parsing & LexicalHandler ─────────────────────────────
class NSRecorder(xml.sax.ContentHandler):
    def __init__(self):
        super().__init__()
        self.events = []

    def startPrefixMapping(self, prefix, uri):
        self.events.append(("ns+", prefix, uri))

    def endPrefixMapping(self, prefix):
        self.events.append(("ns-", prefix))

    def startElementNS(self, name, qname, attrs):
        self.events.append(("startNS", name, qname, sorted(attrs.items()), attrs.getQNames()))

    def endElementNS(self, name, qname):
        self.events.append(("endNS", name, qname))


class LexRec(xml.sax.handler.LexicalHandler):
    def __init__(self):
        self.lex = []

    def comment(self, content):
        self.lex.append(("comment", content))

    def startCDATA(self):
        self.lex.append(("cdata+",))

    def endCDATA(self):
        self.lex.append(("cdata-",))


parser = xml.sax.make_parser()
parser.setFeature(xml.sax.handler.feature_namespaces, True)
assert bool(parser.getFeature(xml.sax.handler.feature_namespaces)) is True
ns_rec = NSRecorder()
lex_rec = LexRec()
parser.setContentHandler(ns_rec)
parser.setProperty(xml.sax.handler.property_lexical_handler, lex_rec)
parser.parse(io.BytesIO(b'<p:root xmlns:p="urn:p" p:id="7"><!--c--><![CDATA[x]]></p:root>'))
assert ("ns+", "p", "urn:p") in ns_rec.events
assert ("startNS", ("urn:p", "root"), None, [(("urn:p", "id"), "7")], ["p:id"]) in ns_rec.events
assert ("endNS", ("urn:p", "root"), None) in ns_rec.events
assert ("ns-", "p") in ns_rec.events
assert lex_rec.lex == [("comment", "c"), ("cdata+",), ("cdata-",)]

# ── IncrementalParser feed / close ───────────────────────────────────────────
inc = xml.sax.expatreader.create_parser()
rec2 = Recorder()
inc.setContentHandler(rec2)
inc.feed(b"<doc>")
inc.feed(b"<x>42</x>")
inc.feed(b"</doc>")
inc.close()
assert ("start", "doc", {}) in rec2.events
assert ("chars", "42") in rec2.events
assert ("end", "doc") in rec2.events

# ── Error handling (SAXParseException) ───────────────────────────────────────
try:
    xml.sax.parseString(b"<a></b>", xml.sax.ContentHandler())
    assert False, "expected SAXParseException"
except xml.sax.SAXParseException as exc:
    assert exc.getLineNumber() == 1
    assert exc.getColumnNumber() >= 0

# ── XMLGenerator & XMLFilterBase ─────────────────────────────────────────────
buf = io.StringIO()
gen = xml.sax.saxutils.XMLGenerator(buf, encoding="utf-8", short_empty_elements=True)
gen.startDocument()
gen.startElement("root", {"a": "1", "b": 'x"y'})
gen.startElement("empty", {})
gen.endElement("empty")
gen.startElement("txt", {})
gen.characters("a < b & c")
gen.endElement("txt")
gen.endElement("root")
gen.endDocument()
out_xml = buf.getvalue()
assert out_xml == (
    '<?xml version="1.0" encoding="utf-8"?>\n'
    '<root a="1" b=\'x"y\'><empty/><txt>a &lt; b &amp; c</txt></root>'
)

# XMLFilterBase uppercase tag filter
class UpperFilter(xml.sax.saxutils.XMLFilterBase):
    def startElement(self, name, attrs):
        super().startElement(name.upper(), attrs)

    def endElement(self, name):
        super().endElement(name.upper())


fbuf = io.StringIO()
fgen = xml.sax.saxutils.XMLGenerator(fbuf, encoding="utf-8", short_empty_elements=True)
flt = UpperFilter(xml.sax.make_parser())
flt.setContentHandler(fgen)
flt.parse(io.BytesIO(b"<root><item/></root>"))
assert "<ROOT><ITEM/></ROOT>" in fbuf.getvalue()

# ── AttributesImpl & AttributesNSImpl ────────────────────────────────────────
a1 = xml.sax.xmlreader.AttributesImpl({"x": "1", "y": "2"})
assert len(a1) == 2 and a1.getLength() == 2
assert a1["x"] == "1" and a1.getValue("y") == "2"
assert "x" in a1 and "z" not in a1
assert a1.get("z", "def") == "def"
assert a1.getType("x") == "CDATA"
assert sorted(a1.keys()) == ["x", "y"]
assert sorted(a1.getNames()) == ["x", "y"]
assert a1.copy()["x"] == "1"

a2 = xml.sax.xmlreader.AttributesNSImpl({("urn:a", "k"): "v"}, {("urn:a", "k"): "a:k"})
assert a2.getValueByQName("a:k") == "v"
assert a2.getNameByQName("a:k") == ("urn:a", "k")
assert a2.getQNameByName(("urn:a", "k")) == "a:k"
assert a2.getQNames() == ["a:k"]

print("xml.sax OK")
