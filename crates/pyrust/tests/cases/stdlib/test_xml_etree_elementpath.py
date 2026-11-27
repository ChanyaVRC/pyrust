# Parity test for Python 3.12 xml.etree.ElementPath (the XPath subset used by
# Element.find / findall / findtext / iterfind).

import sys
import warnings

import xml.etree.ElementPath as EP
import xml.etree.ElementTree as ET


def show(label, func, *args, **kwargs):
    """Print the result of func(*args, **kwargs) or the exception it raises."""
    try:
        result = func(*args, **kwargs)
    except Exception as exc:  # noqa: BLE001 - parity of the exception itself
        print(label, "!!", type(exc).__name__, exc)
        return None
    print(label, "->", result)
    return result


def tags(elems):
    return None if elems is None else [e.tag if isinstance(e.tag, str) else "<%s>" % e.tag.__name__ for e in elems]


def ids(elems):
    return [e.get("id") for e in elems]


# ── 1. Module namespace ──────────────────────────────────────────────────────
# Dunders differ by import mechanism (CPython's module is a .py file with
# __file__ / __cached__), so only the module's own names are compared.
print("names:", [n for n in vars(EP) if not n.startswith("__")])
print("package:", EP.__package__, "doc:", EP.__doc__)
print("re:", EP.re is sys.modules["re"])
print("pattern:", repr(EP.xpath_tokenizer_re.pattern))
print("ops:", list(EP.ops))
print("ops_identity:", EP.ops[""] is EP.prepare_child, EP.ops["*"] is EP.prepare_star,
      EP.ops["."] is EP.prepare_self, EP.ops[".."] is EP.prepare_parent,
      EP.ops["//"] is EP.prepare_descendant, EP.ops["["] is EP.prepare_predicate)
print("cache_type:", type(EP._cache).__name__)
print("context_cls:", EP._SelectorContext.__name__, EP._SelectorContext.parent_map)
print("et_uses_ep:", ET.ElementPath is EP)

# ── 2. xpath_tokenizer ───────────────────────────────────────────────────────


def check(p, namespaces=None):
    try:
        out = [op or tag for op, tag in EP.xpath_tokenizer(p, namespaces)]
    except Exception as exc:  # noqa: BLE001
        print("tok", repr(p), "!!", type(exc).__name__, exc)
        return
    print("tok", repr(p), out)


# tests from the xml specification
for p in ["*", "text()", "@name", "@*", "para[1]", "para[last()]", "*/para",
          "/doc/chapter[5]/section[2]", "chapter//para", "//para",
          "//olist/item", ".", ".//para", "..", "../@lang", "chapter[title]",
          "employee[@secretary and @assistant]"]:
    check(p)
# additional tests
for p in ["@{ns}attr", "{http://spam}egg", "./spam.egg", ".//{http://spam}egg",
          "{ns}*", "{}*", "{*}tag", "{*}*", ".//{*}tag", "{a}/b", "{a b}c",
          "{}a}b", "a!b", "!x", "a != 'b'", "[@x='1 2']", "[.=\"q'q\"]",
          "'open", "a'b'c", "a::b", "::b", "a:", ":a", "*a", ".5", "a.b*",
          "x[last()-1]", "x[ last() ]", "\t a 　 b\x1c", "aé[١]",
          "(())", "@@", "=", "", "  "]:
    check(p)
# namespace prefix resolution
xsd = {"xsd": "http://www.w3.org/2001/XMLSchema"}
dflt = {"": "http://www.w3.org/2001/XMLSchema"}
check("./xsd:type", xsd)
check("type", dflt)
check("@xsd:type", xsd)
check("@type", dflt)
check("@{*}type", dflt)
check("@{ns}attr", {"": "http://www.w3.org/2001/XMLSchema", "ns": "http://www.w3.org/2001/XMLSchema"})
check("@ type", dflt)
check("a[1]", dflt)
check("a[last()]", dflt)
check("p:x", None)
check("p:x", {})
check("p:x", {"q": "urn:q"})
check("p:x:y", {"p": "urn:p"})
check("{urn}p:x", None)
check("a/p:x/b", {"p": 42})
check("a", {"": 0})
check("a", {"": ""})
check("a", {"": 7})

# laziness: nothing runs until next(), and errors surface token by token
gen = EP.xpath_tokenizer("a/p:x")
print("tok_type:", type(gen).__name__)
print("tok_first:", next(gen), next(gen))
show("tok_third", next, gen)
show("tok_after_error", next, gen, "done")
show("tok_bad_pattern", list, EP.xpath_tokenizer(5))
show("tok_bytes", list, EP.xpath_tokenizer(b"a"))
show("tok_bad_ns", list, EP.xpath_tokenizer("a", 5))
show("tok_no_args", EP.xpath_tokenizer)
show("tok_kw", lambda: list(EP.xpath_tokenizer(pattern="a", namespaces={"": "u"})))
show("tok_extra", EP.xpath_tokenizer, "a", None, 1)


class NS(dict):
    def __missing__(self, key):
        return "urn:missing-" + key


check("p:x", NS(a="b"))

# ── 3. find / findall / findtext / iterfind ──────────────────────────────────
SAMPLE_XML = """\
<body>
  <tag class='a'>text</tag>
  <tag class='b' />
  <section>
    <tag class='b' id='inner'>subtext</tag>
  </section>
</body>
"""
e = ET.XML(SAMPLE_XML)
print("find_tag:", e.find("tag").tag, EP.find(e, "tag").tag)
print("find_section_tag:", EP.find(e, "section/tag").tag)
print("find_dot_tag:", EP.find(e, "./tag").tag)
print("find_rel:", EP.find(e, "section/tag/../tag").get("id"))
print("findtext:", repr(EP.findtext(e, "tag")), repr(EP.findtext(e, "tag[2]")),
      repr(EP.findtext(e, "section/tag")), repr(EP.findtext(e, "missing")),
      repr(EP.findtext(e, "missing", "dflt")), repr(EP.findtext(e, "section")))
paths = [
    ".", "tag", "*", "*/tag", "./tag", "section", "section/*", "section//*",
    "section/.//*", "*/*", "*//*", "*/./tag", ".//tag", "./section/", "./tag/",
    "*/", "//tag", ".//tag/..", "./section/..", ".//section/..", "tag/..",
    "..", "../tag", "*/..", "section/tag/..", ".//*/..", ".//..",
    "tag[@class]", "tag[@class='a']", 'tag[@class="b"]', "tag[@class!='a']",
    "tag[@class!='b']", "tag[@id]", "tag[@id='inner']", ".//tag[@class]",
    ".//tag[@class='b']", ".//tag[@class!='b']", ".//tag[@id!='x']",
    "section[tag]", "section[tag='subtext']", "section[tag!='subtext']",
    "section[tag='nope']", "section[tag!='nope']", ".//tag[.='subtext']",
    ".//tag[.!='subtext']", ".//tag[.='']", "*[.='text']", "tag[1]", "tag[2]",
    "tag[3]", "tag[last()]", "tag[last()-1]", "tag[last()-2]", ".//tag[1]",
    ".//tag[2]", ".//tag[last()]", ".//tag[last()-1]", "*[1]", "*[2]",
    "*[last()]", "tag[@class='b'][1]", "tag[1][@class='b']", "tag[ 1 ]",
    "tag[٢]", "tag[99999999999999999999]", "tag[last()-99999999999999999999]",
    "section[ tag ]", "section[ tag = 'subtext' ]", ".//tag[@class = 'b']",
    "tag[-x]", "tag[1_0]", "tag[last()-1_0]", "tag[last()-+1]", "*[name]",
    "{}tag", "{*}tag", "{*}*", "{}*", "{x}*", "section/{*}tag", ".//{}tag",
    "  ", "tag ", " tag", "section//", "section/./", ".//*[@id]",
    "*[@class][tag]", "*[tag][@class]",
]
for p in paths:
    show("findall " + repr(p), lambda p=p: tags(EP.findall(e, p)))

bad = [
    "/tag", "tag[0]", "tag[-1]", "tag[-0]", "tag[last()-0]", "tag[last()+1]",
    "tag[last() - 1]", "tag[last()-x]", "tag[foo()]", "tag[a/b]", "tag[a/b='x']",
    "tag[@a=b]", "tag[@a='b'=c]", "tag[]", "tag[@]", "tag[(]", "tag[=]",
    ".//[1]", "section//.", "section//..", "a//@x", "@x", "]", "=", "()",
    "'q'", "::", "!=", "tag[", "tag[1", "tag[@class", "//", "a//", "/", "",
]
for p in bad:
    for name, func in [("find", EP.find), ("findall", EP.findall),
                       ("findtext", EP.findtext), ("iterfind", EP.iterfind)]:
        def run(func=func, p=p):
            result = func(e, p)
            if hasattr(result, "__next__"):
                return tags(result)
            if isinstance(result, list):
                return tags(result)
            return result if result is None or isinstance(result, str) else result.tag
        show(name + " " + repr(p), run)

# non-str paths
for p in [None, 5, b"tag", bytearray(b"tag"), ["tag"], ("tag",), 1.5]:
    show("findall_type " + type(p).__name__, EP.findall, e, p)

# argument binding
show("find_no_args", EP.find)
show("find_one_arg", EP.find, e)
show("find_extra", EP.find, e, "tag", None, 1)
show("find_kw", lambda: EP.find(elem=e, path="tag").tag)
show("find_dup", lambda: EP.find(e, "tag", elem=e))
show("find_unknown_kw", lambda: EP.find(e, "tag", bogus=1))
show("findtext_kw", EP.findtext, e, "missing", namespaces=None, default="D")
show("findtext_extra", EP.findtext, e, "tag", 1, 2, 3)
show("findall_kw", lambda: tags(EP.findall(e, path="tag", namespaces=None)))
show("iterfind_kw", lambda: tags(EP.iterfind(elem=e, path="tag")))

# ── 4. Namespaces ────────────────────────────────────────────────────────────
nsdoc = ET.fromstring(
    '<r xmlns="urn:d" xmlns:a="urn:a" xmlns:b="urn:b">'
    '<x id="1"/><a:x id="2"><y>ok</y><a:y>A</a:y></a:x><b:x id="3"/><z id="4" a:k="v"/>'
    '<!-- c --><?pi data?></r>'
)
nsmap = {"": "urn:d", "p": "urn:a", "q": "urn:b"}
for p, ns in [
    ("x", nsmap), ("p:x", nsmap), ("q:x", nsmap), ("p:x/y", nsmap),
    ("p:x/p:y", nsmap), ("p:x/{urn:d}y", nsmap), ("*", None), ("{*}x", None),
    ("{urn:a}*", None), ("{urn:b}x", None), ("{*}*", None), ("{}*", None),
    ("{urn:d}x", None), ("x", None), ("x", {}), ("{*}y", None), (".//{*}y", None),
    (".//{urn:a}*", None), ("z[@p:k]", nsmap), ("z[@{urn:a}k='v']", nsmap),
    ("x[1]", nsmap), ("p:x[y]", nsmap), ("p:x[p:y='A']", nsmap),
    ("p:x[{urn:d}y='ok']", nsmap), ("r:x", nsmap), ("x", {"": "urn:d", "a": 1}),
]:
    def run(p=p, ns=ns):
        return [(el.tag if isinstance(el.tag, str) else "<%s>" % el.tag.__name__, el.get("id"))
                for el in EP.findall(nsdoc, p, ns)]
    show("ns " + repr(p) + " " + repr(ns), run)
show("ns_mixed_keys", EP.findall, nsdoc, "x", {"": "urn:d", 3: "x"})
show("ns_unhashable", EP.findall, nsdoc, "x", {"": ["urn:d"]})
show("ns_findtext", EP.findtext, nsdoc, "p:x/y", namespaces=nsmap)
print("ns_default_cache_keys:", sorted(k for k in EP._cache if len(k) > 1 and k[0] == "p:x")[:1])

# ── 5. Comments, processing instructions and QName tags ──────────────────────
cdoc = ET.Element("root")
cdoc.append(ET.Comment("comment"))
ET.SubElement(cdoc, "a").text = "A"
cdoc.append(ET.PI("target", "data"))
ET.SubElement(cdoc, "b")
for p in ["*", "{*}*", "{}*", ".//*", ".//{*}*", "a", "*[1]", "a[1]"]:
    show("comments " + repr(p), lambda p=p: tags(EP.findall(cdoc, p)))

qdoc = ET.Element("root")
ET.SubElement(qdoc, ET.QName("q"))
ET.SubElement(qdoc, ET.QName("urn:x", "q"))
for p in ["q", "{*}q", "{urn:x}q", "{urn:x}*", "*"]:
    show("qname " + repr(p), lambda p=p: len(EP.findall(qdoc, p)))

empty_tag = ET.Element("root")
ET.SubElement(empty_tag, "")
ET.SubElement(empty_tag, "x")
show("empty_tag_nons", lambda: tags(EP.findall(empty_tag, "{}*")))
show("empty_tag_child", lambda: tags(EP.findall(empty_tag, " ")))

# ── 6. Laziness of iterfind and the select generators ────────────────────────
it = EP.iterfind(e, ".//tag")
print("iter_type:", type(it).__name__)
print("iter_first:", next(it).get("class"))
print("iter_rest:", [t.get("class") for t in it])
show("iter_done", next, it, "exhausted")
print("iterfind_empty_path:", EP.iterfind(e, ""))
print("iterfind_trailing:", tags(EP.iterfind(e, "section/")))

live = ET.Element("live")
ET.SubElement(live, "c", n="1")
lit = EP.iterfind(live, "c")
ET.SubElement(live, "c", n="2")
print("iter_live:", [c.get("n") for c in lit])


class Boom(ET.Element):
    @property
    def tag(self):
        raise ValueError("tag exploded")

    @tag.setter
    def tag(self, value):
        pass


bdoc = ET.Element("root")
ET.SubElement(bdoc, "ok")
bdoc.append(Boom("x"))
bit = EP.iterfind(bdoc, "ok")
print("boom_first:", next(bit).tag)
show("boom_next", next, bit)
show("boom_after", next, bit, "closed")
show("boom_findall", EP.findall, bdoc, "*[1]")
show("boom_star", lambda: len(EP.findall(bdoc, "*")))

# ── 7. _cache ────────────────────────────────────────────────────────────────
EP._cache.clear()
EP.findall(e, "tag")
EP.findall(e, "tag")
EP.findall(e, "section/")
EP.findall(e, "tag", {"": "urn:d"})
EP.findall(e, "tag", {})
for p in ["/abs", "tag[0]", "tag["]:
    try:
        EP.findall(e, p)
    except Exception:
        pass
keys = list(EP._cache)
print("cache_keys:", keys)
print("cache_values:", [[s.__qualname__ if s is not None else None for s in EP._cache[k]] for k in keys])
EP._cache.clear()
for i in range(10):
    ET.ElementTree(e).find("./" + str(i))
cache_len_10 = len(EP._cache)
for i in range(10):
    ET.ElementTree(e).find("./" + str(i))
print("cache_stable:", len(EP._cache) == cache_len_10, cache_len_10)
for i in range(20):
    ET.ElementTree(e).find("./" + str(i))
print("cache_grows:", len(EP._cache) > cache_len_10)
for i in range(600):
    ET.ElementTree(e).find("./" + str(i))
print("cache_bounded:", len(EP._cache) < 500, len(EP._cache))
# A cached selector is reused, even if it was stored by hand.
EP._cache[("handmade",)] = [EP.prepare_star(None, None)]
print("cache_handmade:", tags(EP.findall(e, "handmade")))
EP._cache[("empty",)] = []
show("cache_empty_findall", lambda: tags(EP.findall(e, "empty")))
show("cache_empty_find", EP.find, e, "empty")
show("cache_empty_iterfind", lambda: tags(EP.iterfind(e, "empty")))
EP._cache.clear()

# ── 8. get_parent_map and _SelectorContext ───────────────────────────────────
ctx = EP._SelectorContext(e)
print("ctx:", ctx.root is e, ctx.parent_map, sorted(vars(ctx)))
pm = EP.get_parent_map(ctx)
print("pm:", type(pm).__name__, len(pm), ctx.parent_map is pm, EP.get_parent_map(ctx) is pm)
print("pm_values:", sorted(p.tag for p in pm.values()))
show("ctx_no_root", EP._SelectorContext)
show("ctx_extra", EP._SelectorContext, e, 1)
show("pm_no_args", EP.get_parent_map)
show("pm_bad", EP.get_parent_map, object())


class Ctx:
    parent_map = None

    def __init__(self, root):
        self.root = root


c2 = Ctx(e)
print("pm_custom_ctx:", len(EP.get_parent_map(c2)), type(c2.parent_map).__name__)

# ── 9. prepare_* and select callables ────────────────────────────────────────
sel = EP.prepare_child(None, ("", "tag"))
print("sel_names:", sel.__name__, sel.__qualname__)
print("sel_call:", tags(sel(ctx, [e])))
print("sel_call_kw:", tags(sel(context=ctx, result=[e])))
show("sel_missing", sel, ctx)
show("sel_extra", sel, ctx, [e], 1)
show("sel_kw_bad", lambda: sel(ctx, [e], other=1))
lazy = sel(ctx, 5)
print("sel_lazy:", type(lazy).__name__)
show("sel_lazy_next", next, lazy)
for name, token in [("prepare_child", ("", "{*}tag")), ("prepare_child", ("", "{}tag")),
                    ("prepare_star", ("*", "")), ("prepare_self", (".", "")),
                    ("prepare_parent", ("..", ""))]:
    s = getattr(EP, name)(None, token)
    print("prep", name, token, s.__qualname__, tags(s(EP._SelectorContext(e), [e.find("section")])))
show("prep_child_no_args", EP.prepare_child)
show("prep_child_bad_tag", EP.prepare_child, None, ("", None))
show("prep_child_short_token", EP.prepare_child, None, ("",))
print("is_wildcard:", [EP._is_wildcard_tag(t) for t in ["{*}a", "{a}*", "a}*", "a", "*", "{*}", ""]])
show("is_wildcard_bad", EP._is_wildcard_tag, None)
for t in ["{*}*", "{}*", "{*}tag", "{ns}*", "a}*"]:
    s = EP._prepare_tag(t)
    print("prepare_tag", t, s.__qualname__, tags(s(None, list(nsdoc))))
show("prepare_tag_plain", EP._prepare_tag, "tag")
show("prepare_tag_none", EP._prepare_tag, None)


def feed(*tokens):
    it = iter(tokens)
    return lambda: next(it)


for toks in [
    [("@", ""), ("", "class"), ("]", "")],
    [("@", ""), ("", "class"), ("=", ""), ("'b'", ""), ("]", "")],
    [("@", ""), ("", "class"), ("!=", ""), ('"b"', ""), ("]", "")],
    [("", "tag"), ("]", "")],
    [("", ""), ("", "tag"), ("", ""), ("]", "")],
    [(".", ""), ("=", ""), ("'text'", ""), ("]", "")],
    [("", "2"), ("]", "")],
    [("", "last"), ("()", ""), ("]", "")],
    [("", "last"), ("()", ""), ("", "-1"), ("]", "")],
    [("", "first"), ("()", ""), ("]", "")],
    [("", "tag")],
    [(None, "tag"), ("]", "")],
]:
    def run(toks=toks):
        s = EP.prepare_predicate(feed(*toks), ("[", ""))
        return None if s is None else (s.__name__, tags(s(EP._SelectorContext(e), list(e))))
    show("pred " + repr(toks), run)
show("desc_empty", EP.prepare_descendant, feed(), ("//", ""))
show("desc_bad", EP.prepare_descendant, feed(("[", "")), ("//", ""))
d = EP.prepare_descendant(feed(("*", "")), ("//", ""))
print("desc_star:", d.__qualname__, tags(d(EP._SelectorContext(e), [e])))
show("desc_not_callable", EP.prepare_descendant, 5, ("//", ""))

# ── 10. Replacing ops entries ────────────────────────────────────────────────
seen = []


def prepare_attr(next, token):
    name = next()
    seen.append((token, name, next.__name__ if hasattr(next, "__name__") else None))

    def select(context, result):
        for elem in result:
            if elem.get(name[1]) is not None:
                yield elem
    return select


EP._cache.clear()
EP.ops["@"] = prepare_attr
try:
    print("ops_custom:", tags(EP.findall(e, "*/@class")), seen)
    print("ops_custom_mixed:", tags(EP.findall(e, ".//tag/@id")))
finally:
    del EP.ops["@"]
    EP._cache.clear()
show("ops_restored", EP.findall, e, "@x")
orig_child = EP.ops[""]


def counting_child(next, token):
    seen.append(("child", token))
    return orig_child(next, token)


EP.ops[""] = counting_child
try:
    del seen[:]
    print("ops_wrapped:", tags(EP.findall(e, "section/tag")), seen)
finally:
    EP.ops[""] = orig_child
    EP._cache.clear()


def stop_prepare(next, token):
    raise StopIteration


EP.ops["@"] = stop_prepare
try:
    show("ops_stop", EP.findall, e, "@x")
finally:
    del EP.ops["@"]
    EP._cache.clear()

# ── 11. Duck-typed elements ──────────────────────────────────────────────────
calls = []


class Node:
    def __init__(self, tag, *children, text=None, **attrib):
        self.tag = tag
        self.children = list(children)
        self.text = text
        self.attrib = attrib

    def __iter__(self):
        calls.append(("iter", self.tag))
        return iter(self.children)

    def iter(self, tag=None):
        calls.append(("walk", self.tag, tag))
        if tag in (None, "*") or self.tag == tag:
            yield self
        for c in self.children:
            yield from c.iter(tag)

    def itertext(self):
        if self.text:
            yield self.text
        for c in self.children:
            yield from c.itertext()

    def get(self, key, default=None):
        calls.append(("get", self.tag, key))
        return self.attrib.get(key, default)

    def find(self, path):
        calls.append(("find", self.tag, path))
        return EP.find(self, path)

    def findall(self, path):
        calls.append(("findall", self.tag, path))
        return EP.findall(self, path)

    def iterfind(self, path):
        calls.append(("iterfind", self.tag, path))
        return EP.iterfind(self, path)


tree = Node("r", Node("a", Node("b", text="x"), k="1"), Node("a", Node("c", text="y")), Node("d"))
for p in ["a", "*", ".//b", ".//*", "a/..", "a[@k]", "a[@k='1']", "a[@k!='1']",
          "a[b]", "a[b='x']", "a[c!='x']", "a[.='y']", "a[2]", "a[last()]", "{*}a"]:
    del calls[:]
    show("duck " + repr(p), lambda p=p: [n.tag for n in EP.findall(tree, p)])
    print("duck_calls", calls)
show("duck_text", EP.findtext, tree, "a/b")
show("duck_text_none", EP.findtext, tree, "d")


class BadText(Node):
    def itertext(self):
        yield "a"
        yield 5


show("duck_bad_text", lambda: EP.findall(Node("r", BadText("a")), "a[.='a5']"))
show("duck_not_iterable", EP.findall, Node("r", object()), "*/x")

# ── 12. Element methods (the C accelerator's plain-tag fast path) ────────────
fe = ET.fromstring("<a><b>1</b><p:x xmlns:p='urn:p'/><c/></a>")
show("el_find_colon", fe.find, "p:x")
show("el_findall_colon", fe.findall, "p:x")
show("el_findtext_colon", fe.findtext, "p:x", "dflt")
show("el_iterfind_colon", lambda: list(fe.iterfind("p:x")))
show("el_find_colon_ns", lambda: fe.find("p:x", {"p": "urn:p"}).tag)
show("el_find_colon_empty_ns", fe.find, "p:x", {})
show("el_find_empty", fe.find, "")
show("el_findall_empty", fe.findall, "")
show("el_findtext_empty", fe.findtext, "", "D")
show("el_findtext_b", fe.findtext, "b")
show("el_findtext_c", lambda: repr(fe.findtext("c")))
show("el_find_brace_dot", lambda: fe.find("{a.b}c"))
show("el_find_wild", lambda: fe.find("{*}x").tag)
show("el_find_nons", lambda: fe.find("{}b").tag)
show("el_find_bytes", fe.find, b"b")
show("el_find_none", fe.find, None)
show("el_find_slash", fe.find, "/b")
show("el_findall_dot", lambda: tags(fe.findall("b.c")))
tr = ET.ElementTree(fe)
with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter("always")
    print("tree_find:", tr.find("/b").text, tr.findtext("/b"), tags(tr.findall("/b")),
          tags(tr.iterfind("/b")), [str(x.message)[:40] for x in w])
print("tree_find_plain:", tr.find("b").text, tags(tr.findall(".//*")))

# ── 13. Generator protocol details and re-import ─────────────────────────────
holder = {}


class Reentrant(ET.Element):
    def __iter__(self):
        try:
            next(holder["it"])
        except Exception as exc:  # noqa: BLE001
            print("reentrant:", type(exc).__name__, exc)
        return iter([])


rroot = ET.Element("r")
rroot.append(Reentrant("x"))
holder["it"] = EP.iterfind(rroot, "x/y")
print("reentrant_result:", list(holder["it"]))


class StopGet(ET.Element):
    def get(self, key, default=None):
        raise StopIteration


sroot = ET.Element("r")
sroot.append(StopGet("x"))
try:
    EP.findall(sroot, "x[@a]")
except RuntimeError as exc:
    print("pep479:", exc, type(exc.__cause__).__name__)


class StopText(ET.Element):
    @property
    def text(self):
        raise StopIteration

    @text.setter
    def text(self, value):
        pass


troot = ET.Element("r")
troot.append(StopText("x"))
print("findtext_stop:", EP.findtext(troot, "x", "DEFAULT"))

try:
    EP.findall(rroot, "@a")
except KeyError as exc:
    print("keyerror_args:", exc.args)
try:
    EP.findall(rroot, "/x")
except SyntaxError as exc:
    print("syntax_attrs:", exc.args, exc.msg, exc.filename, exc.lineno, exc.offset, exc.text)

old_find = EP.find
del sys.modules["xml.etree.ElementPath"]
import xml.etree.ElementPath as EP2  # noqa: E402

print("reimport:", EP2 is not EP, EP2.find(fe, "b").text, old_find(fe, "b").text,
      EP2.ops[""] is EP2.prepare_child, len(EP2._cache))
sys.modules["xml.etree.ElementPath"] = EP
