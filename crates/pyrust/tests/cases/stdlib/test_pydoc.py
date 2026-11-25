# Cross-platform CPython 3.12 parity fixture for `pydoc`, `pydoc_data`,
# `pydoc_data.topics`, and interactive `help()` (issue #3255).

import builtins
import contextlib
import io
import os
import pydoc
import pydoc_data
import pydoc_data.topics
import reprlib
import sys
import tempfile
import types
import warnings


# ── 1. `pydoc_data` and `pydoc_data.topics` ──────────────────────────────────

print("pydoc_data.topics identity:", pydoc_data.topics is sys.modules["pydoc_data.topics"])
print("topics is dict:", isinstance(pydoc_data.topics.topics, dict))
topic_keys = sorted(pydoc_data.topics.topics.keys())
print("topics count:", len(topic_keys))
print("topics first/last:", topic_keys[0], topic_keys[-1])
print(
    "topics all non-empty str:",
    all(isinstance(v, str) and len(v.strip()) > 0 for v in pydoc_data.topics.topics.values()),
)
for k in (
    "assert",
    "assignment",
    "async",
    "await",
    "context-managers",
    "exceptions",
    "operator-summary",
    "types",
    "with",
    "yield",
):
    print(f"topic {k} heading:", pydoc_data.topics.topics[k].splitlines()[0])


# ── 2. `ErrorDuringImport` ───────────────────────────────────────────────────

val_err = ValueError("bad import")
edi = pydoc.ErrorDuringImport("foo.py", val_err)
print("edi isinstance:", isinstance(edi, Exception))
print("edi attrs:", edi.filename, edi.exc is ValueError, edi.value is val_err, edi.tb is None)
print("edi str:", str(edi))

with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    edi2 = pydoc.ErrorDuringImport("bar.py", (KeyError, KeyError("k"), None))
print("edi2 deprecated warning:", len(caught), caught[0].category is DeprecationWarning)
print("edi2 str:", str(edi2))


# ── 3. Core helper functions ─────────────────────────────────────────────────

pdirs = pydoc.pathdirs()
print("pathdirs list of existing abs dirs:", isinstance(pdirs, list), all(os.path.isabs(d) and os.path.isdir(d) for d in pdirs))

def sample_fn(a, b=42, *args, kw="hi", **kwargs):
    """Add two numbers.

    Detailed explanation here.
    """
    return a + b

async def sample_coro(x):
    """Async sample."""
    return x

class BaseDemo:
    """Base class doc."""

    BASE_CONST = 10

    def base_method(self):
        """Inherited docstring."""
        return 1

class SubDemo(BaseDemo):
    """Sub class doc.

    More details about SubDemo.
    """

    SUB_CONST = "hello"

    def __init__(self, x, y=2):
        """Initialize SubDemo."""
        self.x = x
        self.y = y

    def base_method(self):
        return 2

    def compute(self, factor=1):
        """Compute scaled value."""
        return self.x * factor

    @classmethod
    def make(cls, val):
        """Construct from val."""
        return cls(val)

    @staticmethod
    def helper_static(n):
        """Double n."""
        return n * 2

    @property
    def readonly_prop(self):
        """Read-only property doc."""
        return self.x

    def _get_rw(self):
        return self.y

    def _set_rw(self, v):
        self.y = v

    rw_prop = property(_get_rw, _set_rw, doc="Read-write property doc.")

print("getdoc fn:", repr(pydoc.getdoc(sample_fn)))
print("getdoc inherited method:", repr(pydoc.getdoc(SubDemo.base_method)))
print("getdoc no doc:", repr(pydoc.getdoc(lambda x: x)))

print("splitdoc single:", pydoc.splitdoc("One line"))
print("splitdoc two with blank:", pydoc.splitdoc("Summary line\n\nBody line 1\nBody line 2"))
print("splitdoc no blank:", pydoc.splitdoc("Line 1\nLine 2"))
print("splitdoc empty:", pydoc.splitdoc(""))

print("classname same mod:", pydoc.classname(SubDemo, SubDemo.__module__))
print("classname other mod:", pydoc.classname(SubDemo, "other_mod"))
print("parentname method:", pydoc.parentname(SubDemo.compute, SubDemo.__module__))
print("parentname method other mod:", pydoc.parentname(SubDemo.compute, "other_mod"))

print(
    "isdata:",
    pydoc.isdata(42),
    pydoc.isdata("abc"),
    pydoc.isdata(SubDemo),
    pydoc.isdata(sample_fn),
    pydoc.isdata(sys),
    pydoc.isdata(SubDemo(1)),
)
print("replace:", pydoc.replace("a < b & c > d", "&", "&amp;", "<", "&lt;", ">", "&gt;"))
print("cram short:", pydoc.cram("hello", 10))
print("cram long:", pydoc.cram("0123456789abcdef", 10))
print("cram tiny:", pydoc.cram("abcdef", 3))
print("stripid:", pydoc.stripid("<foo object at 0x7f1234ab5678>"))
print("stripid double gt:", pydoc.stripid("<foo at 0xDEADBEEF>>"))
print("stripid no match:", pydoc.stripid("<foo at 0x123>"))
print("plain:", repr(pydoc.plain("H\bHe\bel\bll\blo\bo")))
print("plain keeps newline:", repr(pydoc.plain("A\n\bB")))

methods_map = pydoc.allmethods(SubDemo)
print("allmethods contains:", "compute" in methods_map, "base_method" in methods_map, "make" in methods_map)

print(
    "visiblename:",
    pydoc.visiblename("public"),
    pydoc.visiblename("_private"),
    pydoc.visiblename("__init__"),
    pydoc.visiblename("__doc__"),
    pydoc.visiblename("_secret", all=["_secret"]),
    pydoc.visiblename("other", all=["_secret"]),
)

classified = {name: kind for name, kind, cls, val in pydoc.classify_class_attrs(SubDemo)}
print(
    "classify_class_attrs:",
    classified.get("compute"),
    classified.get("make"),
    classified.get("helper_static"),
    classified.get("readonly_prop"),
    classified.get("rw_prop"),
    classified.get("SUB_CONST"),
)

print("describe sys:", pydoc.describe(sys))
print("describe len:", pydoc.describe(len))
print("describe class:", pydoc.describe(SubDemo))
print("describe fn:", pydoc.describe(sample_fn))
print("describe method:", pydoc.describe(SubDemo(1).compute))
print("describe int inst:", pydoc.describe(123))

print("locate builtins int:", pydoc.locate("int") is int)
print("locate sys.version_info:", pydoc.locate("sys.version_info") is sys.version_info)
print("locate missing:", pydoc.locate("no_such_module_3255.foo"))

res_obj, res_name = pydoc.resolve("sys")
print("resolve str:", res_obj is sys, res_name)
res_obj2, res_name2 = pydoc.resolve(sample_fn)
print("resolve fn:", res_obj2 is sample_fn, res_name2)
try:
    pydoc.resolve("definitely_missing_xyz_3255")
except ImportError as exc:
    print("resolve missing:", "No Python documentation found" in str(exc))


# ── 4. `source_synopsis`, `synopsis`, `ispackage`, `safeimport`, `importfile`

sio_syn = io.StringIO("#!/usr/bin/env python3\n# comment\n\n\"\"\"Quick module summary.\n\nMore.\n\"\"\"\nX = 1\n")
print("source_synopsis:", repr(pydoc.source_synopsis(sio_syn)))
sio_raw = io.StringIO("r\"\"\"Raw summary.\"\"\"\n")
print("source_synopsis raw:", repr(pydoc.source_synopsis(sio_raw)))
sio_none = io.StringIO("x = 1\n")
print("source_synopsis none:", repr(pydoc.source_synopsis(sio_none)))

with tempfile.TemporaryDirectory() as td:
    pkg_dir = os.path.join(td, "demo_pkg_3255")
    os.makedirs(pkg_dir)
    print("ispackage before init:", pydoc.ispackage(pkg_dir))
    init_py = os.path.join(pkg_dir, "__init__.py")
    with open(init_py, "w", encoding="utf-8") as f:
        f.write('"""Demo package synopsis."""\nVALUE = 99\n')
    print("ispackage after init:", pydoc.ispackage(pkg_dir))
    print("ispackage file:", pydoc.ispackage(init_py))

    syn_cache = {}
    print("synopsis first:", repr(pydoc.synopsis(init_py, syn_cache)))
    print("synopsis cached:", init_py in syn_cache, repr(pydoc.synopsis(init_py, syn_cache)))

    mod_py = os.path.join(td, "demo_mod_3255.py")
    with open(mod_py, "w", encoding="utf-8") as f:
        f.write(
            '"""Demo module for pydoc tests.\n\nExtended description.\n"""\n'
            'ANSWER = 42\n'
            'def greet(who="world"):\n'
            '    """Say hello."""\n'
            '    return f"hello {who}"\n'
            'class Greeter:\n'
            '    """A greeter class."""\n'
            '    def hi(self):\n'
            '        """Greet."""\n'
            '        return "hi"\n'
        )
    imported = pydoc.importfile(mod_py)
    print("importfile:", imported.__name__, imported.ANSWER, imported.greet("py"))

    broken_py = os.path.join(td, "broken_3255.py")
    with open(broken_py, "w", encoding="utf-8") as f:
        f.write("raise RuntimeError('boom')\n")
    try:
        pydoc.importfile(broken_py)
    except pydoc.ErrorDuringImport as err:
        print("importfile error:", err.exc is RuntimeError, "boom" in str(err))

    saved_path = list(sys.path)
    try:
        sys.path.insert(0, td)
        m1 = pydoc.safeimport("demo_mod_3255")
        print("safeimport ok:", m1 is not None, m1.ANSWER)
        print("safeimport missing:", pydoc.safeimport("no_such_mod_3255"))
        print("describe file mod:", pydoc.describe(m1))

        # Test writedoc in temp directory
        saved_cwd = os.getcwd()
        try:
            os.chdir(td)
            wd_buf = io.StringIO()
            with contextlib.redirect_stdout(wd_buf):
                pydoc.writedoc("demo_mod_3255")
            print("writedoc stdout:", wd_buf.getvalue().strip())
            html_path = os.path.join(td, "demo_mod_3255.html")
            print("writedoc exists:", os.path.isfile(html_path))
            with open(html_path, "r", encoding="utf-8") as hf:
                html_text = hf.read()
            print(
                "writedoc content:",
                "<!DOCTYPE html>" in html_text,
                "demo_mod_3255" in html_text,
                "Greeter" in html_text,
                "greet" in html_text,
            )
        finally:
            os.chdir(saved_cwd)
    finally:
        sys.path[:] = saved_path
        sys.modules.pop("demo_mod_3255", None)


# ── 5. `Doc`, `TextRepr`, `HTMLRepr`, `TextDoc`, `_PlainTextDoc`, `HTMLDoc` ──

base_doc = pydoc.Doc()
print("Doc.docmodule is Doc.fail:", pydoc.Doc.docmodule is pydoc.Doc.fail)
try:
    base_doc.document(42, "x")
except TypeError as exc:
    print("Doc.fail:", str(exc))

tr = pydoc.TextRepr()
hr = pydoc.HTMLRepr()
print("Repr subclasses:", isinstance(tr, reprlib.Repr), isinstance(hr, reprlib.Repr))
print("TextRepr limits:", tr.maxlist, tr.maxdict, tr.maxstring)
print("TextRepr raw str:", tr.repr("a\\b"))
print("HTMLRepr escape:", hr.escape("<a&b>"))
print("HTMLRepr repr str:", hr.repr("a\nb"))

tdoc = pydoc.TextDoc()
pdoc = pydoc._PlainTextDoc()
hdoc = pydoc.HTMLDoc()

print("TextDoc.bold:", repr(tdoc.bold("Hi")))
print("_PlainTextDoc.bold:", repr(pdoc.bold("Hi")))
print("TextDoc.indent:", repr(tdoc.indent("a\nb\n", "  ")))
print("TextDoc.section:", repr(pydoc.plain(tdoc.section("TITLE", "body"))))
print("TextDoc.formatvalue:", tdoc.formatvalue(123))
print("TextDoc.docother:", pydoc.plain(tdoc.docother(42, "X")))
print("TextDoc.docroutine:", "sample_fn(a, b=42" in pydoc.plain(tdoc.docroutine(sample_fn)))
print("TextDoc.docroutine async:", pydoc.plain(tdoc.docroutine(sample_coro)).startswith("async sample_coro"))

tdoc_cls = pydoc.plain(tdoc.docclass(SubDemo))
print(
    "TextDoc.docclass sections:",
    "class SubDemo(BaseDemo)" in tdoc_cls,
    "Method resolution order:" in tdoc_cls,
    "Methods defined here:" in tdoc_cls,
    "Class methods defined here:" in tdoc_cls,
    "Static methods defined here:" in tdoc_cls,
    "Readonly properties defined here:" in tdoc_cls,
    "Data descriptors defined here:" in tdoc_cls,
    "Data and other attributes defined here:" in tdoc_cls,
)

# Synthetic module rendering (TextDoc + HTMLDoc)
synth = types.ModuleType("synth_demo_3255")
synth.__doc__ = "Synthetic module synopsis.\n\nSynthetic body paragraph."
synth.MY_VAL = 123
synth.sample_fn = sample_fn
sample_fn.__module__ = "synth_demo_3255"
synth.SubDemo = SubDemo
SubDemo.__module__ = "synth_demo_3255"
sys.modules["synth_demo_3255"] = synth
try:
    mod_txt = pydoc.plain(tdoc.docmodule(synth))
    print(
        "TextDoc.docmodule:",
        "NAME" in mod_txt,
        "synth_demo_3255 - Synthetic module synopsis." in mod_txt,
        "DESCRIPTION" in mod_txt,
        "CLASSES" in mod_txt,
        "FUNCTIONS" in mod_txt,
        "DATA" in mod_txt,
        "MY_VAL = 123" in mod_txt,
        "FILE" in mod_txt,
    )

    mod_html = hdoc.docmodule(synth)
    print(
        "HTMLDoc.docmodule:",
        "<strong class=\"title\">synth_demo_3255</strong>" in mod_html,
        "Classes" in mod_html,
        "Functions" in mod_html,
        "Data" in mod_html,
    )
    page_html = hdoc.page("synth", mod_html)
    print("HTMLDoc.page:", page_html.startswith("<!DOCTYPE html>"), "<title>Python: synth</title>" in page_html)

    print("HTMLDoc.escape:", hdoc.escape("x < y & z > 0"))
    print("HTMLDoc.preformat:", hdoc.preformat("a b\nc"))
    print("HTMLDoc.grey:", hdoc.grey("note"))
    print("HTMLDoc.formatvalue:", hdoc.formatvalue(7))
    print(
        "HTMLDoc.markup links:",
        'href="https://peps.python.org/pep-0008/"' in hdoc.markup("See PEP 8"),
        'href="https://www.rfc-editor.org/rfc/rfc2616.txt"' in hdoc.markup("See RFC 2616"),
        'href="https://example.com/a"' in hdoc.markup("Visit https://example.com/a."),
        "self.<strong>attr</strong>" in hdoc.markup("Uses self.attr here"),
    )

    # render_doc & doc(output=...)
    rd_mod = pydoc.plain(pydoc.render_doc(synth))
    print("render_doc module:", rd_mod.startswith("Python Library Documentation: module synth_demo_3255"))
    rd_fn = pydoc.plain(pydoc.render_doc(sample_fn))
    print("render_doc fn:", rd_fn.startswith("Python Library Documentation: function sample_fn in module synth_demo_3255"))
    rd_cls = pydoc.plain(pydoc.render_doc(SubDemo))
    print("render_doc cls:", rd_cls.startswith("Python Library Documentation: class SubDemo in module synth_demo_3255"))
    rd_inst = pydoc.plain(pydoc.render_doc(SubDemo(1)))
    print("render_doc inst:", "SubDemo object" in rd_inst.splitlines()[0])

    doc_buf = io.StringIO()
    pydoc.doc(sample_fn, output=doc_buf)
    print("doc output:", "sample_fn(a, b=42" in doc_buf.getvalue(), "\b" not in doc_buf.getvalue())

    miss_buf = io.StringIO()
    pydoc.doc("no_such_target_3255", output=miss_buf)
    print("doc missing output:", "No Python documentation found" in miss_buf.getvalue())
finally:
    sys.modules.pop("synth_demo_3255", None)


# ── 6. `Helper`, interactive `help()`, topics, keywords, symbols ─────────────

print("module singletons:", isinstance(pydoc.text, pydoc.TextDoc), isinstance(pydoc.plaintext, pydoc._PlainTextDoc), isinstance(pydoc.html, pydoc.HTMLDoc), isinstance(pydoc.help, pydoc.Helper))
print("Helper.keywords count:", len(pydoc.Helper.keywords))
print("Helper.topics count:", len(pydoc.Helper.topics))
print("Helper.symbols count:", len(pydoc.Helper.symbols))

h_buf = io.StringIO()
h_inst = pydoc.Helper(output=h_buf)
print("Helper repr:", repr(h_inst))
h_inst.help(sample_fn)
print("Helper.help(fn):", "Help on function sample_fn" in h_buf.getvalue())

kw_buf = io.StringIO()
pydoc.Helper(output=kw_buf).help("keywords")
print("Helper listkeywords:", "Here is a list of the Python keywords." in kw_buf.getvalue(), "yield" in kw_buf.getvalue())

tp_buf = io.StringIO()
pydoc.Helper(output=tp_buf).help("topics")
print("Helper listtopics:", "Here is a list of available topics." in tp_buf.getvalue(), "ASSERTION" in tp_buf.getvalue())

sy_buf = io.StringIO()
pydoc.Helper(output=sy_buf).help("symbols")
print("Helper listsymbols:", "punctuation symbols" in sy_buf.getvalue(), "**=" in sy_buf.getvalue())

# Topic, keyword, symbol routing via pager -> stdout
for req, expect_word in [("ASSERTION", "assert"), ("def", "Function"), ("+", "Operator")]:
    h_out = io.StringIO()
    std_out = io.StringIO()
    with contextlib.redirect_stdout(std_out):
        pydoc.Helper(output=h_out).help(req)
    print(f"Helper topic {req!r}:", expect_word.lower() in std_out.getvalue().lower(), len(std_out.getvalue()) > 20)

# Helper._gettopic
tdoc_text, txrefs = pydoc.Helper()._gettopic("ASSERTION")
print("_gettopic ASSERTION:", "assert" in tdoc_text.lower(), repr(txrefs))
tdoc_text2, txrefs2 = pydoc.Helper()._gettopic("with")
print("_gettopic with:", "with" in tdoc_text2.lower(), "CONTEXTMANAGERS" in txrefs2)

# Helper.interact() with simulated input
in_stream = io.StringIO("keywords\nquit\n")
out_stream = io.StringIO()
pydoc.Helper(input=in_stream, output=out_stream)()
interact_text = out_stream.getvalue()
print(
    "Helper interactive session:",
    "Welcome to Python" in interact_text,
    "help> " in interact_text,
    "Here is a list of the Python keywords." in interact_text,
    "You are now leaving help" in interact_text,
)

# builtins.help() wired via _sitebuiltins._Helper -> pydoc.help
b_out = io.StringIO()
with contextlib.redirect_stdout(b_out):
    builtins.help(sample_fn)
print("builtins.help(fn):", "Help on function sample_fn" in b_out.getvalue(), "Add two numbers." in b_out.getvalue())


# ── 7. Additional edge cases: lambda, plainpager, getpager, writedocs, topics

lam = lambda a, b=1: a + b
print("lambda docroutine:", pydoc.plain(tdoc.docroutine(lam, "my_lam")).strip())
print("lambda htmldoc:", "<em>lambda</em>" in hdoc.docroutine(lam, "my_lam"))

pp_buf = io.StringIO()
with contextlib.redirect_stdout(pp_buf):
    pydoc.plainpager("B\bBo\bol\bld\bd text")
print("plainpager:", repr(pp_buf.getvalue()))
print("getpager callable:", callable(pydoc.getpager()))

# Verify every Helper.topics and Helper.keywords label exists in pydoc_data.topics.topics
all_labels_ok = True
for mapping in (pydoc.Helper.topics, pydoc.Helper.keywords):
    for k, v in mapping.items():
        if isinstance(v, tuple):
            if v[0] not in pydoc_data.topics.topics:
                all_labels_ok = False
print("all Helper topic/keyword labels resolve:", all_labels_ok)

# HTMLDoc helpers: section, bigsection, multicolumn
mc = hdoc.multicolumn(["a", "b", "c", "d", "e"], lambda x: f"<i>{x}</i>")
print("multicolumn:", "<table><tr>" in mc, "<i>e</i>" in mc)
sec_html = hdoc.bigsection("MyTitle", "index", "body_content")
print("bigsection:", "bigsection" in sec_html, "MyTitle" in sec_html, "body_content" in sec_html)

# writedocs + apropos on a temporary directory
with tempfile.TemporaryDirectory() as td2:
    with open(os.path.join(td2, "aprop_demo_3255.py"), "w", encoding="utf-8") as f:
        f.write('"""UniqueZebraToken3255 synopsis line."""\nX = 1\n')
    saved_path2 = list(sys.path)
    saved_cwd2 = os.getcwd()
    try:
        sys.path.insert(0, td2)
        os.chdir(td2)
        wd2_buf = io.StringIO()
        with contextlib.redirect_stdout(wd2_buf):
            pydoc.writedocs(td2)
        print("writedocs output:", "wrote aprop_demo_3255.html" in wd2_buf.getvalue())
        print("writedocs file:", os.path.isfile(os.path.join(td2, "aprop_demo_3255.html")))
    finally:
        os.chdir(saved_cwd2)
        sys.path[:] = saved_path2
        sys.modules.pop("aprop_demo_3255", None)

