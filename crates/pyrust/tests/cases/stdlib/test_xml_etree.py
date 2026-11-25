# Parity test for Python 3.12 xml, xml.etree, xml.etree.ElementPath, and xml.etree.ElementTree.

import contextlib
import copy
import io
import os
import pathlib
import sys
import tempfile
import warnings

import xml
import xml.etree
import xml.etree.ElementPath as EP
import xml.etree.ElementTree as ET
from xml.etree import ElementTree as ET2

# ── 1. Module & package identity ──────────────────────────────────────────────
print("id_etree:", xml.etree is sys.modules["xml.etree"])
print("id_et:", xml.etree.ElementTree is ET, ET is ET2, ET is sys.modules["xml.etree.ElementTree"])
print("id_ep:", xml.etree.ElementPath is EP, ET.ElementPath is EP)
print("names:", xml.__name__, xml.etree.__name__, ET.__name__, EP.__name__)
print("pkgs:", xml.__package__, xml.etree.__package__, ET.__package__, EP.__package__)
print("version:", ET.VERSION)
print("html_empty:", sorted(ET.HTML_EMPTY))
print("iselement:", ET.iselement(ET.Element("a")), ET.iselement("a"), ET.iselement(None))

# ── 2. Element & SubElement manipulation ──────────────────────────────────────
root = ET.Element("root", {"b": "2", "a": "1"}, c="3")
print("tag_attrib:", root.tag, list(root.items()), root.keys(), root.get("a"), root.get("missing", "def"))
root.set("d", "4")
print("after_set:", root.get("d"), len(root))

try:
    ET.Element("bad", "not-a-dict")
except TypeError:
    print("bad_attrib_type: TypeError")

c0 = ET.SubElement(root, "c0", id="0")
c1 = ET.SubElement(root, "c1", {"id": "1"})
c2 = ET.SubElement(root, "c2", id="2")
c3 = ET.SubElement(root, "c3", id="3")
print("len_iter:", len(root), [e.tag for e in root], root[-1].tag)
print("slice:", [e.tag for e in root[1:3]])

# __setitem__ (int and slice)
x = ET.Element("x")
root[1] = x
print("after_setitem_int:", [e.tag for e in root])
root[1:3] = [ET.Element("y0"), ET.Element("y1"), ET.Element("y2")]
print("after_setitem_slice:", [e.tag for e in root])

try:
    root[0] = "not-an-element"
except TypeError:
    print("setitem_bad: TypeError")

# __delitem__ (int and slice)
del root[0]
del root[1:3]
print("after_delitem:", [e.tag for e in root])

# insert, append, extend, remove, clear
root.insert(0, ET.Element("head"))
root.insert(-10, ET.Element("very_first"))
root.insert(999, ET.Element("tail_el"))
extra_el = ET.Element("extra")
root.append(extra_el)
root.extend([ET.Element("e1"), ET.Element("e2")])
print("after_ops:", [e.tag for e in root])
root.remove(extra_el)
print("after_remove:", [e.tag for e in root])

try:
    root.remove(ET.Element("e1"))
except ValueError:
    print("remove_missing: ValueError")

# Self-extend does not infinite loop
se = ET.Element("se")
ET.SubElement(se, "k")
se.extend(se)
print("self_extend:", len(se), se[0] is se[1])

# makeelement
made = root.makeelement("mk", {"k": "v"})
print("makeelement:", type(made) is ET.Element, made.tag, made.attrib)

# clear
made.text = "t"
made.tail = "tl"
ET.SubElement(made, "sub")
made.clear()
print("cleared:", made.tag, made.attrib, len(made), made.text, made.tail)

# copy.copy and copy.deepcopy
orig = ET.Element("orig", {"a": "1"})
orig.text = "hello"
orig.tail = "world"
child = ET.SubElement(orig, "child", {"k": "v"})
child.text = "inner"

shallow = copy.copy(orig)
deep = copy.deepcopy(orig)
print("copy_checks:", shallow is not orig, shallow[0] is orig[0], shallow.text == "hello", shallow.tail == "world")
print("deepcopy_checks:", deep is not orig, deep[0] is not orig[0], deep.attrib is not orig.attrib, deep[0].text == "inner")
shallow.append(ET.Element("added"))
deep.set("new", "val")
deep[0].text = "mutated"
print("copy_isolation:", len(orig), len(shallow), "new" in orig.attrib, orig[0].text, deep[0].text)

# iter and itertext
doc_text = ET.fromstring("<a>1<b>2<c>3</c>4</b>5</a>")
doc_text.tail = "OUTSIDE"
print("iter_all:", [e.tag for e in doc_text.iter()])
print("iter_star:", [e.tag for e in doc_text.iter("*")])
print("iter_b:", [e.tag for e in doc_text.iter("b")])
print("itertext:", list(doc_text.itertext()))

# itertext skips comments and PIs
tb_ci = ET.TreeBuilder(insert_comments=True, insert_pis=True)
p_ci = ET.XMLParser(target=tb_ci)
p_ci.feed("<a>hi<!--skip--><?pi skip?><b>there</b>!</a>")
ci_root = p_ci.close()
print("itertext_skip_cm_pi:", list(ci_root.itertext()))
print("tostring_cm_pi:", ET.tostring(ci_root, encoding="unicode"))

# ── 3. Comment, ProcessingInstruction, QName ──────────────────────────────────
cm = ET.Comment("a comment")
pi1 = ET.ProcessingInstruction("target", "data=1")
pi2 = ET.PI("onlytarget")
print("comment:", cm.tag is ET.Comment, cm.text, ET.tostring(cm, encoding="unicode"))
print("pi:", pi1.tag is ET.PI, ET.PI is ET.ProcessingInstruction, pi1.text, ET.tostring(pi1, encoding="unicode"))
print("pi2:", pi2.text, ET.tostring(pi2, encoding="unicode"))

q1 = ET.QName("http://example.com/ns", "item")
q2 = ET.QName("{http://example.com/ns}item")
q3 = ET.QName("http://example.com/ns", "other")
print("qname_str_repr:", str(q1), repr(q1))
print("qname_cmp:", q1 == q2, q1 == "{http://example.com/ns}item", q1 != q3, q1 < q3, q3 > q1, q1 <= q2, q1 >= q2)
print("qname_hash:", hash(q1) == hash(q2), hash(q1) == hash("{http://example.com/ns}item"))

qel = ET.Element(q1, {ET.QName("http://example.com/attr", "k"): q3})
print("qname_ser:", ET.tostring(qel, encoding="unicode"))

# ── 4. Parsing: entities, char refs, CDATA, quotes, newlines, bytes, DOCTYPE ─
xml_rich = (
    '<?xml version="1.0" encoding="utf-8" standalone="yes"?>\r\n'
    '<!-- header comment -->\r\n'
    '<?app info?>\r\n'
    '<!DOCTYPE doc [<!ENTITY greet "Hello">]>\r\n'
    '<doc a="1&amp;2&lt;3&gt;4&quot;5&apos;6" b=\'single\' ws="a\tb\nc\rd" cref="&#9;&#10;&#13;">'
    '&greet; &lt;world&gt; &#65;&#x42;&#x1f600;'
    '<![CDATA[raw <tag> &amp; not escaped]]>'
    '<empty/>\r\n'
    '</doc><!-- trailer -->'
)
rich = ET.fromstring(xml_rich)
print("rich_attrs:", rich.get("a"), rich.get("b"), repr(rich.get("ws")), repr(rich.get("cref")))
print("rich_text:", repr(rich.text))
print("rich_child:", rich[0].tag, repr(rich[0].tail))

# Bytes inputs: UTF-8, UTF-8 with BOM, ISO-8859-1 with XML decl
b_utf8 = "<a>café</a>".encode("utf-8")
b_bom = b"\xef\xbb\xbf<a>caf\xc3\xa9</a>"
b_latin = b"<?xml version='1.0' encoding='iso-8859-1'?><a>caf\xe9</a>"
print("bytes_parse:", ET.fromstring(b_utf8).text, ET.fromstring(b_bom).text, ET.fromstring(b_latin).text)

# XMLID & fromstringlist
id_root, id_map = ET.XMLID('<r id="r0"><a id="a1"><b id="b2"/></a><c/></r>')
print("xmlid:", id_root.tag, sorted(id_map.keys()), id_map["b2"].tag)
fsl = ET.fromstringlist(["<r>", "<a k='v'>", "hi", "</a>", "</r>"])
print("fromstringlist:", ET.tostring(fsl, encoding="unicode"))

# Chunked XMLParser feed (1-char and 1-byte splits across tags/entities/UTF-8/CDATA)
sample = "<?xml version='1.0' encoding='utf-8'?><r a='1&amp;2'><!--cm--><?pi x?><![CDATA[a<b]]>café\r\n</r>"
p_ch = ET.XMLParser()
for ch in sample:
    p_ch.feed(ch)
r_ch = p_ch.close()
print("chunk_1char:", r_ch.tag, r_ch.attrib, repr(r_ch.text))

p_b = ET.XMLParser()
sample_bytes = sample.encode("utf-8")
for i in range(len(sample_bytes)):
    p_b.feed(sample_bytes[i:i + 1])
r_b = p_b.close()
print("chunk_1byte:", r_b.tag, r_b.attrib, repr(r_b.text))

# Custom target on XMLParser
events_log = []

class CustomTarget:
    def start(self, tag, attrib):
        events_log.append(("start", tag, sorted(attrib.items())))

    def end(self, tag):
        events_log.append(("end", tag))

    def data(self, text):
        events_log.append(("data", text))

    def comment(self, text):
        events_log.append(("comment", text))

    def pi(self, target, data):
        events_log.append(("pi", target, data))

    def close(self):
        return "DONE"

ct_parser = ET.XMLParser(target=CustomTarget())
ct_parser.feed('<r k="v"><!--c--><?t d?>hi<x/></r>')
print("custom_target:", ct_parser.close(), events_log)

# ── 5. Serialization: tostring, tostringlist, write, dump ─────────────────────
ser = ET.Element("a", {"x": '1\t2\n3\r4"5&6<7>'})
ser.text = "hi & < > \u00e9"
print("ser_ascii:", ET.tostring(ser))
print("ser_utf8:", ET.tostring(ser, encoding="utf-8"))
print("ser_unicode:", repr(ET.tostring(ser, encoding="unicode")))
print("ser_latin1:", ET.tostring(ser, encoding="iso-8859-1"))
print("ser_decl_t:", ET.tostring(ET.Element("e"), encoding="utf-8", xml_declaration=True))
print("ser_decl_f:", ET.tostring(ET.Element("e"), encoding="iso-8859-1", xml_declaration=False))
print("ser_decl_uni:", repr(ET.tostring(ET.Element("e"), encoding="unicode", xml_declaration=True)))
print("ser_short_t:", ET.tostring(ET.Element("e"), encoding="unicode", short_empty_elements=True))
print("ser_short_f:", ET.tostring(ET.Element("e"), encoding="unicode", short_empty_elements=False))

html_el = ET.fromstring('<div><br/><hr/><p>a&lt;b</p><script>x&lt;y&amp;z</script><style>a&gt;b</style><input type="text"/></div>')
print("ser_html:", ET.tostring(html_el, encoding="unicode", method="html"))
print("ser_text:", repr(ET.tostring(ET.fromstring("<a>1<b>2</b>3</a>"), encoding="unicode", method="text")))
print("tostringlist_uni:", "".join(ET.tostringlist(ET.fromstring("<a><b>hi</b></a>"), encoding="unicode")))
print("tostringlist_bytes:", b"".join(ET.tostringlist(ET.fromstring("<a><b>hi</b></a>"))))

buf_out = io.StringIO()
with contextlib.redirect_stdout(buf_out):
    ET.dump(ET.fromstring("<a><b>1</b></a>"))
print("dump:", repr(buf_out.getvalue()))

try:
    ET.tostring(ET.Element("a"), method="bogus")
except ValueError:
    print("bad_method: ValueError")

# ── 6. Namespaces & register_namespace ────────────────────────────────────────
ns_xml = (
    '<root xmlns="http://def.example" xmlns:p="http://p.example" xml:lang="en" plain="1" p:attr="2">'
    '  <child/>'
    '  <p:item xmlns:p="http://p2.example"><sub xmlns=""/></p:item>'
    '</root>'
)
ns_root = ET.fromstring(ns_xml)
print("ns_root_tag:", ns_root.tag)
print("ns_root_attrs:", sorted(ns_root.items()))
print("ns_children:", [e.tag for e in ns_root.iter()])

ET.register_namespace("myp", "http://custom.example/ns")
ns_el = ET.Element("{http://custom.example/ns}root")
ET.SubElement(ns_el, "{http://custom.example/ns}leaf")
print("reg_ns_ser:", ET.tostring(ns_el, encoding="unicode"))
print("default_ns_ser:", ET.tostring(ns_el, encoding="unicode", default_namespace="http://custom.example/ns"))

try:
    ET.register_namespace("ns42", "http://bad.example")
except ValueError:
    print("reserved_ns_prefix: ValueError")

try:
    ET.tostring(ET.Element("plain"), encoding="unicode", default_namespace="http://example.com")
except ValueError:
    print("default_ns_unqualified: ValueError")

# ── 7. ElementPath (XPath subset) ─────────────────────────────────────────────
xdoc = ET.fromstring(
    '<catalog>'
    '  <book id="1" lang="en"><title>Alpha</title><author>Ann</author><price>10</price></book>'
    '  <book id="2" lang="fr"><title>Beta</title><author>Bob</author><price>20</price></book>'
    '  <book id="3" lang="en"><title>Gamma</title><price>30</price></book>'
    '  <magazine id="4"><title>Delta</title></magazine>'
    '</catalog>'
)
print("xp_self:", xdoc.find(".").tag)
print("xp_star:", [e.tag for e in xdoc.findall("*")])
print("xp_child:", [e.get("id") for e in xdoc.findall("book")])
print("xp_path:", [e.text for e in xdoc.findall("book/title")])
print("xp_desc:", [e.text for e in xdoc.findall(".//title")])
print("xp_attr_pres:", [e.get("id") for e in xdoc.findall("book[@lang]")])
print("xp_attr_eq:", [e.get("id") for e in xdoc.findall("book[@lang='en']")])
print("xp_attr_dq:", [e.get("id") for e in xdoc.findall('book[@lang="fr"]')])
print("xp_attr_ne:", [e.get("id") for e in xdoc.findall("book[@lang!='en']")])
print("xp_tag_pres:", [e.get("id") for e in xdoc.findall("book[author]")])
print("xp_tag_eq:", [e.get("id") for e in xdoc.findall("book[title='Beta']")])
print("xp_dot_eq:", [e.text for e in xdoc.findall(".//title[.='Gamma']")])
print("xp_pos_1:", [e.get("id") for e in xdoc.findall("book[1]")])
print("xp_pos_2:", [e.get("id") for e in xdoc.findall("book[2]")])
print("xp_last:", [e.get("id") for e in xdoc.findall("book[last()]")])
print("xp_last_m1:", [e.get("id") for e in xdoc.findall("book[last()-1]")])
print("xp_star_pos:", [e.tag for e in xdoc.findall("*[4]")])
print("xp_chained:", [e.get("id") for e in xdoc.findall("book[@lang='en'][2]")])
print("xp_parent:", [e.get("id") for e in xdoc.findall("book[title='Alpha']/title/..")])
print("xp_findtext:", xdoc.findtext("book[@id='2']/title"), xdoc.findtext("missing", "N/A"))
print("xp_iterfind:", [e.get("id") for e in xdoc.iterfind("book")])

# XPath with namespaces
ns_xdoc = ET.fromstring(
    '<r xmlns="urn:def" xmlns:a="urn:a" xmlns:b="urn:b">'
    '<x id="1"/><a:x id="2"><y>ok</y></a:x><b:x id="3"/>'
    '</r>'
)
ns_map = {"": "urn:def", "p": "urn:a", "q": "urn:b"}
print("xp_ns_def:", [e.get("id") for e in ns_xdoc.findall("x", ns_map)])
print("xp_ns_p:", ns_xdoc.findtext("p:x/y", namespaces=ns_map))
print("xp_ns_wild_local:", [e.get("id") for e in ns_xdoc.findall("{*}x")])
print("xp_ns_wild_uri:", [e.get("id") for e in ns_xdoc.findall("{urn:a}*")])
print("xp_ns_clark:", [e.get("id") for e in ns_xdoc.findall("{urn:b}x")])

# ElementTree find/findall/findtext with leading '/'
tree = ET.ElementTree(xdoc)
with warnings.catch_warnings():
    warnings.simplefilter("ignore", FutureWarning)
    print("tree_leading_slash:", tree.find("/book").get("id"), len(tree.findall("/book")))

# ElementPath SyntaxError cases
print("xp_no_ns_colon:", xdoc.find("unknown:tag"))
for bad_path, ns in [("/book", None), ("book[0]", None), ("book[]", None), ("unknown:tag", {"p": "urn:p"})]:
    try:
        xdoc.find(bad_path, ns)
    except SyntaxError:
        print("xp_err:", bad_path)

# ── 8. indent & canonicalize ──────────────────────────────────────────────────
ind_el = ET.fromstring("<a><b><c/></b><d>keep</d><e><f/></e></a>")
ET.indent(ind_el, space="  ", level=0)
print("indent_default:", repr(ET.tostring(ind_el, encoding="unicode")))

ind_tree = ET.ElementTree(ET.fromstring("<a><b/></a>"))
ET.indent(ind_tree, space="\t", level=1)
print("indent_tree:", repr(ET.tostring(ind_tree.getroot(), encoding="unicode")))

try:
    ET.indent(ET.Element("a"), level=-1)
except ValueError:
    print("indent_neg: ValueError")

c14n_in = '<r xmlns="http://a" xmlns:b="http://b" z="1" a="2"><!-- hi --><b:c>  txt  </b:c><d/></r>'
print("c14n_basic:", ET.canonicalize(c14n_in))
print("c14n_comments:", ET.canonicalize(c14n_in, with_comments=True))
print("c14n_strip:", ET.canonicalize(c14n_in, strip_text=True))
print("c14n_rewrite:", ET.canonicalize(c14n_in, rewrite_prefixes=True))
print("c14n_excl_attr:", ET.canonicalize(c14n_in, exclude_attrs={"z"}))
print("c14n_excl_tag:", ET.canonicalize(c14n_in, exclude_tags={"{http://a}d"}))

c14n_out = io.StringIO()
ret = ET.canonicalize("<a b='2' a='1'/>", out=c14n_out)
print("c14n_out:", ret, c14n_out.getvalue())

try:
    ET.canonicalize()
except ValueError:
    print("c14n_neither: ValueError")

print("c14n_xml_precedence:", ET.canonicalize("<a/>", from_file="nonexistent.xml"))

# C14NWriterTarget directly + ElementTree.write_c14n
c14n_buf = io.StringIO()
c14n_parser = ET.XMLParser(target=ET.C14NWriterTarget(c14n_buf.write, with_comments=True))
c14n_parser.feed('<r b="2" a="1"><!--note--><c/></r>')
c14n_parser.close()
print("c14n_target:", c14n_buf.getvalue())

# pickle round-trip of Element
import pickle
p_el = ET.fromstring('<r a="1">hi<c b="2">sub</c>tail</r>')
p_el2 = pickle.loads(pickle.dumps(p_el))
print("pickle_roundtrip:", ET.tostring(p_el2, encoding="unicode"))

# Element.__bool__ (with DeprecationWarning suppressed)
with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    e_empty = ET.Element("empty")
    e_nonempty = ET.Element("nonempty")
    ET.SubElement(e_nonempty, "c")
    print("elem_bool:", bool(e_empty), bool(e_nonempty))

# ── 9. parse, write, iterparse, XMLPullParser with BytesIO / StringIO / files ─
bio = io.BytesIO(b"<root><item>1</item><item>2</item></root>")
parsed_tree = ET.parse(bio)
print("parse_bytesio:", parsed_tree.getroot().tag, [e.text for e in parsed_tree.findall("item")])

sio = io.StringIO("<root><item>A</item></root>")
parsed_tree2 = ET.parse(sio)
print("parse_stringio:", parsed_tree2.findtext("item"))

# iterparse default and full events
ip_xml = '<root xmlns="http://def" xmlns:p="http://p"><!--cm--><?pi data?><p:x>hi</p:x></root>'
ip_default = [(ev, el.tag) for ev, el in ET.iterparse(io.StringIO(ip_xml))]
print("iterparse_default:", ip_default)

ip_all = [
    (ev, (el.tag if isinstance(el.tag, str) else el.tag.__name__, el.text) if ET.iselement(el) else el)
    for ev, el in ET.iterparse(io.StringIO(ip_xml), events=("start", "end", "start-ns", "end-ns", "comment", "pi"))
]
print("iterparse_all:", ip_all)

try:
    list(ET.iterparse(io.StringIO("<a/>"), events=("bad-event",)))
except ValueError:
    print("iterparse_bad_event: ValueError")

# XMLPullParser incremental chunks
pp = ET.XMLPullParser(["start", "end", "comment", "pi"])
pp.feed("<r><!--c--><?p d?><a")
ev1 = [(ev, el.tag if isinstance(el.tag, str) else el.tag.__name__, el.text) for ev, el in pp.read_events()]
pp.feed(">val</a></r>")
pp.close()
ev2 = [(ev, el.tag if isinstance(el.tag, str) else el.tag.__name__, el.text) for ev, el in pp.read_events()]
print("pullparser:", ev1, ev2)

# Tempfile round-trip with str path and pathlib.Path
with tempfile.TemporaryDirectory() as tmpdir:
    str_path = os.path.join(tmpdir, "doc.xml")
    path_obj = pathlib.Path(tmpdir) / "doc2.xml"
    t_out = ET.ElementTree(ET.fromstring("<doc><v>café</v></doc>"))
    t_out.write(str_path, encoding="utf-8", xml_declaration=True)
    t_out.write(path_obj, encoding="unicode")

    t_in1 = ET.parse(str_path)
    t_in2 = ET.parse(path_obj)
    print("file_roundtrip:", t_in1.findtext("v"), t_in2.findtext("v"))

    # canonicalize from_file
    print("c14n_from_file:", ET.canonicalize(from_file=path_obj))

    # iterparse from file path
    ip_file = [(ev, el.tag) for ev, el in ET.iterparse(str_path)]
    print("iterparse_file:", ip_file)

# ── 10. ParseError cases ──────────────────────────────────────────────────────
print("parseerror_sub:", issubclass(ET.ParseError, SyntaxError))
bad_xml_cases = [
    ("empty", ""),
    ("ws_only", "   \n  "),
    ("mismatched", "<a>\n  <b></a>"),
    ("unclosed", "<a><b></a>"),
    ("unclosed_eof", "<a>"),
    ("two_roots", "<a/><b/>"),
    ("junk_after", "<a/>junk"),
    ("junk_before", "junk<a/>"),
    ("dup_attr", '<a x="1" x="2"/>'),
    ("dup_ns_attr", '<a xmlns:p="urn:1" xmlns:q="urn:1" p:x="1" q:x="2"/>'),
    ("unbound_prefix", "<p:a/>"),
    ("unbound_attr_prefix", '<a p:x="1"/>'),
    ("bad_entity", "<a>&nope;</a>"),
    ("bare_amp", "<a>a & b</a>"),
    ("bad_charref_zero", "<a>&#0;</a>"),
    ("bad_comment_dd", "<a><!-- a -- b --></a>"),
    ("unclosed_comment", "<a><!-- oops</a>"),
    ("unclosed_cdata", "<a><![CDATA[oops</a>"),
    ("cdata_end_in_text", "<a>]]></a>"),
    ("lt_in_attr", '<a x="a<b"/>'),
    ("decl_not_start", " <?xml version='1.0'?><a/>"),
    ("unclosed_tag_tok", "<a"),
]
for label, bad in bad_xml_cases:
    try:
        ET.fromstring(bad)
        print("FAIL_NO_ERROR:", label)
    except ET.ParseError as exc:
        ok_types = isinstance(exc.code, int) and isinstance(exc.position, tuple) and len(exc.position) == 2
        ok_pos = isinstance(exc.position[0], int) and isinstance(exc.position[1], int) and exc.position[0] >= 1 and exc.position[1] >= 0
        print("parse_err:", label, ok_types, ok_pos)
