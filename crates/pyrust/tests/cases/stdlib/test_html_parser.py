# CPython 3.12 parity tests for `html.parser` (issue #3149).

import sys
import html
import html.parser
from html.parser import HTMLParser

# 1. Module identity & __all__
print("mod_id:", html.parser is sys.modules["html.parser"], html.parser.HTMLParser is HTMLParser)
print("parser_all:", html.parser.__all__)


class Recorder(HTMLParser):
    def __init__(self, *, convert_charrefs=True, record_startend=False, record_pos=False):
        super().__init__(convert_charrefs=convert_charrefs)
        self.events = []
        self._record_startend = record_startend
        self._record_pos = record_pos

    def _rec(self, kind, *payload):
        if self._record_pos:
            self.events.append((kind, self.getpos()) + payload)
        else:
            self.events.append((kind,) + payload)

    def handle_starttag(self, tag, attrs):
        self._rec("start", tag, attrs)

    def handle_endtag(self, tag):
        self._rec("end", tag)

    def handle_startendtag(self, tag, attrs):
        if self._record_startend:
            self._rec("startend", tag, attrs)
        else:
            super().handle_startendtag(tag, attrs)

    def handle_data(self, data):
        self._rec("data", data)

    def handle_comment(self, data):
        self._rec("comment", data)

    def handle_decl(self, decl):
        self._rec("decl", decl)

    def handle_pi(self, data):
        self._rec("pi", data)

    def handle_entityref(self, name):
        self._rec("entityref", name)

    def handle_charref(self, name):
        self._rec("charref", name)

    def unknown_decl(self, data):
        self._rec("unknown_decl", data)


def parse_all(doc, **kwargs):
    p = Recorder(**kwargs)
    p.feed(doc)
    p.close()
    return p.events


# 2. Full HTML document: doctype, comments, PI, tags, attributes, entities
doc = (
    "<!DOCTYPE html>\n"
    "<?xml version='1.0'?>\n"
    "<!-- top comment -->\n"
    "<HTML lang=\"en\">\n"
    "  <head><meta charset=utf-8 /><link rel='icon' href=\"/a?x=1&amp;y=2\" disabled></head>\n"
    "  <body>\n"
    "    <Div CLASS=\"main\" data-empty=\"\" data-bare=42>Hello &amp; &#65; &#x42;!</Div>\n"
    "    <br/>\n"
    "  </body>\n"
    "</HTML>"
)
print("full_doc:", parse_all(doc))
print("full_doc_startend:", parse_all(doc, record_startend=True))

# 3. convert_charrefs=False
charref_doc = "<p>a &amp; b &#65; &#x42; &copy; &unknown; &</p>"
print("charrefs_true:", parse_all(charref_doc, convert_charrefs=True))
print("charrefs_false:", parse_all(charref_doc, convert_charrefs=False))

# 4. Rawtext elements: <script> and <style>
raw_doc = (
    '<script type="text/javascript">'
    'if (a < b && c > d) { var s = "<div>&amp;</div>"; }'
    "</SCRIPT>"
    "<style>.a > .b { content: '<x>&lt;</x>'; }</Style>"
    "<p>after &lt;ok&gt;</p>"
)
print("rawtext_true:", parse_all(raw_doc, convert_charrefs=True))
print("rawtext_false:", parse_all(raw_doc, convert_charrefs=False))

# 5. CDATA / marked sections (unknown_decl)
print("cdata:", parse_all("<div><![CDATA[raw <tag> &amp; text]]></div>"))

# 6. getpos() and get_starttag_text()
class PosParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.info = []

    def handle_starttag(self, tag, attrs):
        self.info.append(("start", tag, self.getpos(), self.get_starttag_text()))

    def handle_endtag(self, tag):
        self.info.append(("end", tag, self.getpos()))

pp = PosParser()
pp.feed("<div>\n  <A  HREF='x'>hi</a>\n</div>")
pp.close()
print("pos_and_text:", pp.info)

# 7. Chunked incremental .feed() — split across tags, attrs, comments, entities, script
chunked_doc = (
    "<!DOCTYPE html>"
    "<p title=\"a &amp; b\">Hello &copy; &#x41;<!-- mid\ncomment -->"
    "<script>x < y</script><br/></p>"
)
whole = parse_all(chunked_doc, record_startend=True)

for chunk_size in [1, 2, 3, 5, 11]:
    p = Recorder(record_startend=True)
    for i in range(0, len(chunked_doc), chunk_size):
        p.feed(chunked_doc[i : i + chunk_size])
    p.close()
    # Coalesce adjacent data events for chunk-boundary invariance
    def coalesce(evts):
        out = []
        for ev in evts:
            if ev[0] == "data" and out and out[-1][0] == "data":
                out[-1] = ("data", out[-1][1] + ev[1])
            else:
                out.append(ev)
        return out
    print("chunk_match", chunk_size, coalesce(p.events) == coalesce(whole))

# 8. reset() reuse
rp = Recorder()
rp.feed("<p>first</p>")
rp.close()
first_events = list(rp.events)
rp.reset()
rp.events = []
rp.feed("<span>second</span>")
rp.close()
print("reset_reuse:", first_events, rp.events, rp.getpos())

# 9. Bare HTMLParser (default no-op handlers) doesn't crash
bare = HTMLParser()
bare.feed("<!DOCTYPE html><!-- c --><?pi?><p a=1>t &amp; &#65;<br/></p>")
bare.close()
print("bare_lasttag:", bare.lasttag)
