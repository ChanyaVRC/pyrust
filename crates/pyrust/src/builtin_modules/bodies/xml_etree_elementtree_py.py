"""
Lightweight XML support for Python (xml.etree.ElementTree).
"""

import collections
import copy
import io
import re
import sys
import warnings

from xml.etree import ElementPath

__package__ = "xml.etree"

__all__ = [
    "Comment",
    "dump",
    "Element",
    "ElementTree",
    "fromstring",
    "fromstringlist",
    "indent",
    "iselement",
    "iterparse",
    "parse",
    "ParseError",
    "PI",
    "ProcessingInstruction",
    "QName",
    "SubElement",
    "tostring",
    "tostringlist",
    "TreeBuilder",
    "VERSION",
    "XML",
    "XMLID",
    "XMLParser",
    "XMLPullParser",
    "register_namespace",
    "canonicalize",
    "C14NWriterTarget",
]

VERSION = "1.3.0"

HTML_EMPTY = {
    "area",
    "base",
    "basefont",
    "br",
    "col",
    "embed",
    "frame",
    "hr",
    "img",
    "input",
    "isindex",
    "link",
    "meta",
    "param",
    "source",
    "track",
    "wbr",
}

_XML_NAMESPACE = "http://www.w3.org/XML/1998/namespace"
_XMLNS_NAMESPACE = "http://www.w3.org/2000/xmlns/"

_namespace_map = {
    "http://www.w3.org/XML/1998/namespace": "xml",
    "http://www.w3.org/1999/xhtml": "html",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#": "rdf",
    "http://schemas.xmlsoap.org/wsdl/": "wsdl",
    "http://www.w3.org/2001/XMLSchema": "xs",
    "http://www.w3.org/2001/XMLSchema-instance": "xsi",
    "http://purl.org/dc/elements/1.1/": "dc",
}


class ParseError(SyntaxError):
    """XML parse error with .code and .position = (line, column)."""
    pass


def iselement(element):
    """Return True if *element* appears to be an Element."""
    return hasattr(element, "tag")


def _assert_is_element(e):
    if not isinstance(e, Element):
        raise TypeError("expected an Element, not %s" % type(e).__name__)


def _checkpath(tag):
    """``checkpath()`` from ``_elementtree.c``: whether *tag* may be an
    ElementPath expression rather than a plain tag."""
    if isinstance(tag, str):
        brace, close, star = "{", "}", "*"
        pathchars = ("/", "*", "[", "@", ".")
    elif isinstance(tag, bytes):
        brace, close, star = b"{", b"}", b"*"
        pathchars = (b"/", b"*", b"[", b"@", b".")
    else:
        return True  # unknown type; might be path expression
    if len(tag) >= 3 and tag[:1] == brace and (
            tag[1:2] == close or (tag[1:2] == star and tag[2:3] == close)):
        return True  # wildcard: '{}tag' or '{*}tag'
    # Path characters only count outside '{...}' namespace sections.
    start = 0
    while True:
        end = tag.find(brace, start)
        segment = tag[start:] if end < 0 else tag[start:end]
        for ch in pathchars:
            if ch in segment:
                return True
        if end < 0:
            return False
        start = tag.find(close, end + 1)
        if start < 0:
            return False
        start += 1


class Element:
    """An XML element."""

    def __init__(self, tag, attrib={}, **extra):
        if not isinstance(attrib, dict):
            raise TypeError("attrib must be dict, not %s" % (type(attrib).__name__,))
        self.tag = tag
        self.attrib = attrib.copy()
        if extra:
            self.attrib.update(extra)
        self._children = []
        self.text = None
        self.tail = None

    def __repr__(self):
        return "<%s %r at %#x>" % (self.__class__.__name__, self.tag, id(self))

    def makeelement(self, tag, attrib):
        """Create a new element with the same type."""
        return self.__class__(tag, attrib)

    def __copy__(self):
        elem = self.makeelement(self.tag, {})
        elem.attrib = self.attrib
        elem.text = self.text
        elem.tail = self.tail
        elem._children = list(self._children)
        return elem

    def __deepcopy__(self, memo):
        elem = self.makeelement(self.tag, copy.deepcopy(self.attrib, memo))
        memo[id(self)] = elem
        elem.text = copy.deepcopy(self.text, memo)
        elem.tail = copy.deepcopy(self.tail, memo)
        elem._children = [copy.deepcopy(c, memo) for c in self._children]
        return elem

    def __getstate__(self):
        return (self.tag, self.attrib, self._children, self.text, self.tail)

    def __setstate__(self, state):
        tag, attrib, children, text, tail = state
        self.tag = tag
        self.attrib = attrib if attrib is not None else {}
        self._children = list(children) if children is not None else []
        self.text = text
        self.tail = tail

    def __len__(self):
        return len(self._children)

    def __bool__(self):
        warnings.warn(
            "Testing an element's truth value will always return True in future "
            "versions.  Use specific 'len(elem)' or 'elem is not None' test instead.",
            DeprecationWarning,
            stacklevel=2,
        )
        return len(self._children) != 0

    def __getitem__(self, index):
        return self._children[index]

    def __setitem__(self, index, element):
        if isinstance(index, slice):
            elements = list(element)
            for elt in elements:
                _assert_is_element(elt)
            self._children[index] = elements
        else:
            _assert_is_element(element)
            self._children[index] = element

    def __delitem__(self, index):
        del self._children[index]

    def __iter__(self):
        return iter(self._children)

    def append(self, subelement):
        """Add *subelement* to the end of this element."""
        _assert_is_element(subelement)
        self._children.append(subelement)

    def extend(self, elements):
        """Append subelements from a sequence."""
        if elements is self:
            elements = list(self._children)
        for element in elements:
            _assert_is_element(element)
            self._children.append(element)

    def insert(self, index, subelement):
        """Insert *subelement* at position *index*."""
        _assert_is_element(subelement)
        self._children.insert(index, subelement)

    def remove(self, subelement):
        """Remove matching subelement (by identity)."""
        self._children.remove(subelement)

    # Like the C accelerator, a plain tag (``_checkpath`` false) with no
    # namespace map is matched against the children directly; anything else
    # goes through ElementPath.

    def find(self, path, namespaces=None):
        """Find first matching element by tag name or path."""
        if namespaces is None and not _checkpath(path):
            for e in self._children:
                t = e.tag
                if t is path or t == path:
                    return e
            return None
        return ElementPath.find(self, path, namespaces)

    def findtext(self, path, default=None, namespaces=None):
        """Find text for first matching element by tag name or path."""
        if namespaces is None and not _checkpath(path):
            for e in self._children:
                t = e.tag
                if t is path or t == path:
                    text = e.text
                    return "" if text is None else text
            return default
        return ElementPath.findtext(self, path, default, namespaces)

    def findall(self, path, namespaces=None):
        """Find all matching subelements by tag name or path."""
        if namespaces is None and not _checkpath(path):
            return [e for e in self._children if e.tag is path or e.tag == path]
        return ElementPath.findall(self, path, namespaces)

    def iterfind(self, path, namespaces=None):
        """Find all matching subelements by tag name or path as an iterator."""
        return ElementPath.iterfind(self, path, namespaces)

    def clear(self):
        """Reset element: remove all subelements, clear attributes, reset text/tail."""
        self.attrib.clear()
        self._children = []
        self.text = None
        self.tail = None

    def get(self, key, default=None):
        """Get element attribute."""
        return self.attrib.get(key, default)

    def set(self, key, value):
        """Set element attribute."""
        self.attrib[key] = value

    def keys(self):
        """Get list of attribute names."""
        return list(self.attrib.keys())

    def items(self):
        """Get list of (name, value) attribute pairs."""
        return list(self.attrib.items())

    def iter(self, tag=None):
        """Create tree iterator in document order."""
        if tag == "*":
            tag = None
        if tag is None or self.tag == tag:
            yield self
        for e in self._children:
            yield from e.iter(tag)

    def itertext(self):
        """Create text iterator in document order."""
        t = self.text
        if t:
            yield t
        for e in self._children:
            yield from e.itertext()
            t = e.tail
            if t:
                yield t


def SubElement(parent, tag, attrib={}, **extra):
    """Subelement factory which creates an element instance and appends it to parent."""
    _assert_is_element(parent)
    if not isinstance(attrib, dict):
        raise TypeError("attrib must be dict, not %s" % (type(attrib).__name__,))
    attrib = attrib.copy()
    if extra:
        attrib.update(extra)
    element = parent.makeelement(tag, attrib)
    parent.append(element)
    return element


def Comment(text=None):
    """Comment element factory."""
    element = Element(Comment)
    element.text = text
    return element


def ProcessingInstruction(target, text=None):
    """Processing Instruction element factory."""
    element = Element(ProcessingInstruction)
    element.text = target
    if text:
        element.text = element.text + " " + text
    return element


PI = ProcessingInstruction


class QName:
    """Qualified name wrapper."""

    def __init__(self, text_or_uri, tag=None):
        if tag:
            text_or_uri = "{%s}%s" % (text_or_uri, tag)
        self.text = text_or_uri

    def __str__(self):
        return self.text

    def __repr__(self):
        return "<%s %r>" % (self.__class__.__name__, self.text)

    def __hash__(self):
        return hash(self.text)

    def __le__(self, other):
        if isinstance(other, QName):
            return self.text <= other.text
        if isinstance(other, str):
            return self.text <= other
        return NotImplemented

    def __lt__(self, other):
        if isinstance(other, QName):
            return self.text < other.text
        if isinstance(other, str):
            return self.text < other
        return NotImplemented

    def __ge__(self, other):
        if isinstance(other, QName):
            return self.text >= other.text
        if isinstance(other, str):
            return self.text >= other
        return NotImplemented

    def __gt__(self, other):
        if isinstance(other, QName):
            return self.text > other.text
        if isinstance(other, str):
            return self.text > other
        return NotImplemented

    def __eq__(self, other):
        if isinstance(other, QName):
            return self.text == other.text
        if isinstance(other, str):
            return self.text == other
        return NotImplemented


def register_namespace(prefix, uri):
    """Register a namespace prefix."""
    if prefix.startswith("ns") and prefix[2:].isdigit():
        raise ValueError("Prefix format reserved for internal use")
    for k, v in list(_namespace_map.items()):
        if k == uri or v == prefix:
            del _namespace_map[k]
    _namespace_map[uri] = prefix


def _raise_serialization_error(text):
    raise TypeError(
        "cannot serialize %r (type %s)" % (text, type(text).__name__)
    )


def _escape_cdata(text):
    try:
        if "&" in text:
            text = text.replace("&", "&amp;")
        if "<" in text:
            text = text.replace("<", "&lt;")
        if ">" in text:
            text = text.replace(">", "&gt;")
        return text
    except (TypeError, AttributeError):
        _raise_serialization_error(text)


def _escape_attrib(text):
    try:
        if "&" in text:
            text = text.replace("&", "&amp;")
        if "<" in text:
            text = text.replace("<", "&lt;")
        if ">" in text:
            text = text.replace(">", "&gt;")
        if '"' in text:
            text = text.replace('"', "&quot;")
        if "\r\n" in text:
            text = text.replace("\r\n", "\n")
        if "\r" in text:
            text = text.replace("\r", "\n")
        if "\n" in text:
            text = text.replace("\n", "&#10;")
        if "\t" in text:
            text = text.replace("\t", "&#09;")
        return text
    except (TypeError, AttributeError):
        _raise_serialization_error(text)


def _escape_attrib_xml(text):
    try:
        if "&" in text:
            text = text.replace("&", "&amp;")
        if "<" in text:
            text = text.replace("<", "&lt;")
        if ">" in text:
            text = text.replace(">", "&gt;")
        if '"' in text:
            text = text.replace('"', "&quot;")
        if "\r" in text:
            text = text.replace("\r", "&#13;")
        if "\n" in text:
            text = text.replace("\n", "&#10;")
        if "\t" in text:
            text = text.replace("\t", "&#09;")
        return text
    except (TypeError, AttributeError):
        _raise_serialization_error(text)


def _escape_attrib_html(text):
    try:
        if "&" in text:
            text = text.replace("&", "&amp;")
        if ">" in text:
            text = text.replace(">", "&gt;")
        if '"' in text:
            text = text.replace('"', "&quot;")
        return text
    except (TypeError, AttributeError):
        _raise_serialization_error(text)


def _namespaces(elem, default_namespace=None):
    """Identify namespaces used in this tree and assign qualified names."""
    qnames = {None: None}
    namespaces = {}
    if default_namespace:
        namespaces[default_namespace] = ""

    def add_qname(qname):
        try:
            if qname[:1] == "{":
                uri, tag = qname[1:].rsplit("}", 1)
                prefix = namespaces.get(uri)
                if prefix is None:
                    prefix = _namespace_map.get(uri)
                    if prefix is None:
                        prefix = "ns%d" % len(namespaces)
                    if prefix != "xml":
                        namespaces[uri] = prefix
                if prefix:
                    qnames[qname] = "%s:%s" % (prefix, tag)
                else:
                    qnames[qname] = tag
            else:
                if default_namespace:
                    raise ValueError(
                        "cannot use non-qualified names with "
                        "default_namespace option"
                    )
                qnames[qname] = qname
        except TypeError:
            _raise_serialization_error(qname)

    def add_attr_qname(qname):
        try:
            if qname[:1] == "{":
                uri, tag = qname[1:].rsplit("}", 1)
                prefix = namespaces.get(uri)
                if prefix is None or prefix == "":
                    # Attributes cannot use the default namespace.
                    if prefix == "":
                        # Need an explicit prefix for this URI on attributes.
                        existing = None
                        for u, p in namespaces.items():
                            if u == uri and p:
                                existing = p
                                break
                        prefix = existing
                    if prefix is None:
                        prefix = _namespace_map.get(uri)
                        if prefix is None:
                            prefix = "ns%d" % len(namespaces)
                        if prefix != "xml":
                            namespaces[uri] = prefix
                if prefix:
                    qnames[qname] = "%s:%s" % (prefix, tag)
                else:
                    qnames[qname] = tag
            else:
                qnames[qname] = qname
        except TypeError:
            _raise_serialization_error(qname)

    for el in elem.iter():
        tag = el.tag
        if isinstance(tag, QName):
            if tag.text not in qnames:
                add_qname(tag.text)
        elif isinstance(tag, str):
            if tag not in qnames:
                add_qname(tag)
        elif tag is not None and tag is not Comment and tag is not PI:
            _raise_serialization_error(tag)
        for key, value in el.items():
            if isinstance(key, QName):
                key = key.text
            if key not in qnames:
                add_attr_qname(key)
            if isinstance(value, QName) and value.text not in qnames:
                add_qname(value.text)

    return qnames, namespaces


def _serialize_xml(write, elem, qnames, namespaces, short_empty_elements):
    tag = elem.tag
    text = elem.text
    if tag is Comment:
        write("<!--%s-->" % (text or ""))
    elif tag is ProcessingInstruction:
        write("<?%s?>" % (text or ""))
    else:
        if isinstance(tag, QName):
            tag = tag.text
        tag = qnames[tag]
        if tag is None:
            if text:
                write(_escape_cdata(text))
            for e in elem:
                _serialize_xml(write, e, qnames, None, short_empty_elements)
        else:
            write("<" + tag)
            items = list(elem.items())
            if items or namespaces:
                if namespaces:
                    for v, k in sorted(namespaces.items(), key=lambda x: x[1]):
                        if k:
                            k = ":" + k
                        write(' xmlns%s="%s"' % (k, _escape_attrib_xml(v)))
                for k, v in items:
                    if isinstance(k, QName):
                        k = k.text
                    if isinstance(v, QName):
                        v = qnames[v.text]
                    else:
                        v = _escape_attrib_xml(v)
                    write(' %s="%s"' % (qnames[k], v))
            if text or len(elem) or not short_empty_elements:
                write(">")
                if text:
                    write(_escape_cdata(text))
                for e in elem:
                    _serialize_xml(write, e, qnames, None, short_empty_elements)
                write("</" + tag + ">")
            else:
                write(" />")
    if elem.tail:
        write(_escape_cdata(elem.tail))


def _serialize_html(write, elem, qnames, namespaces, short_empty_elements):
    tag = elem.tag
    text = elem.text
    if tag is Comment:
        write("<!--%s-->" % _escape_cdata(text or ""))
    elif tag is ProcessingInstruction:
        write("<?%s?>" % _escape_cdata(text or ""))
    else:
        if isinstance(tag, QName):
            tag = tag.text
        tag = qnames[tag]
        if tag is None:
            if text:
                write(_escape_cdata(text))
            for e in elem:
                _serialize_html(write, e, qnames, None, short_empty_elements)
        else:
            write("<" + tag)
            items = list(elem.items())
            if items or namespaces:
                if namespaces:
                    for v, k in sorted(namespaces.items(), key=lambda x: x[1]):
                        if k:
                            k = ":" + k
                        write(' xmlns%s="%s"' % (k, _escape_attrib_html(v)))
                for k, v in items:
                    if isinstance(k, QName):
                        k = k.text
                    if isinstance(v, QName):
                        v = qnames[v.text]
                    else:
                        v = _escape_attrib_html(v)
                    write(' %s="%s"' % (qnames[k], v))
            write(">")
            ltag = tag.lower()
            if text:
                if ltag == "script" or ltag == "style":
                    write(text)
                else:
                    write(_escape_cdata(text))
            for e in elem:
                _serialize_html(write, e, qnames, None, short_empty_elements)
            if ltag not in HTML_EMPTY:
                write("</" + tag + ">")
    if elem.tail:
        write(_escape_cdata(elem.tail))


def _serialize_text(write, elem):
    for part in elem.itertext():
        write(part)
    if elem.tail:
        write(elem.tail)


def _write_serialized(elem, file_or_filename, encoding, xml_declaration,
                      default_namespace, method, short_empty_elements):
    if method is None:
        method = "xml"
    if method not in ("xml", "html", "text"):
        raise ValueError("unknown method %r" % method)
    if encoding is None:
        encoding = "us-ascii"
    if method == "html" and default_namespace:
        raise ValueError("cannot use default_namespace option with html method")

    is_unicode = encoding.lower() == "unicode"
    open_file = not hasattr(file_or_filename, "write")
    if open_file:
        if is_unicode:
            f = open(file_or_filename, "w", encoding="utf-8")
        else:
            f = open(file_or_filename, "wb")
    else:
        f = file_or_filename

    try:
        chunks = []
        if is_unicode:
            write = f.write
        else:
            write = chunks.append

        if method == "text":
            _serialize_text(write, elem)
        else:
            qnames, namespaces = _namespaces(elem, default_namespace)
            if method == "xml":
                if xml_declaration or (
                    xml_declaration is None
                    and encoding.lower() not in ("utf-8", "us-ascii", "unicode")
                ):
                    decl_enc = "utf-8" if is_unicode else encoding
                    write("<?xml version='1.0' encoding='%s'?>\n" % (decl_enc,))
                _serialize_xml(write, elem, qnames, namespaces, short_empty_elements)
            elif method == "html":
                _serialize_html(write, elem, qnames, namespaces, short_empty_elements)

        if not is_unicode and chunks:
            f.write("".join(chunks).encode(encoding, "xmlcharrefreplace"))
    finally:
        if open_file:
            f.close()


class ElementTree:
    """An XML element hierarchy."""

    def __init__(self, element=None, file=None):
        if element is not None:
            _assert_is_element(element)
        self._root = element
        if file:
            self.parse(file)

    def getroot(self):
        """Return root element of this tree."""
        return self._root

    def _setroot(self, element):
        """Replace root element of this tree."""
        _assert_is_element(element)
        self._root = element

    def parse(self, source, parser=None):
        """Load external XML document into element tree."""
        close_source = False
        if not hasattr(source, "read"):
            source = open(source, "rb")
            close_source = True
        try:
            if parser is None:
                parser = XMLParser()
            while True:
                data = source.read(65536)
                if not data:
                    break
                parser.feed(data)
            self._root = parser.close()
            return self._root
        finally:
            if close_source:
                source.close()

    def iter(self, tag=None):
        """Create and return tree iterator for the root element."""
        return self._root.iter(tag)

    def find(self, path, namespaces=None):
        """Find first matching element by tag name or path."""
        if path[:1] == "/":
            path = "." + path
            warnings.warn(
                "This search is broken in 1.3 and earlier, and will be "
                "fixed in a future version.  If you rely on the current "
                "behaviour, change it to %r" % path,
                FutureWarning,
                stacklevel=2,
            )
        return self._root.find(path, namespaces)

    def findtext(self, path, default=None, namespaces=None):
        """Find first matching element by tag name or path."""
        if path[:1] == "/":
            path = "." + path
            warnings.warn(
                "This search is broken in 1.3 and earlier, and will be "
                "fixed in a future version.  If you rely on the current "
                "behaviour, change it to %r" % path,
                FutureWarning,
                stacklevel=2,
            )
        return self._root.findtext(path, default, namespaces)

    def findall(self, path, namespaces=None):
        """Find all matching subelements by tag name or path."""
        if path[:1] == "/":
            path = "." + path
            warnings.warn(
                "This search is broken in 1.3 and earlier, and will be "
                "fixed in a future version.  If you rely on the current "
                "behaviour, change it to %r" % path,
                FutureWarning,
                stacklevel=2,
            )
        return self._root.findall(path, namespaces)

    def iterfind(self, path, namespaces=None):
        """Find all matching subelements by tag name or path."""
        if path[:1] == "/":
            path = "." + path
            warnings.warn(
                "This search is broken in 1.3 and earlier, and will be "
                "fixed in a future version.  If you rely on the current "
                "behaviour, change it to %r" % path,
                FutureWarning,
                stacklevel=2,
            )
        return self._root.iterfind(path, namespaces)

    def write(self, file_or_filename,
              encoding=None,
              xml_declaration=None,
              default_namespace=None,
              method=None, *,
              short_empty_elements=True):
        """Write element tree to a file as XML."""
        _write_serialized(
            self._root,
            file_or_filename,
            encoding,
            xml_declaration,
            default_namespace,
            method,
            short_empty_elements,
        )

    def write_c14n(self, file):
        """Legacy C14N write stub matching CPython 3.12."""
        return self.write(file, method="c14n")


def tostring(element, encoding=None, method=None, *,
             xml_declaration=None, default_namespace=None,
             short_empty_elements=True):
    """Generate string representation of XML element."""
    stream = io.StringIO() if encoding == "unicode" else io.BytesIO()
    ElementTree(element).write(
        stream,
        encoding,
        xml_declaration=xml_declaration,
        default_namespace=default_namespace,
        method=method,
        short_empty_elements=short_empty_elements,
    )
    return stream.getvalue()


class _ListDataStream(io.BufferedIOBase):
    """An auxiliary stream accumulating into a list reference."""

    def __init__(self, lst):
        self.lst = lst

    def writable(self):
        return True

    def write(self, b):
        self.lst.append(b)
        return len(b)


def tostringlist(element, encoding=None, method=None, *,
                 xml_declaration=None, default_namespace=None,
                 short_empty_elements=True):
    """Generate a list of encoded XML chunks for *element*."""
    lst = []
    stream = _ListDataStream(lst)
    ElementTree(element).write(
        stream,
        encoding,
        xml_declaration=xml_declaration,
        default_namespace=default_namespace,
        method=method,
        short_empty_elements=short_empty_elements,
    )
    return lst


def dump(elem):
    """Write element tree or element structure to sys.stdout."""
    if not isinstance(elem, ElementTree):
        elem = ElementTree(elem)
    elem.write(sys.stdout, encoding="unicode")
    tail = elem.getroot().tail
    if not tail or tail[-1] != "\n":
        sys.stdout.write("\n")


def indent(tree, space="  ", level=0):
    """Indent an XML document by inserting newlines and indentation space."""
    if isinstance(tree, ElementTree):
        tree = tree.getroot()
    if level < 0:
        raise ValueError(f"Initial indentation level must be >= 0, got {level}")
    if not len(tree):
        return

    indentations = ["\n" + level * space]

    def _indent_children(elem, level):
        child_level = level + 1
        try:
            child_indentation = indentations[child_level]
        except IndexError:
            child_indentation = indentations[level] + space
            indentations.append(child_indentation)

        if not elem.text or not elem.text.strip():
            elem.text = child_indentation

        for child in elem:
            if len(child):
                _indent_children(child, child_level)
            if not child.tail or not child.tail.strip():
                child.tail = child_indentation

        if not child.tail.strip():
            child.tail = indentations[level]

    _indent_children(tree, 0)


# ---------------------------------------------------------------------------
# TreeBuilder
# ---------------------------------------------------------------------------

class TreeBuilder:
    """Generic element structure builder."""

    def __init__(self, element_factory=None, *,
                 comment_factory=None, pi_factory=None,
                 insert_comments=False, insert_pis=False):
        self._data = []
        self._elem = []
        self._last = None
        self._root = None
        self._tail = None
        if comment_factory is None:
            comment_factory = Comment
        self._comment_factory = comment_factory
        self.insert_comments = insert_comments
        if pi_factory is None:
            pi_factory = ProcessingInstruction
        self._pi_factory = pi_factory
        self.insert_pis = insert_pis
        if element_factory is None:
            element_factory = Element
        self._factory = element_factory

    def close(self):
        """Flush builder buffers and return toplevel document element."""
        assert len(self._elem) == 0, "missing end tags"
        assert self._root is not None, "missing toplevel element"
        return self._root

    def _flush(self):
        if self._data:
            if self._last is not None:
                text = "".join(self._data)
                if self._tail:
                    assert self._last.tail is None, "internal error (tail)"
                    self._last.tail = text
                else:
                    assert self._last.text is None, "internal error (text)"
                    self._last.text = text
            self._data = []

    def data(self, text):
        """Add text to current element."""
        self._data.append(text)

    def start(self, tag, attrs):
        """Open new element and return it."""
        self._flush()
        self._last = elem = self._factory(tag, attrs)
        if self._elem:
            self._elem[-1].append(elem)
        elif self._root is None:
            self._root = elem
        self._elem.append(elem)
        self._tail = 0
        return elem

    def end(self, tag):
        """Close and return current element."""
        self._flush()
        self._last = self._elem.pop()
        assert self._last.tag == tag, (
            "end tag mismatch (expected %s, got %s)" % (self._last.tag, tag)
        )
        self._tail = 1
        return self._last

    def comment(self, text):
        """Create a comment using the comment_factory."""
        return self._handle_single(
            self._comment_factory, self.insert_comments, text
        )

    def pi(self, target, text=None):
        """Create a processing instruction using the pi_factory."""
        return self._handle_single(
            self._pi_factory, self.insert_pis, target, text
        )

    def _handle_single(self, factory, insert, *args):
        elem = factory(*args)
        if insert:
            self._flush()
            self._last = elem
            if self._elem:
                self._elem[-1].append(elem)
            self._tail = 1
        return elem


# ---------------------------------------------------------------------------
# Streaming XML 1.0 Parser
# ---------------------------------------------------------------------------

_XML_WHITESPACE = frozenset(" \t\r\n")
_BUILTIN_ENTITIES = {
    "lt": "<",
    "gt": ">",
    "amp": "&",
    "apos": "'",
    "quot": '"',
}

_DECL_ENC_RE = re.compile(
    r"""^<\?xml\s+version\s*=\s*['"][^'"]+['"](?:\s+encoding\s*=\s*['"]([^'"]+)['"])?"""
)


def _is_xml_char(cp):
    return (
        cp == 0x9
        or cp == 0xA
        or cp == 0xD
        or (0x20 <= cp <= 0xD7FF)
        or (0xE000 <= cp <= 0xFFFD)
        or (0x10000 <= cp <= 0x10FFFF)
    )


def _is_name_start_char(ch):
    o = ord(ch)
    if o < 128:
        return (
            (65 <= o <= 90)
            or (97 <= o <= 122)
            or ch == "_"
            or ch == ":"
        )
    return (
        (0xC0 <= o <= 0xD6)
        or (0xD8 <= o <= 0xF6)
        or (0xF8 <= o <= 0x2FF)
        or (0x370 <= o <= 0x37D)
        or (0x37F <= o <= 0x1FFF)
        or (0x200C <= o <= 0x200D)
        or (0x2070 <= o <= 0x218F)
        or (0x2C00 <= o <= 0x2FEF)
        or (0x3001 <= o <= 0xD7FF)
        or (0xF900 <= o <= 0xFDCF)
        or (0xFDF0 <= o <= 0xFFFD)
        or (0x10000 <= o <= 0xEFFFF)
    )


def _is_name_char(ch):
    o = ord(ch)
    if o < 128:
        return (
            (65 <= o <= 90)
            or (97 <= o <= 122)
            or (48 <= o <= 57)
            or ch == "_"
            or ch == ":"
            or ch == "-"
            or ch == "."
        )
    return (
        _is_name_start_char(ch)
        or o == 0xB7
        or (0x0300 <= o <= 0x036F)
        or (0x203F <= o <= 0x2040)
    )


def _is_valid_name(name):
    if not name or not _is_name_start_char(name[0]):
        return False
    for ch in name[1:]:
        if not _is_name_char(ch):
            return False
    return True


def _decode_bytes_incremental(raw, encoding, final):
    """Decode bytes using encoding, returning (decoded_str, leftover_bytes)."""
    if not raw:
        return "", b""
    if final:
        return raw.decode(encoding), b""
    enc_norm = encoding.lower().replace("_", "-")
    if enc_norm in ("utf-8", "utf8", "u8", "utf", "utf-8-sig"):
        # Hold up to 3 trailing incomplete UTF-8 bytes
        n = len(raw)
        cut = n
        # Walk back up to 3 bytes looking for a start byte of an incomplete sequence
        for back in range(1, min(4, n + 1)):
            b = raw[n - back]
            if b < 0x80:
                break
            if b >= 0xC0:
                if b < 0xE0:
                    expected = 2
                elif b < 0xF0:
                    expected = 3
                else:
                    expected = 4
                if back < expected:
                    cut = n - back
                break
        if cut == 0:
            return "", raw
        return raw[:cut].decode(encoding), raw[cut:]
    if enc_norm in ("utf-16", "utf16", "utf-16-le", "utf-16-be"):
        cut = len(raw) & ~1
        if cut >= 2:
            # Avoid splitting a surrogate pair
            if enc_norm == "utf-16-be":
                u = (raw[cut - 2] << 8) | raw[cut - 1]
            else:
                u = raw[cut - 2] | (raw[cut - 1] << 8)
            if 0xD800 <= u <= 0xDBFF:
                cut -= 2
        if cut <= 0:
            return "", raw
        return raw[:cut].decode(encoding), raw[cut:]
    if enc_norm in ("utf-32", "utf32", "utf-32-le", "utf-32-be"):
        cut = len(raw) & ~3
        if cut == 0:
            return "", raw
        return raw[:cut].decode(encoding), raw[cut:]
    # Single-byte codecs decode completely
    return raw.decode(encoding), b""


class XMLParser:
    """Streaming XML 1.0 parser building an element structure via *target*."""

    def __init__(self, *, target=None, encoding=None):
        if target is None:
            target = TreeBuilder()
        self.target = target
        self.entity = {}
        self.version = "Expat 2.6.0"

        self._explicit_encoding = encoding
        self._detected_encoding = encoding
        self._bom_checked = False
        self._byte_buf = b""
        self._str_buf = ""
        self._pending_cr = False

        # Line (1-based) and column (0-based) of the start of _str_buf
        self._line = 1
        self._col = 0
        self._at_doc_start = True

        # Document state
        self._root_seen = False
        self._root_closed = False
        self._doctype_seen = False
        self._closed = False

        # Open element stack: list of (raw_tag, expanded_tag, declared_prefixes_list)
        self._stack = []
        # Namespace stack: list of dicts mapping prefix -> uri
        self._ns_stack = [{"xml": _XML_NAMESPACE}]
        # Internal DOCTYPE entities
        self._doctype_entities = {}

        # Optional event queue for XMLPullParser / iterparse
        self._events_queue = None
        self._events_set = None

    def _setevents(self, events_queue, events_to_report=None):
        if events_to_report is None:
            events_to_report = ("end",)
        events_set = set()
        for event in events_to_report:
            if event not in ("start", "end", "start-ns", "end-ns", "comment", "pi"):
                raise ValueError("unknown event %r" % event)
            events_set.add(event)
        self._events_queue = events_queue
        self._events_set = events_set

    def _pos_at(self, buf, offset):
        """Return (line, col) for byte/char offset within buf."""
        segment = buf[:offset]
        nl = segment.count("\n")
        if nl == 0:
            return (self._line, self._col + offset)
        last_nl = segment.rfind("\n")
        return (self._line + nl, offset - last_nl - 1)

    def _advance_pos(self, segment):
        nl = segment.count("\n")
        if nl == 0:
            self._col += len(segment)
        else:
            self._line += nl
            self._col = len(segment) - segment.rfind("\n") - 1

    def _error(self, message, code, buf, offset):
        line, col = self._pos_at(buf, offset)
        err = ParseError("%s: line %d, column %d" % (message, line, col))
        err.code = code
        err.position = (line, col)
        raise err

    def _normalize_newlines(self, text, final):
        if self._pending_cr:
            self._pending_cr = False
            if text.startswith("\n"):
                text = text[1:]
        if not final and text.endswith("\r"):
            self._pending_cr = True
            text = text[:-1] + "\n"
        if "\r" in text:
            text = text.replace("\r\n", "\n").replace("\r", "\n")
        return text

    def _detect_encoding_and_decode(self, final):
        raw = self._byte_buf
        if not raw:
            return ""
        if self._detected_encoding is None:
            if not self._bom_checked:
                if len(raw) < 4 and not final:
                    return ""
                self._bom_checked = True
                if raw.startswith(b"\xff\xfe\x00\x00"):
                    self._detected_encoding = "utf-32-le"
                    raw = raw[4:]
                elif raw.startswith(b"\x00\x00\xfe\xff"):
                    self._detected_encoding = "utf-32-be"
                    raw = raw[4:]
                elif raw.startswith(b"\xef\xbb\xbf"):
                    self._detected_encoding = "utf-8"
                    raw = raw[3:]
                elif raw.startswith(b"\xff\xfe"):
                    self._detected_encoding = "utf-16-le"
                    raw = raw[2:]
                elif raw.startswith(b"\xfe\xff"):
                    self._detected_encoding = "utf-16-be"
                    raw = raw[2:]
                elif raw.startswith(b"\x00<\x00?"):
                    self._detected_encoding = "utf-16-be"
                elif raw.startswith(b"<\x00?\x00"):
                    self._detected_encoding = "utf-16-le"
                self._byte_buf = raw

            if self._detected_encoding is None:
                # Sniff <?xml ... encoding="..."?> in ASCII-compatible bytes
                if raw.startswith(b"<?xml"):
                    end_pi = raw.find(b"?>")
                    if end_pi == -1:
                        if not final and len(raw) < 256:
                            return ""
                        self._detected_encoding = "utf-8"
                    else:
                        head = raw[:end_pi + 2].decode("ascii", "replace")
                        m = _DECL_ENC_RE.match(head)
                        if m and m.group(1):
                            self._detected_encoding = m.group(1)
                        else:
                            self._detected_encoding = "utf-8"
                else:
                    if len(raw) < 5 and not final and b"<?xml".startswith(raw):
                        return ""
                    self._detected_encoding = "utf-8"

        decoded, leftover = _decode_bytes_incremental(
            self._byte_buf, self._detected_encoding, final
        )
        self._byte_buf = leftover
        return decoded

    def feed(self, data):
        """Feed encoded or text XML data to parser."""
        if self._closed:
            raise ParseError("parsing finished")
        if isinstance(data, str):
            if self._at_doc_start and not self._bom_checked:
                self._bom_checked = True
                if data.startswith("\ufeff"):
                    data = data[1:]
            chunk = data
        elif isinstance(data, (bytes, bytearray, memoryview)):
            self._byte_buf += bytes(data)
            chunk = self._detect_encoding_and_decode(False)
        else:
            raise TypeError(
                "a bytes-like object or str is required, not %r"
                % type(data).__name__
            )
        if not chunk:
            return
        chunk = self._normalize_newlines(chunk, False)
        self._str_buf += chunk
        self._parse_buffer(False)

    def close(self):
        """Finish feeding data to parser and return element structure."""
        if self._closed:
            return None
        self._closed = True
        if self._byte_buf:
            chunk = self._detect_encoding_and_decode(True)
            if chunk:
                chunk = self._normalize_newlines(chunk, True)
                self._str_buf += chunk
        elif self._pending_cr:
            self._pending_cr = False
        self._parse_buffer(True)
        if not self._root_seen:
            self._error("no element found", 3, self._str_buf, len(self._str_buf))
        if self._stack:
            self._error("no element found", 3, self._str_buf, len(self._str_buf))
        if hasattr(self.target, "close"):
            return self.target.close()
        return None

    def flush(self):
        """Flush buffered parser state (no-op for streaming parser)."""
        pass

    def _emit_data(self, text):
        if text and hasattr(self.target, "data"):
            self.target.data(text)

    def _emit_comment(self, text):
        elem = None
        if hasattr(self.target, "comment"):
            elem = self.target.comment(text)
        if self._events_set and "comment" in self._events_set:
            if elem is None:
                elem = Comment(text)
            self._events_queue.append(("comment", elem))

    def _emit_pi(self, target_name, data):
        elem = None
        if hasattr(self.target, "pi"):
            elem = self.target.pi(target_name, data)
        if self._events_set and "pi" in self._events_set:
            if elem is None:
                elem = ProcessingInstruction(target_name, data)
            self._events_queue.append(("pi", elem))

    def _resolve_ns_map(self, prefix):
        for scope in reversed(self._ns_stack):
            if prefix in scope:
                return scope[prefix]
        return None

    def _expand_tag(self, raw_name, buf, offset):
        if ":" in raw_name:
            prefix, local = raw_name.split(":", 1)
            if not prefix or not local or ":" in local:
                self._error("not well-formed (invalid token)", 4, buf, offset)
            if not _is_valid_name(prefix) or not _is_valid_name(local):
                self._error("not well-formed (invalid token)", 4, buf, offset)
            uri = self._resolve_ns_map(prefix)
            if uri is None:
                self._error("unbound prefix", 27, buf, offset)
            return "{%s}%s" % (uri, local)
        default_uri = self._resolve_ns_map("")
        if default_uri:
            return "{%s}%s" % (default_uri, raw_name)
        return raw_name

    def _expand_attr(self, raw_name, buf, offset):
        if ":" in raw_name:
            prefix, local = raw_name.split(":", 1)
            if not prefix or not local or ":" in local:
                self._error("not well-formed (invalid token)", 4, buf, offset)
            if not _is_valid_name(prefix) or not _is_valid_name(local):
                self._error("not well-formed (invalid token)", 4, buf, offset)
            uri = self._resolve_ns_map(prefix)
            if uri is None:
                self._error("unbound prefix", 27, buf, offset)
            return "{%s}%s" % (uri, local)
        return raw_name

    def _unescape(self, text, buf, base_offset, is_attr):
        if "&" not in text:
            if is_attr and ("\t" in text or "\n" in text or "\r" in text):
                text = text.replace("\t", " ").replace("\n", " ").replace("\r", " ")
            return text
        out = []
        pos = 0
        n = len(text)
        while pos < n:
            amp = text.find("&", pos)
            if amp == -1:
                seg = text[pos:]
                if is_attr and ("\t" in seg or "\n" in seg or "\r" in seg):
                    seg = seg.replace("\t", " ").replace("\n", " ").replace("\r", " ")
                out.append(seg)
                break
            if amp > pos:
                seg = text[pos:amp]
                if is_attr and ("\t" in seg or "\n" in seg or "\r" in seg):
                    seg = seg.replace("\t", " ").replace("\n", " ").replace("\r", " ")
                out.append(seg)
            semi = text.find(";", amp + 1)
            if semi == -1:
                self._error("not well-formed (invalid token)", 4, buf, base_offset + amp)
            ref = text[amp + 1:semi]
            if not ref:
                self._error("not well-formed (invalid token)", 4, buf, base_offset + amp)
            if ref[0] == "#":
                try:
                    if len(ref) >= 2 and ref[1] == "x":
                        hex_digits = ref[2:]
                        if not hex_digits:
                            raise ValueError
                        cp = int(hex_digits, 16)
                    elif len(ref) >= 2 and ref[1] == "X":
                        # XML 1.0 requires lowercase 'x' in &#x...;
                        raise ValueError
                    else:
                        dec_digits = ref[1:]
                        if not dec_digits or not dec_digits.isdigit():
                            raise ValueError
                        cp = int(dec_digits, 10)
                except ValueError:
                    self._error("not well-formed (invalid token)", 4, buf, base_offset + amp)
                if not _is_xml_char(cp):
                    self._error("reference to invalid character number", 14, buf, base_offset + amp)
                out.append(chr(cp))
            else:
                if not _is_valid_name(ref):
                    self._error("not well-formed (invalid token)", 4, buf, base_offset + amp)
                val = _BUILTIN_ENTITIES.get(ref)
                if val is None:
                    val = self._doctype_entities.get(ref)
                if val is None:
                    val = self.entity.get(ref)
                if val is None:
                    self._error("undefined entity", 11, buf, base_offset + amp)
                out.append(val)
            pos = semi + 1
        return "".join(out)

    def _parse_xml_decl(self, content, buf, offset):
        # content is inside <?xml ... ?> (after "xml")
        i = 0
        n = len(content)
        while i < n and content[i] in _XML_WHITESPACE:
            i += 1
        attrs = []
        while i < n:
            name_start = i
            while i < n and content[i] not in _XML_WHITESPACE and content[i] != "=":
                i += 1
            name = content[name_start:i]
            while i < n and content[i] in _XML_WHITESPACE:
                i += 1
            if i >= n or content[i] != "=":
                self._error("XML or text declaration not well-formed", 2, buf, offset)
            i += 1
            while i < n and content[i] in _XML_WHITESPACE:
                i += 1
            if i >= n or content[i] not in ("'", '"'):
                self._error("XML or text declaration not well-formed", 2, buf, offset)
            q = content[i]
            i += 1
            v_start = i
            v_end = content.find(q, v_start)
            if v_end == -1:
                self._error("XML or text declaration not well-formed", 2, buf, offset)
            val = content[v_start:v_end]
            attrs.append((name, val))
            i = v_end + 1
            if i < n and content[i] not in _XML_WHITESPACE:
                self._error("XML or text declaration not well-formed", 2, buf, offset)
            while i < n and content[i] in _XML_WHITESPACE:
                i += 1

        if not attrs or attrs[0][0] != "version":
            self._error("XML or text declaration not well-formed", 2, buf, offset)
        idx = 1
        if idx < len(attrs) and attrs[idx][0] == "encoding":
            idx += 1
        if idx < len(attrs) and attrs[idx][0] == "standalone":
            if attrs[idx][1] not in ("yes", "no"):
                self._error("XML or text declaration not well-formed", 2, buf, offset)
            idx += 1
        if idx != len(attrs):
            self._error("XML or text declaration not well-formed", 2, buf, offset)

    def _parse_doctype_internal_subset(self, subset):
        """Extract basic internal <!ENTITY name 'value'> declarations."""
        i = 0
        n = len(subset)
        while i < n:
            ent = subset.find("<!ENTITY", i)
            if ent == -1:
                break
            j = ent + 8
            if j >= n or subset[j] not in _XML_WHITESPACE:
                i = j
                continue
            while j < n and subset[j] in _XML_WHITESPACE:
                j += 1
            if j < n and subset[j] == "%":
                # Parameter entity; skip to '>'
                end = subset.find(">", j)
                i = n if end == -1 else end + 1
                continue
            name_start = j
            while j < n and subset[j] not in _XML_WHITESPACE:
                j += 1
            name = subset[name_start:j]
            while j < n and subset[j] in _XML_WHITESPACE:
                j += 1
            if j < n and subset[j] in ("'", '"'):
                q = subset[j]
                v_end = subset.find(q, j + 1)
                if v_end != -1:
                    val = subset[j + 1:v_end]
                    if name and name not in self._doctype_entities:
                        self._doctype_entities[name] = val
                    end = subset.find(">", v_end + 1)
                    i = n if end == -1 else end + 1
                    continue
            end = subset.find(">", j)
            i = n if end == -1 else end + 1

    def _parse_start_tag(self, buf, pos, final):
        """Parse <tag ...> or <tag .../>. Returns new pos, or None if incomplete."""
        n = len(buf)
        # Scan until unquoted '>'
        i = pos + 1
        in_quote = None
        while i < n:
            ch = buf[i]
            if in_quote:
                if ch == in_quote:
                    in_quote = None
                elif ch == "<":
                    self._error("not well-formed (invalid token)", 4, buf, i)
            else:
                if ch == "'" or ch == '"':
                    in_quote = ch
                elif ch == ">":
                    break
                elif ch == "<":
                    self._error("not well-formed (invalid token)", 4, buf, i)
            i += 1
        else:
            if final:
                self._error("unclosed token", 5, buf, pos)
            return None

        tag_end = i  # index of '>'
        if self._root_closed:
            self._error("junk after document element", 9, buf, pos)

        # Parse tag name
        j = pos + 1
        name_start = j
        while j < tag_end and buf[j] not in _XML_WHITESPACE and buf[j] != "/":
            j += 1
        raw_tag = buf[name_start:j]
        if not _is_valid_name(raw_tag):
            self._error("not well-formed (invalid token)", 4, buf, name_start)

        # Parse attributes
        raw_attrs = []
        raw_attr_names = set()
        empty = False
        while j < tag_end:
            had_ws = False
            while j < tag_end and buf[j] in _XML_WHITESPACE:
                had_ws = True
                j += 1
            if j >= tag_end:
                break
            if buf[j] == "/":
                if j + 1 != tag_end:
                    self._error("not well-formed (invalid token)", 4, buf, j)
                empty = True
                j += 1
                break
            if not had_ws:
                self._error("not well-formed (invalid token)", 4, buf, j)
            a_start = j
            while j < tag_end and buf[j] not in _XML_WHITESPACE and buf[j] not in "=/":
                j += 1
            attr_name = buf[a_start:j]
            if not _is_valid_name(attr_name):
                self._error("not well-formed (invalid token)", 4, buf, a_start)
            if attr_name in raw_attr_names:
                self._error("duplicate attribute", 8, buf, a_start)
            raw_attr_names.add(attr_name)
            while j < tag_end and buf[j] in _XML_WHITESPACE:
                j += 1
            if j >= tag_end or buf[j] != "=":
                self._error("not well-formed (invalid token)", 4, buf, j)
            j += 1
            while j < tag_end and buf[j] in _XML_WHITESPACE:
                j += 1
            if j >= tag_end or buf[j] not in ("'", '"'):
                self._error("not well-formed (invalid token)", 4, buf, j)
            q = buf[j]
            j += 1
            v_start = j
            v_end = buf.find(q, v_start, tag_end)
            if v_end == -1:
                self._error("not well-formed (invalid token)", 4, buf, v_start)
            raw_val = buf[v_start:v_end]
            val = self._unescape(raw_val, buf, v_start, True)
            raw_attrs.append((attr_name, val, a_start))
            j = v_end + 1

        # Process namespace declarations first
        ns_scope = {}
        declared_prefixes = []
        non_ns_attrs = []
        for attr_name, val, a_offset in raw_attrs:
            if attr_name == "xmlns":
                ns_scope[""] = val
                declared_prefixes.append(("", val))
            elif attr_name.startswith("xmlns:"):
                prefix = attr_name[6:]
                if not prefix or ":" in prefix or not _is_valid_name(prefix):
                    self._error("not well-formed (invalid token)", 4, buf, a_offset)
                if prefix == "xmlns":
                    self._error("illegal namespace prefix", 2, buf, a_offset)
                if prefix == "xml" and val != _XML_NAMESPACE:
                    self._error("illegal namespace declaration", 2, buf, a_offset)
                if not val:
                    self._error("syntax error", 2, buf, a_offset)
                ns_scope[prefix] = val
                declared_prefixes.append((prefix, val))
            else:
                non_ns_attrs.append((attr_name, val, a_offset))

        self._ns_stack.append(ns_scope)

        # Emit start-ns events before start
        for prefix, uri in declared_prefixes:
            if hasattr(self.target, "start_ns"):
                self.target.start_ns(prefix, uri)
            if self._events_set and "start-ns" in self._events_set:
                self._events_queue.append(("start-ns", (prefix, uri)))

        # Expand element tag and attributes
        expanded_tag = self._expand_tag(raw_tag, buf, name_start)
        attrib = {}
        for attr_name, val, a_offset in non_ns_attrs:
            expanded_attr = self._expand_attr(attr_name, buf, a_offset)
            if expanded_attr in attrib:
                self._error("duplicate attribute", 8, buf, a_offset)
            attrib[expanded_attr] = val

        self._root_seen = True
        elem = self.target.start(expanded_tag, attrib)
        if self._events_set and "start" in self._events_set:
            self._events_queue.append(("start", elem))

        if empty:
            end_elem = self.target.end(expanded_tag)
            if self._events_set and "end" in self._events_set:
                self._events_queue.append(("end", end_elem))
            self._ns_stack.pop()
            for prefix, _uri in reversed(declared_prefixes):
                if hasattr(self.target, "end_ns"):
                    self.target.end_ns(prefix)
                if self._events_set and "end-ns" in self._events_set:
                    self._events_queue.append(("end-ns", None))
            if not self._stack:
                self._root_closed = True
        else:
            self._stack.append((raw_tag, expanded_tag, declared_prefixes))

        return tag_end + 1

    def _parse_end_tag(self, buf, pos, final):
        """Parse </tag>. Returns new pos, or None if incomplete."""
        gt = buf.find(">", pos + 2)
        if gt == -1:
            if final:
                self._error("unclosed token", 5, buf, pos)
            return None
        inner = buf[pos + 2:gt]
        raw_tag = inner.rstrip(" \t\r\n")
        if not raw_tag or not _is_valid_name(raw_tag):
            self._error("not well-formed (invalid token)", 4, buf, pos + 2)
        if not self._stack:
            self._error("mismatched tag", 7, buf, pos)
        expected_raw, expanded_tag, declared_prefixes = self._stack.pop()
        if raw_tag != expected_raw:
            self._error("mismatched tag", 7, buf, pos + 2)

        end_elem = self.target.end(expanded_tag)
        if self._events_set and "end" in self._events_set:
            self._events_queue.append(("end", end_elem))

        self._ns_stack.pop()
        for prefix, _uri in reversed(declared_prefixes):
            if hasattr(self.target, "end_ns"):
                self.target.end_ns(prefix)
            if self._events_set and "end-ns" in self._events_set:
                self._events_queue.append(("end-ns", None))

        if not self._stack:
            self._root_closed = True
        return gt + 1

    def _parse_buffer(self, final):
        buf = self._str_buf
        n = len(buf)
        pos = 0

        while pos < n:
            ch = buf[pos]
            if ch != "<":
                lt = buf.find("<", pos)
                if lt == -1:
                    if not final:
                        # Check for trailing incomplete entity reference '&...'
                        amp = buf.rfind("&", pos)
                        if amp != -1 and buf.find(";", amp) == -1:
                            if amp > pos:
                                self._handle_text(buf, pos, amp)
                                pos = amp
                            break
                        # Check for trailing ']' or ']]' that could become ']]>'
                        if buf.endswith("]]"):
                            end_text = n - 2
                        elif buf.endswith("]"):
                            end_text = n - 1
                        else:
                            end_text = n
                        if end_text > pos:
                            self._handle_text(buf, pos, end_text)
                            pos = end_text
                        break
                    self._handle_text(buf, pos, n)
                    pos = n
                    break
                else:
                    self._handle_text(buf, pos, lt)
                    pos = lt
                    continue

            # buf[pos] == '<'
            if pos + 1 >= n:
                if final:
                    self._error("unclosed token", 5, buf, pos)
                break

            nxt = buf[pos + 1]
            if nxt == "/":
                new_pos = self._parse_end_tag(buf, pos, final)
                if new_pos is None:
                    break
                self._at_doc_start = False
                pos = new_pos
                continue

            if nxt == "?":
                end_pi = buf.find("?>", pos + 2)
                if end_pi == -1:
                    if final:
                        self._error("unclosed token", 5, buf, pos)
                    break
                body = buf[pos + 2:end_pi]
                # Extract target name
                k = 0
                blen = len(body)
                while k < blen and body[k] not in _XML_WHITESPACE:
                    k += 1
                pi_target = body[:k]
                if not pi_target or not _is_valid_name(pi_target):
                    self._error("not well-formed (invalid token)", 4, buf, pos + 2)
                if pi_target.lower() == "xml":
                    if pi_target != "xml" or not self._at_doc_start:
                        self._error(
                            "XML or text declaration not at start of entity",
                            14,
                            buf,
                            pos,
                        )
                    self._parse_xml_decl(body[k:], buf, pos)
                    self._at_doc_start = False
                    pos = end_pi + 2
                    continue
                if ":" in pi_target:
                    self._error("not well-formed (invalid token)", 4, buf, pos + 2)
                # Strip leading whitespace between target and data
                d = k
                while d < blen and body[d] in _XML_WHITESPACE:
                    d += 1
                pi_data = body[d:] if d < blen else ""
                self._at_doc_start = False
                self._emit_pi(pi_target, pi_data)
                pos = end_pi + 2
                continue

            if nxt == "!":
                if pos + 3 < n and buf[pos:pos + 4] == "<!--":
                    end_c = buf.find("-->", pos + 4)
                    if end_c == -1:
                        if final:
                            self._error("unclosed token", 5, buf, pos)
                        break
                    comment_text = buf[pos + 4:end_c]
                    if "--" in comment_text or comment_text.endswith("-"):
                        self._error("not well-formed (invalid token)", 4, buf, pos)
                    self._at_doc_start = False
                    self._emit_comment(comment_text)
                    pos = end_c + 3
                    continue

                if pos + 8 < n and buf[pos:pos + 9] == "<![CDATA[":
                    if not self._stack:
                        self._error("syntax error", 2, buf, pos)
                    end_cd = buf.find("]]>", pos + 9)
                    if end_cd == -1:
                        if final:
                            self._error("unclosed CDATA section", 20, buf, pos)
                        break
                    cdata = buf[pos + 9:end_cd]
                    self._at_doc_start = False
                    self._emit_data(cdata)
                    pos = end_cd + 3
                    continue

                if pos + 8 < n and buf[pos:pos + 9] == "<!DOCTYPE":
                    if self._root_seen or self._doctype_seen:
                        self._error("syntax error", 2, buf, pos)
                    # Scan DOCTYPE respecting quotes and internal [...] subset
                    k = pos + 9
                    in_q = None
                    bracket_depth = 0
                    subset_start = -1
                    subset_end = -1
                    while k < n:
                        c = buf[k]
                        if in_q:
                            if c == in_q:
                                in_q = None
                        else:
                            if c == "'" or c == '"':
                                in_q = c
                            elif c == "[":
                                if bracket_depth == 0:
                                    subset_start = k + 1
                                bracket_depth += 1
                            elif c == "]":
                                bracket_depth -= 1
                                if bracket_depth == 0:
                                    subset_end = k
                            elif c == ">" and bracket_depth == 0:
                                break
                        k += 1
                    else:
                        if final:
                            self._error("unclosed token", 5, buf, pos)
                        break
                    if subset_start != -1 and subset_end != -1:
                        self._parse_doctype_internal_subset(
                            buf[subset_start:subset_end]
                        )
                    self._doctype_seen = True
                    self._at_doc_start = False
                    pos = k + 1
                    continue

                # Check if prefix of '<!--', '<![CDATA[', or '<!DOCTYPE' is incomplete
                if not final:
                    rem = buf[pos:]
                    if (
                        "<!--".startswith(rem)
                        or "<![CDATA[".startswith(rem)
                        or "<!DOCTYPE".startswith(rem)
                    ):
                        break
                self._error("not well-formed (invalid token)", 4, buf, pos)

            # Start tag
            new_pos = self._parse_start_tag(buf, pos, final)
            if new_pos is None:
                break
            self._at_doc_start = False
            pos = new_pos

        if pos > 0:
            self._advance_pos(buf[:pos])
            self._str_buf = buf[pos:]

    def _handle_text(self, buf, start, end):
        raw = buf[start:end]
        if not self._stack:
            # Outside root element: only whitespace is legal
            for idx, ch in enumerate(raw):
                if ch not in _XML_WHITESPACE:
                    if self._root_closed:
                        self._error("junk after document element", 9, buf, start + idx)
                    else:
                        self._error("syntax error", 2, buf, start + idx)
            if raw:
                self._at_doc_start = False
            return

        if "]]>" in raw:
            bad = raw.find("]]>")
            self._error("not well-formed (invalid token)", 4, buf, start + bad)
        text = self._unescape(raw, buf, start, False)
        self._emit_data(text)


# ---------------------------------------------------------------------------
# XMLPullParser & iterparse
# ---------------------------------------------------------------------------

class XMLPullParser:
    """Non-blocking incremental XML event parser."""

    def __init__(self, events=None, *, _parser=None):
        self._events_queue = collections.deque()
        self._parser = _parser if _parser is not None else XMLParser(target=TreeBuilder())
        if events is None:
            events = ("end",)
        self._parser._setevents(self._events_queue, events)

    def feed(self, data):
        """Feed given data to the parser."""
        if data:
            self._parser.feed(data)

    def close(self):
        """Finish feeding data to parser."""
        self._parser.close()

    def read_events(self):
        """Return an iterator over currently available (event, elem) pairs."""
        events = self._events_queue
        while events:
            yield events.popleft()


def parse(source, parser=None):
    """Parse XML document into element tree."""
    tree = ElementTree()
    tree.parse(source, parser)
    return tree


def iterparse(source, events=None, parser=None):
    """Incrementally parse XML document into element tree, yielding events."""
    if events is None:
        events = ("end",)
    pullparser = XMLPullParser(events=events, _parser=parser)

    def iterator():
        close_source = False
        src = source
        if not hasattr(src, "read"):
            src = open(src, "rb")
            close_source = True
        try:
            while True:
                data = src.read(16 * 1024)
                if not data:
                    break
                pullparser.feed(data)
                yield from pullparser.read_events()
            pullparser.close()
            yield from pullparser.read_events()
        finally:
            if close_source:
                src.close()

    return iterator()


def XML(text, parser=None):
    """Parse XML document from string constant."""
    if not parser:
        parser = XMLParser(target=TreeBuilder())
    parser.feed(text)
    return parser.close()


fromstring = XML


def XMLID(text, parser=None):
    """Parse XML document from string constant for its IDs."""
    if not parser:
        parser = XMLParser(target=TreeBuilder())
    parser.feed(text)
    tree = parser.close()
    ids = {}
    for elem in tree.iter():
        id_val = elem.get("id")
        if id_val:
            ids[id_val] = elem
    return tree, ids


def fromstringlist(sequence, parser=None):
    """Parse XML document from sequence of string fragments."""
    if not parser:
        parser = XMLParser(target=TreeBuilder())
    for text in sequence:
        parser.feed(text)
    return parser.close()


# ---------------------------------------------------------------------------
# Canonical XML 2.0 (C14NWriterTarget & canonicalize)
# ---------------------------------------------------------------------------

def _escape_cdata_c14n(text):
    if "&" in text:
        text = text.replace("&", "&amp;")
    if "<" in text:
        text = text.replace("<", "&lt;")
    if ">" in text:
        text = text.replace(">", "&gt;")
    if "\r" in text:
        text = text.replace("\r", "&#xD;")
    return text


def _escape_attrib_c14n(text):
    if "&" in text:
        text = text.replace("&", "&amp;")
    if "<" in text:
        text = text.replace("<", "&lt;")
    if '"' in text:
        text = text.replace('"', "&quot;")
    if "\t" in text:
        text = text.replace("\t", "&#x9;")
    if "\n" in text:
        text = text.replace("\n", "&#xA;")
    if "\r" in text:
        text = text.replace("\r", "&#xD;")
    return text


class C14NWriterTarget:
    """Canonicalization writer target for the XMLParser (C14N 2.0)."""

    def __init__(self, write, *,
                 with_comments=False, strip_text=False, rewrite_prefixes=False,
                 qname_aware_tags=None, qname_aware_attrs=None,
                 exclude_attrs=None, exclude_tags=None):
        self._write = write
        self._data = []
        self._with_comments = with_comments
        self._strip_text = strip_text
        self._exclude_attrs = set(exclude_attrs) if exclude_attrs else None
        self._exclude_tags = set(exclude_tags) if exclude_tags else None
        self._rewrite_prefixes = rewrite_prefixes
        if qname_aware_tags:
            self._qname_aware_tags = set(qname_aware_tags)
        else:
            self._qname_aware_tags = None
        if qname_aware_attrs:
            self._find_qname_aware_attrs = set(qname_aware_attrs).intersection
        else:
            self._find_qname_aware_attrs = None

        self._ignored_depth = 0
        self._tree_path = []
        self._root_seen = False
        self._root_done = False
        self._ignored_root = False

        # Namespace bookkeeping
        self._declared_ns_stack = [[
            ("http://www.w3.org/XML/1998/namespace", "xml"),
        ]]
        self._ns_stack = []
        if not rewrite_prefixes:
            self._ns_stack.append({"xml": "http://www.w3.org/XML/1998/namespace"})
        self._ns_stack.append({})
        self._prefix_map = {}
        self._preserve_space = [False]
        self._pending_start = None

        self._qname_aware_tag = False

    def _iter_namespaces(self, ns_stack, _reversed=reversed):
        for namespaces in _reversed(ns_stack):
            if namespaces:
                yield from namespaces.items()

    def _resolve_prefix_name(self, prefixed_name):
        prefix, name = prefixed_name.split(":", 1)
        for namespaces in reversed(self._ns_stack):
            if prefix in namespaces:
                return f"{{{namespaces[prefix]}}}{name}"
        raise ValueError(f"Prefix {prefix} of QName {name!r} is not declared in scope")

    def _qname(self, qname, uri=None):
        if uri is None:
            uri, tag = qname[1:].rsplit("}", 1) if qname[:1] == "{" else ("", qname)
        else:
            tag = qname

        prefixes_seen = set()
        for u, prefix in self._iter_namespaces_list():
            if u == uri and prefix not in prefixes_seen:
                return f"{prefix}:{tag}" if prefix else tag, tag, uri
            prefixes_seen.add(prefix)

        if self._rewrite_prefixes:
            if uri in self._prefix_map:
                prefix = self._prefix_map[uri]
            else:
                prefix = self._prefix_map[uri] = f"n{len(self._prefix_map)}"
            self._declared_ns_stack[-1].append((uri, prefix))
            return f"{prefix}:{tag}", tag, uri

        if not uri and "" not in prefixes_seen:
            return tag, tag, uri

        for u, prefix in self._iter_namespaces_list():
            if u == uri:
                return f"{prefix}:{tag}" if prefix else tag, tag, uri

        # Search input namespace stack for a prefix for this uri
        for namespaces in reversed(self._ns_stack):
            for prefix, u in namespaces.items():
                if u == uri:
                    if not prefix:
                        return tag, tag, uri
                    return f"{prefix}:{tag}", tag, uri

        if not uri:
            return tag, tag, uri
        raise ValueError(f"Namespace {uri!r} is not declared in scope")

    def _iter_namespaces_list(self):
        for scope in reversed(self._declared_ns_stack):
            for item in reversed(scope):
                yield item

    def _flush_data(self):
        if self._data:
            data = "".join(self._data)
            self._data = []
            if self._strip_text and not self._preserve_space[-1]:
                data = data.strip()
            if data:
                if self._qname_aware_tag and ":" in data:
                    try:
                        qname = self._resolve_prefix_name(data)
                        prefixed, _, _ = self._qname(qname)
                        data = prefixed
                    except ValueError:
                        pass
                self._write(_escape_cdata_c14n(data))

    def data(self, text):
        if not self._ignored_depth:
            self._data.append(text)

    def _start_ns(self, prefix, uri):
        if self._ignored_depth:
            return
        self._ns_stack[-1][prefix] = uri

    start_ns = _start_ns

    def start(self, tag, attrs):
        if self._exclude_tags is not None and (
            self._ignored_depth or tag in self._exclude_tags
        ):
            self._ignored_depth += 1
            return

        if self._data:
            self._flush_data()

        new_namespaces = []
        self._declared_ns_stack.append(new_namespaces)

        if self._qname_aware_tags is not None and tag in self._qname_aware_tags:
            self._qname_aware_tag = True
        else:
            self._qname_aware_tag = False

        # Determine namespace declarations needed on this element
        parse_attrs = {}
        if attrs:
            for k, v in attrs.items():
                if self._exclude_attrs is not None and k in self._exclude_attrs:
                    continue
                parse_attrs[k] = v

        # Handle xml:space
        xml_space = parse_attrs.get(f"{{{_XML_NAMESPACE}}}space")
        if xml_space == "preserve":
            self._preserve_space.append(True)
        elif xml_space == "default":
            self._preserve_space.append(False)
        else:
            self._preserve_space.append(self._preserve_space[-1])

        # Track URIs used on this element (tag + attrs)
        uris_used = []
        tag_uri, tag_local = (
            tag[1:].rsplit("}", 1) if tag[:1] == "{" else ("", tag)
        )
        uris_used.append((tag_uri, False))

        attr_items = []
        qname_attrs = (
            self._find_qname_aware_attrs(parse_attrs)
            if self._find_qname_aware_attrs is not None
            else None
        )
        for k, v in parse_attrs.items():
            a_uri, a_local = k[1:].rsplit("}", 1) if k[:1] == "{" else ("", k)
            if a_uri:
                uris_used.append((a_uri, True))
            if qname_attrs and k in qname_attrs and ":" in v:
                try:
                    v_qname = self._resolve_prefix_name(v)
                    v_uri = v_qname[1:].rsplit("}", 1)[0]
                    uris_used.append((v_uri, True))
                    v = ("__qname__", v_qname)
                except ValueError:
                    pass
            attr_items.append((a_uri, a_local, k, v))

        # Bind prefixes in declared_ns_stack
        active_by_prefix = {}
        active_by_uri = {}
        for scope in self._declared_ns_stack[:-1]:
            for u, p in scope:
                active_by_prefix[p] = u
                active_by_uri[u] = p

        # If rewrite_prefixes, register "" -> n0 first if not yet mapped
        if self._rewrite_prefixes and not self._root_seen:
            if "" not in self._prefix_map:
                self._prefix_map[""] = "n0"
                new_namespaces.append(("", "n0"))
                active_by_prefix["n0"] = ""
                active_by_uri[""] = "n0"

        def ensure_ns(uri, is_attr):
            if uri == _XML_NAMESPACE:
                return "xml"
            if not uri and not is_attr:
                if self._rewrite_prefixes:
                    return self._prefix_map.get("", "n0")
                if active_by_prefix.get("", "") != "":
                    new_namespaces.append(("", ""))
                    active_by_prefix[""] = ""
                    active_by_uri[""] = ""
                return ""
            if not uri and is_attr:
                return ""

            if self._rewrite_prefixes:
                if uri in self._prefix_map:
                    p = self._prefix_map[uri]
                    if active_by_prefix.get(p) != uri:
                        new_namespaces.append((uri, p))
                        active_by_prefix[p] = uri
                        active_by_uri[uri] = p
                    return p
                p = self._prefix_map[uri] = f"n{len(self._prefix_map)}"
                new_namespaces.append((uri, p))
                active_by_prefix[p] = uri
                active_by_uri[uri] = p
                return p

            # Non-rewriting mode: find active prefix in input _ns_stack
            chosen_prefix = None
            for scope in reversed(self._ns_stack):
                for p, u in scope.items():
                    if u == uri:
                        if is_attr and not p:
                            continue
                        chosen_prefix = p
                        break
                if chosen_prefix is not None:
                    break
            if chosen_prefix is None:
                chosen_prefix = "" if not is_attr else "ns0"
            if active_by_prefix.get(chosen_prefix) != uri:
                new_namespaces.append((uri, chosen_prefix))
                active_by_prefix[chosen_prefix] = uri
                active_by_uri[uri] = chosen_prefix
            return chosen_prefix

        tag_prefix = ensure_ns(tag_uri, False)
        if self._rewrite_prefixes and not tag_uri:
            prefixed_tag = tag_local
        else:
            prefixed_tag = f"{tag_prefix}:{tag_local}" if tag_prefix else tag_local

        resolved_attrs = []
        for a_uri, a_local, _orig_k, v in attr_items:
            if a_uri:
                a_prefix = ensure_ns(a_uri, True)
                a_name = f"{a_prefix}:{a_local}" if a_prefix else a_local
            else:
                a_name = a_local
            if isinstance(v, tuple) and len(v) == 2 and v[0] == "__qname__":
                v_uri, v_local = v[1][1:].rsplit("}", 1)
                v_prefix = ensure_ns(v_uri, True)
                v = f"{v_prefix}:{v_local}" if v_prefix else v_local
            resolved_attrs.append((a_uri, a_local, a_name, v))

        # Sort namespace declarations by prefix (default "" first)
        new_namespaces.sort(key=lambda item: item[1])
        # Sort attributes by (uri, local_name)
        resolved_attrs.sort(key=lambda item: (item[0], item[1]))

        out = self._write
        out(f"<{prefixed_tag}")
        for uri, prefix in new_namespaces:
            if prefix:
                out(f' xmlns:{prefix}="{_escape_attrib_c14n(uri)}"')
            else:
                out(f' xmlns="{_escape_attrib_c14n(uri)}"')
        for _a_uri, _a_local, a_name, v in resolved_attrs:
            out(f' {a_name}="{_escape_attrib_c14n(v)}"')
        out(">")

        self._root_seen = True
        self._tree_path.append(prefixed_tag)
        self._ns_stack.append({})

    def end(self, tag):
        if self._ignored_depth:
            self._ignored_depth -= 1
            return
        if self._data:
            self._flush_data()
        prefixed_tag = self._tree_path.pop()
        self._write(f"</{prefixed_tag}>")
        self._declared_ns_stack.pop()
        self._ns_stack.pop()
        self._preserve_space.pop()
        self._qname_aware_tag = False
        if not self._tree_path:
            self._root_done = True

    def comment(self, text):
        if not self._with_comments or self._ignored_depth:
            return
        if self._data:
            self._flush_data()
        if self._root_done:
            self._write("\n")
        self._write(f"<!--{_escape_cdata_c14n(text)}-->")
        if not self._root_seen:
            self._write("\n")

    def pi(self, target, data):
        if self._ignored_depth:
            return
        if self._data:
            self._flush_data()
        if self._root_done:
            self._write("\n")
        if data:
            self._write(f"<?{target} {_escape_cdata_c14n(data)}?>")
        else:
            self._write(f"<?{target}?>")
        if not self._root_seen:
            self._write("\n")

    def close(self):
        return None


def canonicalize(xml_data=None, *, out=None, from_file=None, **options):
    """Convert XML to its C14N 2.0 serialised form."""
    if xml_data is None and from_file is None:
        raise ValueError("Either 'xml_data' or 'from_file' must be provided as input")

    sio = None
    if out is None:
        sio = out = io.StringIO()

    parser = XMLParser(target=C14NWriterTarget(out.write, **options))

    if xml_data is not None:
        parser.feed(xml_data)
        parser.close()
    else:
        close_f = False
        f = from_file
        if not hasattr(f, "read"):
            f = open(f, "rb")
            close_f = True
        try:
            while True:
                chunk = f.read(16 * 1024)
                if not chunk:
                    break
                parser.feed(chunk)
            parser.close()
        finally:
            if close_f:
                f.close()

    return sio.getvalue() if sio is not None else None
