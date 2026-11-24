# CPython 3.12 parity tests for `html` and `html.entities` (issue #3149).

import sys
import html
import html.entities
from html import escape, unescape
from html.entities import codepoint2name, entitydefs, html5, name2codepoint

# 1. Package & submodule identity and __all__
print("mod_id:", html is sys.modules["html"], html.entities is sys.modules["html.entities"])
print("html_all:", html.__all__)
print("entities_all:", html.entities.__all__)

# 2. html.escape
print("escape_default:", escape('<a href="x" title=\'y\'>a & b</a>'))
print("escape_no_quote:", escape('<a href="x" title=\'y\'>a & b</a>', quote=False))
print("escape_empty:", repr(escape("")))
print("escape_plain:", escape("hello world 123"))
print("escape_already:", escape("&amp;&lt;&gt;"))
print("escape_unicode:", escape("café <日本語> & '🚀'"))

# 3. html.unescape — named entities
print("unesc_basic:", unescape("&amp;&lt;&gt;&quot;&#x27;&apos;"))
print("unesc_named:", unescape("&copy; &copy &nbsp; &euro; &mdash; &alpha; &Omega;"))
print("unesc_multi_cp:", repr(unescape("&NotEqualTilde;")), repr(unescape("&acE;")))
print("unesc_prefix:", repr(unescape("&notit;")), repr(unescape("&notin;")), repr(unescape("&ampfoo")))
print("unesc_unknown:", repr(unescape("&notanentity; &foo &123;")))

# 4. html.unescape — numeric decimal & hex charrefs
print("unesc_dec:", unescape("&#65;&#66;&#67; &#9731; &#128512; &#00065; &#65"))
print("unesc_hex:", unescape("&#x41;&#X42;&#x27; &#x1f600; &#x1F600 &#x0041;"))

# 5. html.unescape — HTML5 invalid charrefs & Windows-1252 remapping
for ref in [
    "&#0;",
    "&#x0;",
    "&#x0d;",
    "&#x80;",
    "&#x82;",
    "&#x91;",
    "&#x92;",
    "&#x93;",
    "&#x94;",
    "&#x97;",
    "&#x99;",
    "&#x9f;",
    "&#xD800;",
    "&#xDFFF;",
    "&#x110000;",
    "&#99999999;",
    "&#x01;",
    "&#x0b;",
    "&#x7f;",
    "&#xfdd0;",
    "&#xfffe;",
    "&#xffff;",
    "&#x10ffff;",
]:
    print("invalid_ref:", ref, repr(unescape(ref)))

# 6. html.unescape — edge cases & round-trip
for edge in [
    "",
    "no ampersands here",
    "&",
    "&&",
    "&#",
    "&#x",
    "&#;",
    "&#x;",
    "&&amp;",
    "a & b & c",
    "&" + "a" * 40 + ";",
    "&amp&lt&gt",
]:
    print("edge:", repr(edge), "->", repr(unescape(edge)))

rt = '<script>alert("1 & 2" + \'3\')</script>'
print("roundtrip:", unescape(escape(rt)) == rt)

# 7. html.entities tables
print("table_lens:", len(html5), len(name2codepoint), len(codepoint2name), len(entitydefs))
print("html5_spot:", repr(html5["amp;"]), repr(html5["amp"]), repr(html5["copy;"]), repr(html5["copy"]))
print("n2cp_spot:", name2codepoint["amp"], name2codepoint["lt"], name2codepoint["copy"], name2codepoint["euro"])
print("cp2n_spot:", codepoint2name[38], codepoint2name[60], codepoint2name[169], codepoint2name[8364])
print("edefs_spot:", repr(entitydefs["amp"]), repr(entitydefs["copy"]), repr(entitydefs["euro"]))
print(
    "tables_consistent:",
    all(codepoint2name[cp] == name and entitydefs[name] == chr(cp) for name, cp in name2codepoint.items()),
)
