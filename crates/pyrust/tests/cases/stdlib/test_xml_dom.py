"""CPython 3.12.3 parity tests for `xml.dom.*` (#3260)."""

import io
import xml.dom
import xml.dom.NodeFilter
import xml.dom.domreg
import xml.dom.expatbuilder
import xml.dom.minicompat
import xml.dom.minidom
import xml.dom.pulldom
import xml.dom.xmlbuilder


# ── xml.dom constants & exception hierarchy ──────────────────────────────────
assert xml.dom.Node.ELEMENT_NODE == 1
assert xml.dom.Node.ATTRIBUTE_NODE == 2
assert xml.dom.Node.TEXT_NODE == 3
assert xml.dom.Node.CDATA_SECTION_NODE == 4
assert xml.dom.Node.COMMENT_NODE == 8
assert xml.dom.Node.DOCUMENT_NODE == 9
assert xml.dom.Node.DOCUMENT_TYPE_NODE == 10
assert xml.dom.Node.DOCUMENT_FRAGMENT_NODE == 11
assert xml.dom.Node.NOTATION_NODE == 12

assert xml.dom.EMPTY_NAMESPACE is None
assert xml.dom.EMPTY_PREFIX is None
assert xml.dom.XML_NAMESPACE == "http://www.w3.org/XML/1998/namespace"
assert xml.dom.XMLNS_NAMESPACE == "http://www.w3.org/2000/xmlns/"
assert xml.dom.XHTML_NAMESPACE == "http://www.w3.org/1999/xhtml"

try:
    xml.dom.DOMException("direct")
    assert False, "expected RuntimeError"
except RuntimeError:
    pass

for cls, code in [
    (xml.dom.IndexSizeErr, 1),
    (xml.dom.DomstringSizeErr, 2),
    (xml.dom.HierarchyRequestErr, 3),
    (xml.dom.WrongDocumentErr, 4),
    (xml.dom.InvalidCharacterErr, 5),
    (xml.dom.NoDataAllowedErr, 6),
    (xml.dom.NoModificationAllowedErr, 7),
    (xml.dom.NotFoundErr, 8),
    (xml.dom.NotSupportedErr, 9),
    (xml.dom.InuseAttributeErr, 10),
    (xml.dom.InvalidStateErr, 11),
    (xml.dom.SyntaxErr, 12),
    (xml.dom.InvalidModificationErr, 13),
    (xml.dom.NamespaceErr, 14),
    (xml.dom.InvalidAccessErr, 15),
    (xml.dom.ValidationErr, 16),
]:
    err = cls("test")
    assert isinstance(err, xml.dom.DOMException)
    assert err.code == code

# ── minicompat, domreg, NodeFilter ───────────────────────────────────────────
nl = xml.dom.minicompat.NodeList([10, 20])
assert nl.length == 2
assert nl.item(0) == 10
assert nl.item(5) is None
enl = xml.dom.minicompat.EmptyNodeList()
assert enl.length == 0
assert enl.item(0) is None
assert isinstance(enl + [1], xml.dom.minicompat.NodeList)
assert xml.dom.minicompat.StringTypes == (str,)

nf = xml.dom.NodeFilter.NodeFilter
assert nf.FILTER_ACCEPT == 1
assert nf.FILTER_REJECT == 2
assert nf.FILTER_SKIP == 3
assert nf.SHOW_ALL == 0xFFFFFFFF
assert nf.SHOW_ELEMENT == 1

dom_impl = xml.dom.getDOMImplementation()
assert dom_impl.hasFeature("Core", "2.0") is True
assert dom_impl.hasFeature("XML", "2.0") is True
assert dom_impl.hasFeature("NonExistent", "1.0") is False

# ── minidom parsing & serialization ──────────────────────────────────────────
xml_src = (
    '<root a="1" b="2">'
    '<child>hello &amp; world</child>'
    '<empty/>'
    '<!-- c -->'
    '<?pi data?>'
    '<![CDATA[<x>]]>'
    '</root>'
)
doc = xml.dom.minidom.parseString(xml_src)
assert doc.nodeType == xml.dom.Node.DOCUMENT_NODE
assert doc.documentElement.tagName == "root"
assert doc.documentElement.getAttribute("a") == "1"
assert doc.documentElement.getAttribute("b") == "2"
assert doc.documentElement.hasAttribute("a") is True
assert doc.documentElement.hasAttribute("missing") is False
assert doc.documentElement.getAttribute("missing") == ""

assert doc.toxml() == (
    '<?xml version="1.0" ?>'
    '<root a="1" b="2"><child>hello &amp; world</child><empty/><!-- c --><?pi data?><![CDATA[<x>]]></root>'
)
assert doc.toxml(encoding="utf-8") == (
    b'<?xml version="1.0" encoding="utf-8"?>'
    b'<root a="1" b="2"><child>hello &amp; world</child><empty/><!-- c --><?pi data?><![CDATA[<x>]]></root>'
)
assert doc.toxml(standalone=True).startswith('<?xml version="1.0" standalone="yes"?>')
assert doc.toprettyxml(indent="  ") == (
    '<?xml version="1.0" ?>\n'
    '<root a="1" b="2">\n'
    '  <child>hello &amp; world</child>\n'
    '  <empty/>\n'
    '  <!-- c -->\n'
    '  <?pi data?>\n'
    '<![CDATA[<x>]]></root>\n'
)

# ── DOM mutation, traversal, namespaces, clone, normalize, IDs ───────────────
doc2 = dom_impl.createDocument(None, "items", None)
root2 = doc2.documentElement
e1 = doc2.createElement("item")
e1.setAttribute("id", "i1")
e1.setIdAttribute("id")
t1 = doc2.createTextNode("foo")
t2 = doc2.createTextNode("bar")
e1.appendChild(t1)
e1.appendChild(t2)
root2.appendChild(e1)

assert len(e1.childNodes) == 2
e1.normalize()
assert len(e1.childNodes) == 1
assert e1.firstChild.data == "foobar"

t_split = e1.firstChild.splitText(3)
assert e1.firstChild.data == "foo"
assert t_split.data == "bar"
assert e1.firstChild.wholeText == "foobar"
assert doc2.getElementById("i1") is e1

e2 = doc2.createElement("item")
e2.setAttribute("name", "second")
root2.insertBefore(e2, e1)
assert [n.tagName for n in doc2.getElementsByTagName("item")] == ["item", "item"]
assert root2.firstChild is e2
assert root2.lastChild is e1
assert e2.nextSibling is e1
assert e1.previousSibling is e2

cloned = e1.cloneNode(True)
assert cloned is not e1
assert cloned.getAttribute("id") == "i1"
assert cloned.toxml() == e1.toxml()

e2.removeAttribute("name")
assert e2.hasAttribute("name") is False
root2.removeChild(e2)
assert len(root2.childNodes) == 1

# Namespace queries
ns_doc = xml.dom.minidom.parseString(
    '<a:root xmlns:a="urn:a" xmlns="urn:def"><a:item a:k="v"/><other/></a:root>'
)
ns_items = ns_doc.getElementsByTagNameNS("urn:a", "item")
assert len(ns_items) == 1
assert ns_items[0].localName == "item"
assert ns_items[0].prefix == "a"
assert ns_items[0].namespaceURI == "urn:a"
assert ns_items[0].getAttributeNS("urn:a", "k") == "v"
assert len(ns_doc.getElementsByTagNameNS("urn:def", "other")) == 1
assert len(ns_doc.getElementsByTagNameNS("*", "*")) == 3

# ── expatbuilder & xmlbuilder ────────────────────────────────────────────────
frag = xml.dom.expatbuilder.parseFragmentString("<x/><y>1</y>", doc)
assert [c.tagName for c in frag.childNodes] == ["x", "y"]

opts = xml.dom.xmlbuilder.Options()
assert opts.namespaces == 1
builder = dom_impl.createDOMBuilder(dom_impl.MODE_SYNCHRONOUS, None)
assert builder.supportsFeature("namespaces") is True
assert builder.canSetFeature("comments", False) is True
src = dom_impl.createDOMInputSource()
src.byteStream = io.BytesIO(b"<built><ok/></built>")
bdoc = builder.parse(src)
assert bdoc.documentElement.tagName == "built"

# ── pulldom ──────────────────────────────────────────────────────────────────
stream = xml.dom.pulldom.parseString('<root x="1"><a/><b>hi</b></root>')
pulled = []
for ev, node in stream:
    if ev == xml.dom.pulldom.START_ELEMENT and node.tagName == "b":
        stream.expandNode(node)
        pulled.append((ev, node.nodeName, node.toxml()))
    else:
        pulled.append((ev, node.nodeName))
assert pulled == [
    ("START_DOCUMENT", "#document"),
    ("START_ELEMENT", "root"),
    ("START_ELEMENT", "a"),
    ("END_ELEMENT", "a"),
    ("START_ELEMENT", "b", "<b>hi</b>"),
    ("END_ELEMENT", "root"),
]

# Cleanup
doc.unlink()
doc2.unlink()
ns_doc.unlink()
bdoc.unlink()

print("xml.dom OK")
