"""CPython 3.12.3 parity tests for `xml.etree.ElementInclude` and `xml.etree.cElementTree` (#3260)."""

import os
import tempfile
import xml.etree.ElementInclude as EI
import xml.etree.ElementTree as ET
import xml.etree.cElementTree as cET


# ── cElementTree parity & identity ───────────────────────────────────────────
for name in [
    "Element",
    "SubElement",
    "ElementTree",
    "Comment",
    "ProcessingInstruction",
    "PI",
    "QName",
    "ParseError",
    "TreeBuilder",
    "XMLParser",
    "XMLPullParser",
    "fromstring",
    "tostring",
    "parse",
    "iterparse",
    "iselement",
    "indent",
    "canonicalize",
]:
    assert getattr(cET, name) is getattr(ET, name), f"mismatch for {name}"

elem = cET.fromstring('<root a="1"><child>ok</child></root>')
assert cET.iselement(elem) is True
assert elem.findtext("child") == "ok"

# ── ElementInclude constants & exceptions ────────────────────────────────────
assert EI.XINCLUDE == "{http://www.w3.org/2001/XInclude}"
assert EI.XINCLUDE_INCLUDE == "{http://www.w3.org/2001/XInclude}include"
assert EI.XINCLUDE_FALLBACK == "{http://www.w3.org/2001/XInclude}fallback"
assert EI.DEFAULT_MAX_INCLUSION_DEPTH == 6
assert issubclass(EI.FatalIncludeError, SyntaxError)
assert issubclass(EI.LimitedRecursiveIncludeError, EI.FatalIncludeError)

# ── Custom loader: XML include, text include, fallback, recursion limit ──────
RESOURCES = {
    "a.xml": '<sub xmlns:xi="http://www.w3.org/2001/XInclude"><item>A</item><xi:include href="b.xml"/></sub>',
    "b.xml": "<leaf>B</leaf>",
    "c.xml": '<c xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="d.xml"/></c>',
    "d.xml": '<d xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="e.xml"/></d>',
    "e.xml": "<e>deep</e>",
    "note.txt": "plain text content",
    "loop.xml": '<loop xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="loop.xml"/></loop>',
}


def custom_loader(href, parse, encoding=None):
    if href not in RESOURCES:
        return None
    if parse == "xml":
        return ET.fromstring(RESOURCES[href])
    return RESOURCES[href]


root = ET.fromstring(
    '<doc xmlns:xi="http://www.w3.org/2001/XInclude">'
    'before'
    '<xi:include href="a.xml"/>'
    ' mid '
    '<xi:include href="note.txt" parse="text"/>'
    ' after'
    '</doc>'
)
EI.include(root, loader=custom_loader)
assert root.text == "before"
assert len(root) == 1
assert root[0].tag == "sub"
assert root[0][0].text == "A"
assert root[0][1].tag == "leaf" and root[0][1].text == "B"
assert root[0].tail == " mid plain text content after"

# Loader returning None -> FatalIncludeError
missing_root = ET.fromstring(
    '<doc xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="missing.xml"/></doc>'
)
try:
    EI.include(missing_root, loader=custom_loader)
    assert False, "expected FatalIncludeError"
except EI.FatalIncludeError:
    pass

# Direct recursive cycle -> FatalIncludeError
loop_root = ET.fromstring(
    '<doc xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="loop.xml"/></doc>'
)
try:
    EI.include(loop_root, loader=custom_loader)
    assert False, "expected FatalIncludeError"
except EI.FatalIncludeError:
    pass

# Max inclusion depth limit -> LimitedRecursiveIncludeError
deep_root = ET.fromstring(
    '<doc xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="c.xml"/></doc>'
)
try:
    EI.include(deep_root, loader=custom_loader, max_depth=2)
    assert False, "expected LimitedRecursiveIncludeError"
except EI.LimitedRecursiveIncludeError:
    pass

# Negative max_depth -> ValueError
try:
    EI.include(ET.Element("doc"), max_depth=-1)
    assert False, "expected ValueError"
except ValueError:
    pass

# Invalid parse mode -> FatalIncludeError
bad_parse = ET.fromstring(
    '<doc xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="a.xml" parse="binary"/></doc>'
)
try:
    EI.include(bad_parse, loader=custom_loader)
    assert False, "expected FatalIncludeError"
except EI.FatalIncludeError:
    pass

# Stray xi:fallback -> FatalIncludeError
stray_fb = ET.fromstring(
    '<doc xmlns:xi="http://www.w3.org/2001/XInclude"><xi:fallback/></doc>'
)
try:
    EI.include(stray_fb, loader=custom_loader)
    assert False, "expected FatalIncludeError"
except EI.FatalIncludeError:
    pass

# default_loader with real temp files
with tempfile.TemporaryDirectory() as tmpdir:
    inc_xml = os.path.join(tmpdir, "part.xml")
    inc_txt = os.path.join(tmpdir, "part.txt")
    with open(inc_xml, "w", encoding="utf-8") as f:
        f.write("<part>from-disk</part>")
    with open(inc_txt, "w", encoding="utf-8") as f:
        f.write("hello-disk")

    loaded_xml = EI.default_loader(inc_xml, "xml")
    assert loaded_xml.tag == "part" and loaded_xml.text == "from-disk"
    loaded_txt = EI.default_loader(inc_txt, "text", "utf-8")
    assert loaded_txt == "hello-disk"

print("xml.etree extra OK")
