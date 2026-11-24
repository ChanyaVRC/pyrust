"""A parser for HTML and XHTML."""

import re

from html import unescape


__all__ = ['HTMLParser']


# ---------------------------------------------------------------------------
# _markupbase.ParserBase (inlined for html.parser foundation)
# ---------------------------------------------------------------------------

_declname_match = re.compile(r'[a-zA-Z][-_.a-zA-Z0-9]*\s*').match
_declstringlit_match = re.compile(r'(\'[^\']*\'|"[^"]*")\s*').match
_commentclose = re.compile(r'--\s*>')
_markedsectionclose = re.compile(r']\s*]\s*>')
_msmarkedsectionclose = re.compile(r']\s*>')


class ParserBase:
    """Parser base class which provides some common support methods used
    by the SGML/HTML and XHTML parsers."""

    def __init__(self):
        if self.__class__ is ParserBase:
            raise RuntimeError(
                "_markupbase.ParserBase must be subclassed")

    def reset(self):
        self.lineno = 1
        self.offset = 0

    def getpos(self):
        """Return current line number and offset."""
        return self.lineno, self.offset

    def updatepos(self, i, j):
        if i >= j:
            return j
        rawdata = self.rawdata
        nlines = rawdata.count("\n", i, j)
        if nlines:
            self.lineno = self.lineno + nlines
            pos = rawdata.rindex("\n", i, j)  # Should not fail
            self.offset = j - (pos + 1)
        else:
            self.offset = self.offset + j - i
        return j

    _decl_otherchars = ''

    def parse_declaration(self, i):
        rawdata = self.rawdata
        j = i + 2
        assert rawdata[i:j] == "<!", "unexpected call to parse_declaration"
        if rawdata[j:j + 1] == ">":
            # the empty comment <!>
            return j + 1
        if rawdata[j:j + 1] in ("-", ""):
            # Start of comment followed by buffer boundary,
            # or just a buffer boundary.
            return -1
        n = len(rawdata)
        if rawdata[j:j + 2] == '--':  # comment
            return self.parse_comment(i)
        elif rawdata[j] == '[':  # marked section
            return self.parse_marked_section(i)
        else:  # all other declaration elements
            decltype, j = self._scan_name(j, i)
        if j < 0:
            return j
        if decltype == "doctype":
            self._decl_otherchars = ''
        while j < n:
            c = rawdata[j]
            if c == ">":
                data = rawdata[i + 2:j]
                if decltype == "doctype":
                    self.handle_decl(data)
                else:
                    self.unknown_decl(data)
                return j + 1
            if c in "\"'":
                m = _declstringlit_match(rawdata, j)
                if not m:
                    return -1  # incomplete
                j = m.end()
            elif c in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ":
                name, j = self._scan_name(j, i)
            elif c in self._decl_otherchars:
                j = j + 1
            elif c == "[":
                if decltype == "doctype":
                    j = self._parse_doctype_subset(j + 1, i)
                elif decltype in {"attlist", "linktype", "link", "element"}:
                    raise AssertionError("unsupported '[' char in %s declaration" % decltype)
                else:
                    raise AssertionError("unexpected '[' char in declaration")
            else:
                raise AssertionError("unexpected %r char in declaration" % rawdata[j])
            if j < 0:
                return j
        return -1  # incomplete

    def parse_marked_section(self, i, report=1):
        rawdata = self.rawdata
        assert rawdata[i:i + 3] == '<![', "unexpected call to parse_marked_section()"
        sectName, j = self._scan_name(i + 3, i)
        if j < 0:
            return j
        if sectName in {"temp", "cdata", "ignore", "include", "rcdata"}:
            match = _markedsectionclose.search(rawdata, i + 3)
        elif sectName in {"if", "else", "endif"}:
            match = _msmarkedsectionclose.search(rawdata, i + 3)
        else:
            raise AssertionError(
                'unknown status keyword %r in marked section' % rawdata[i + 3:j]
            )
        if not match:
            return -1
        if report:
            j = match.start(0)
            self.unknown_decl(rawdata[i + 3:j])
        return match.end(0)

    def parse_comment(self, i, report=1):
        rawdata = self.rawdata
        if rawdata[i:i + 4] != '<!--':
            raise AssertionError('unexpected call to parse_comment()')
        match = _commentclose.search(rawdata, i + 4)
        if not match:
            return -1
        if report:
            j = match.start(0)
            self.handle_comment(rawdata[i + 4:j])
        return match.end(0)

    def _parse_doctype_subset(self, i, declstartpos):
        rawdata = self.rawdata
        n = len(rawdata)
        j = i
        while j < n:
            c = rawdata[j]
            if c == "<":
                s = rawdata[j:j + 2]
                if s == "<":
                    return -1
                if s != "<!":
                    self.updatepos(declstartpos, j + 1)
                    raise AssertionError(
                        "unexpected char in internal subset (in %r)" % s
                    )
                if (j + 2) == n:
                    return -1
                if (j + 4) > n:
                    return -1
                if rawdata[j:j + 4] == "<!--":
                    j = self.parse_comment(j, report=0)
                    if j < 0:
                        return j
                    continue
                name, j = self._scan_name(j + 2, declstartpos)
                if j == -1:
                    return -1
                if name not in {"attlist", "element", "entity", "notation"}:
                    self.updatepos(declstartpos, j + 2)
                    raise AssertionError(
                        "unknown declaration %r in internal subset" % name
                    )
                meth = getattr(self, "_parse_doctype_" + name)
                j = meth(j, declstartpos)
                if j < 0:
                    return j
            elif c == "%":
                if (j + 1) == n:
                    return -1
                s, j = self._scan_name(j + 1, declstartpos)
                if j < 0:
                    return j
                if rawdata[j] == ";":
                    j = j + 1
            elif c == "]":
                j = j + 1
                while j < n and rawdata[j].isspace():
                    j = j + 1
                if j < n:
                    if rawdata[j] == ">":
                        return j
                    self.updatepos(declstartpos, j)
                    raise AssertionError("unexpected char after internal subset")
                else:
                    return -1
            elif c.isspace():
                j = j + 1
            else:
                self.updatepos(declstartpos, j)
                raise AssertionError("unexpected char %r in internal subset" % c)
        return -1

    def _parse_doctype_element(self, i, declstartpos):
        name, j = self._scan_name(i, declstartpos)
        if j == -1:
            return -1
        rawdata = self.rawdata
        if '>' in rawdata[j:]:
            return rawdata.find(">", j) + 1
        return -1

    def _parse_doctype_attlist(self, i, declstartpos):
        rawdata = self.rawdata
        name, j = self._scan_name(i, declstartpos)
        c = rawdata[j:j + 1]
        if c == "":
            return -1
        if c == ">":
            return j + 1
        while 1:
            name, j = self._scan_name(j, declstartpos)
            if j < 0:
                return j
            c = rawdata[j:j + 1]
            if c == "":
                return -1
            if c == "(":
                if ")" in rawdata[j:]:
                    j = rawdata.find(")", j) + 1
                else:
                    return -1
                while rawdata[j:j + 1].isspace():
                    j = j + 1
                if not rawdata[j:]:
                    return -1
            else:
                name, j = self._scan_name(j, declstartpos)
            c = rawdata[j:j + 1]
            if not c:
                return -1
            if c in "'\"":
                m = _declstringlit_match(rawdata, j)
                if m:
                    j = m.end()
                else:
                    return -1
                c = rawdata[j:j + 1]
                if not c:
                    return -1
            if c == "#":
                if rawdata[j:] == "#":
                    return -1
                name, j = self._scan_name(j + 1, declstartpos)
                if j < 0:
                    return j
                c = rawdata[j:j + 1]
                if not c:
                    return -1
            if c == '>':
                return j + 1

    def _parse_doctype_notation(self, i, declstartpos):
        name, j = self._scan_name(i, declstartpos)
        if j < 0:
            return j
        rawdata = self.rawdata
        while 1:
            c = rawdata[j:j + 1]
            if not c:
                return -1
            if c == '>':
                return j + 1
            if c in "'\"":
                m = _declstringlit_match(rawdata, j)
                if not m:
                    return -1
                j = m.end()
            else:
                name, j = self._scan_name(j, declstartpos)
                if j < 0:
                    return j

    def _parse_doctype_entity(self, i, declstartpos):
        rawdata = self.rawdata
        if rawdata[i:i + 1] == "%":
            j = i + 1
            while 1:
                c = rawdata[j:j + 1]
                if not c:
                    return -1
                if c.isspace():
                    j = j + 1
                else:
                    break
        else:
            j = i
        name, j = self._scan_name(j, declstartpos)
        if j < 0:
            return j
        while 1:
            c = self.rawdata[j:j + 1]
            if not c:
                return -1
            if c in "'\"":
                m = _declstringlit_match(rawdata, j)
                if m:
                    j = m.end()
                else:
                    return -1
            elif c == ">":
                return j + 1
            else:
                name, j = self._scan_name(j, declstartpos)
                if j < 0:
                    return j

    def _scan_name(self, i, declstartpos):
        rawdata = self.rawdata
        n = len(rawdata)
        if i == n:
            return None, -1
        m = _declname_match(rawdata, i)
        if m:
            s = m.group()
            name = s.strip()
            if (i + len(s)) == n:
                return None, -1
            return name.lower(), m.end()
        else:
            self.updatepos(declstartpos, i)
            raise AssertionError(
                "expected name token at %r" % rawdata[declstartpos:declstartpos + 20]
            )

    def unknown_decl(self, data):
        pass


# ---------------------------------------------------------------------------
# Regular expressions used for HTML parsing
# ---------------------------------------------------------------------------

interesting_normal = re.compile('[&<]')
incomplete = re.compile('&[a-zA-Z#]')

entityref = re.compile('&([a-zA-Z][-.a-zA-Z0-9]*)[^a-zA-Z0-9]')
charref = re.compile('&#(?:[0-9]+|[xX][0-9a-fA-F]+)[^0-9a-fA-F]')

starttagopen = re.compile('<[a-zA-Z]')
endtagopen = re.compile('</[a-zA-Z]')
piclose = re.compile('>')
commentclose = re.compile(r'--!?>')
commentabruptclose = re.compile(r'-?>')

tagfind_tolerant = re.compile(r'([a-zA-Z][^\t\n\r\f />]*)(?:[\t\n\r\f ]|/(?!>))*')
attrfind_tolerant = re.compile(r"""
  (
    (?<=['"\t\n\r\f /])[^\t\n\r\f />][^\t\n\r\f /=>]*  # attribute name
   )
  ([\t\n\r\f ]*=[\t\n\r\f ]*        # value indicator
    ('[^']*'                        # LITA-enclosed value
    |"[^"]*"                        # LIT-enclosed value
    |(?!['"])[^>\t\n\r\f ]*         # bare value
    )
   )?
  (?:[\t\n\r\f ]|/(?!>))*           # possibly followed by a space
""", re.VERBOSE)
locatetagend = re.compile(r"""
  [a-zA-Z][^\t\n\r\f />]*           # tag name
  [\t\n\r\f /]*                     # optional whitespace before attribute name
  (?:(?<=['"\t\n\r\f /])[^\t\n\r\f />][^\t\n\r\f /=>]*  # attribute name
    (?:[\t\n\r\f ]*=[\t\n\r\f ]*    # value indicator
      (?:'[^']*'                    # LITA-enclosed value
        |"[^"]*"                    # LIT-enclosed value
        |(?!['"])[^>\t\n\r\f ]*     # bare value
       )
     )?
    [\t\n\r\f /]*                   # possibly followed by a space
   )*
   >?
""", re.VERBOSE)
locatestarttagend_tolerant = re.compile(r"""
  <[a-zA-Z][^\t\n\r\f />\x00]*       # tag name
  (?:[\s/]*                          # optional whitespace before attribute name
    (?:(?<=['"\s/])[^\s/>][^\s/=>]*  # attribute name
      (?:\s*=+\s*                    # value indicator
        (?:'[^']*'                   # LITA-enclosed value
          |"[^"]*"                   # LIT-enclosed value
          |(?!['"])[^>\s]*           # bare value
         )
        \s*                          # possibly followed by a space
       )?(?:\s|/(?!>))*
     )*
   )?
  \s*                                # trailing whitespace
""", re.VERBOSE)
endendtag = re.compile('>')
endtagfind = re.compile(r'</\s*([a-zA-Z][-.a-zA-Z0-9:_]*)\s*>')


class HTMLParser(ParserBase):
    """Find tags and other markup and call handler functions."""

    CDATA_CONTENT_ELEMENTS = ("script", "style", "xmp", "iframe", "noembed", "noframes")
    RCDATA_CONTENT_ELEMENTS = ("textarea", "title")

    def __init__(self, *, convert_charrefs=True, scripting=False):
        super().__init__()
        self.convert_charrefs = convert_charrefs
        self.scripting = scripting
        self.reset()

    def reset(self):
        """Reset this instance.  Loses all unprocessed data."""
        self.rawdata = ''
        self.lasttag = '???'
        self.interesting = interesting_normal
        self.cdata_elem = None
        self._support_cdata = True
        self._escapable = True
        super().reset()

    def feed(self, data):
        r"""Feed data to the parser.

        Call this as often as you want, with as little or as much text
        as you want (may include '\n').
        """
        self.rawdata = self.rawdata + data
        self.goahead(0)

    def close(self):
        """Handle any buffered data."""
        self.goahead(1)

    __starttag_text = None

    def get_starttag_text(self):
        """Return full source of start tag: '<...>'."""
        return self.__starttag_text

    def set_cdata_mode(self, elem, *, escapable=False):
        self.cdata_elem = elem.lower()
        self._escapable = escapable
        if self.cdata_elem == 'plaintext':
            self.interesting = re.compile(r'\Z')
        elif escapable and not self.convert_charrefs:
            self.interesting = re.compile(
                r'&|</%s(?=[\t\n\r\f />])' % self.cdata_elem,
                re.IGNORECASE | re.ASCII,
            )
        else:
            self.interesting = re.compile(
                r'</%s(?=[\t\n\r\f />])' % self.cdata_elem,
                re.IGNORECASE | re.ASCII,
            )

    def clear_cdata_mode(self):
        self.interesting = interesting_normal
        self.cdata_elem = None
        self._escapable = True

    def _set_support_cdata(self, flag=True):
        self._support_cdata = flag

    def goahead(self, end):
        rawdata = self.rawdata
        i = 0
        n = len(rawdata)
        while i < n:
            if self.convert_charrefs and not self.cdata_elem:
                j = rawdata.find('<', i)
                if j < 0:
                    amppos = rawdata.rfind('&', max(i, n - 34))
                    if (amppos >= 0 and
                            not re.compile(r'[\t\n\r\f ;]').search(rawdata, amppos)):
                        break  # wait till we get all the text
                    j = n
            else:
                match = self.interesting.search(rawdata, i)  # < or &
                if match:
                    j = match.start()
                else:
                    if self.cdata_elem:
                        break
                    j = n
            if i < j:
                if self.convert_charrefs and self._escapable:
                    self.handle_data(unescape(rawdata[i:j]))
                else:
                    self.handle_data(rawdata[i:j])
            i = self.updatepos(i, j)
            if i == n:
                break
            startswith = rawdata.startswith
            if startswith('<', i):
                if starttagopen.match(rawdata, i):  # < + letter
                    k = self.parse_starttag(i)
                elif startswith("</", i):
                    k = self.parse_endtag(i)
                elif startswith("<!--", i):
                    k = self.parse_comment(i)
                elif startswith("<?", i):
                    k = self.parse_pi(i)
                elif startswith("<!", i):
                    k = self.parse_html_declaration(i)
                elif (i + 1) < n or end:
                    self.handle_data("<")
                    k = i + 1
                else:
                    break
                if k < 0:
                    if not end:
                        break
                    if starttagopen.match(rawdata, i):  # < + letter
                        pass
                    elif startswith("</", i):
                        if i + 2 == n:
                            self.handle_data("</")
                        elif endtagopen.match(rawdata, i):  # </ + letter
                            pass
                        else:
                            # bogus comment
                            self.handle_comment(rawdata[i + 2:])
                    elif startswith("<!--", i):
                        j = n
                        for suffix in ("--!", "--", "-"):
                            if rawdata.endswith(suffix, i + 4):
                                j -= len(suffix)
                                break
                        self.handle_comment(rawdata[i + 4:j])
                    elif startswith("<![CDATA[", i) and self._support_cdata:
                        self.unknown_decl(rawdata[i + 3:])
                    elif rawdata[i:i + 9].lower() == '<!doctype':
                        self.handle_decl(rawdata[i + 2:])
                    elif startswith("<!", i):
                        # bogus comment
                        self.handle_comment(rawdata[i + 2:])
                    elif startswith("<?", i):
                        self.handle_pi(rawdata[i + 2:])
                    else:
                        raise AssertionError("we should not get here!")
                    k = n
                i = self.updatepos(i, k)
            elif startswith("&#", i):
                match = charref.match(rawdata, i)
                if match:
                    name = match.group()[2:-1]
                    self.handle_charref(name)
                    k = match.end()
                    if not startswith(';', k - 1):
                        k = k - 1
                    i = self.updatepos(i, k)
                    continue
                else:
                    if ";" in rawdata[i:]:  # bail by consuming &#
                        self.handle_data(rawdata[i:i + 2])
                        i = self.updatepos(i, i + 2)
                    break
            elif startswith('&', i):
                match = entityref.match(rawdata, i)
                if match:
                    name = match.group(1)
                    self.handle_entityref(name)
                    k = match.end()
                    if not startswith(';', k - 1):
                        k = k - 1
                    i = self.updatepos(i, k)
                    continue
                match = incomplete.match(rawdata, i)
                if match:
                    if end and match.group() == rawdata[i:]:
                        k = match.end()
                        if k <= i:
                            k = n
                        i = self.updatepos(i, i + 1)
                    # incomplete
                    break
                elif (i + 1) < n:
                    self.handle_data("&")
                    i = self.updatepos(i, i + 1)
                else:
                    break
            else:
                assert 0, "interesting.search() lied"
        # end while
        if end and i < n:
            if self.convert_charrefs and self._escapable:
                self.handle_data(unescape(rawdata[i:n]))
            else:
                self.handle_data(rawdata[i:n])
            i = self.updatepos(i, n)
        self.rawdata = rawdata[i:]

    def parse_html_declaration(self, i):
        rawdata = self.rawdata
        assert rawdata[i:i + 2] == '<!', (
            'unexpected call to parse_html_declaration()'
        )
        if rawdata[i:i + 4] == '<!--':
            return self.parse_comment(i)
        elif rawdata[i:i + 9] == '<![CDATA[' and self._support_cdata:
            j = rawdata.find(']]>', i + 9)
            if j < 0:
                return -1
            self.unknown_decl(rawdata[i + 3:j])
            return j + 3
        elif rawdata[i:i + 9].lower() == '<!doctype':
            gtpos = rawdata.find('>', i + 9)
            if gtpos == -1:
                return -1
            self.handle_decl(rawdata[i + 2:gtpos])
            return gtpos + 1
        elif rawdata[i:i + 3] == '<![':
            j = rawdata.find('>', i + 3)
            if j < 0:
                return -1
            if rawdata[j - 1] == ']':
                self.unknown_decl(rawdata[i + 3:j - 1])
            else:
                self.handle_comment(rawdata[i + 2:j])
            return j + 1
        else:
            return self.parse_bogus_comment(i)

    def parse_comment(self, i, report=True):
        rawdata = self.rawdata
        assert rawdata.startswith('<!--', i), 'unexpected call to parse_comment()'
        match = commentclose.search(rawdata, i + 4)
        if not match:
            match = commentabruptclose.match(rawdata, i + 4)
            if not match:
                return -1
        if report:
            j = match.start()
            self.handle_comment(rawdata[i + 4:j])
        return match.end()

    def parse_bogus_comment(self, i, report=1):
        rawdata = self.rawdata
        assert rawdata[i:i + 2] in ('<!', '</'), (
            'unexpected call to parse_comment()'
        )
        pos = rawdata.find('>', i + 2)
        if pos == -1:
            return -1
        if report:
            self.handle_comment(rawdata[i + 2:pos])
        return pos + 1

    def parse_pi(self, i):
        rawdata = self.rawdata
        assert rawdata[i:i + 2] == '<?', 'unexpected call to parse_pi()'
        match = piclose.search(rawdata, i + 2)  # >
        if not match:
            return -1
        j = match.start()
        self.handle_pi(rawdata[i + 2:j])
        j = match.end()
        return j

    def parse_starttag(self, i):
        self.__starttag_text = None
        endpos = self.check_for_whole_start_tag(i)
        if endpos < 0:
            return endpos
        rawdata = self.rawdata
        self.__starttag_text = rawdata[i:endpos]

        attrs = []
        match = tagfind_tolerant.match(rawdata, i + 1)
        assert match, 'unexpected call to parse_starttag()'
        k = match.end()
        self.lasttag = tag = match.group(1).lower()
        while k < endpos:
            m = attrfind_tolerant.match(rawdata, k)
            if not m:
                break
            attrname, rest, attrvalue = m.group(1, 2, 3)
            if not rest:
                attrvalue = None
            elif (attrvalue[:1] == '\'' == attrvalue[-1:] or
                  attrvalue[:1] == '"' == attrvalue[-1:]):
                attrvalue = attrvalue[1:-1]
            if attrvalue:
                attrvalue = unescape(attrvalue)
            attrs.append((attrname.lower(), attrvalue))
            k = m.end()

        end = rawdata[k:endpos].strip()
        if end not in (">", "/>"):
            self.handle_data(rawdata[i:endpos])
            return endpos
        if end.endswith('/>'):
            self.handle_startendtag(tag, attrs)
        else:
            self.handle_starttag(tag, attrs)
            if (tag in self.CDATA_CONTENT_ELEMENTS or
                    (self.scripting and tag == "noscript") or
                    tag == "plaintext"):
                self.set_cdata_mode(tag, escapable=False)
            elif tag in self.RCDATA_CONTENT_ELEMENTS:
                self.set_cdata_mode(tag, escapable=True)
        return endpos

    def check_for_whole_start_tag(self, i):
        rawdata = self.rawdata
        match = locatetagend.match(rawdata, i + 1)
        assert match
        j = match.end()
        if rawdata[j - 1] != ">":
            return -1
        return j

    def parse_endtag(self, i):
        rawdata = self.rawdata
        assert rawdata[i:i + 2] == "</", "unexpected call to parse_endtag"
        if rawdata.find('>', i + 2) < 0:  # fast check
            return -1
        if not endtagopen.match(rawdata, i):  # </ + letter
            if rawdata[i + 2:i + 3] == '>':  # </> is ignored
                return i + 3
            else:
                return self.parse_bogus_comment(i)

        match = locatetagend.match(rawdata, i + 2)
        assert match
        j = match.end()
        if rawdata[j - 1] != ">":
            return -1

        match = tagfind_tolerant.match(rawdata, i + 2)
        assert match
        tag = match.group(1).lower()
        self.handle_endtag(tag)
        self.clear_cdata_mode()
        return j

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)
        self.handle_endtag(tag)

    def handle_starttag(self, tag, attrs):
        pass

    def handle_endtag(self, tag):
        pass

    def handle_charref(self, name):
        pass

    def handle_entityref(self, name):
        pass

    def handle_data(self, data):
        pass

    def handle_comment(self, data):
        pass

    def handle_decl(self, decl):
        pass

    def handle_pi(self, data):
        pass

    def unknown_decl(self, data):
        pass
