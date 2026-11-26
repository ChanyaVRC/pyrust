# csv / _csv surface implemented natively in Rust (issue #3400): the dialect
# type and registry, field_size_limit, QUOTE_STRINGS / QUOTE_NOTNULL, writer
# quoting rules, C-level argument diagnostics, DictReader / DictWriter, and
# Sniffer.  Complements test_csv.py and test_csv_multiline.py.
import _csv
import csv
import decimal
import fractions
import io


def show(label, fn):
    try:
        print(label, repr(fn()))
    except BaseException as e:
        print(label, type(e).__name__, e)


def w(rows, **kw):
    b = io.StringIO()
    wr = csv.writer(b, **kw)
    for r in rows:
        wr.writerow(r)
    return b.getvalue()


def r(lines, **kw):
    return list(csv.reader(lines, **kw))


# --- module surface -----------------------------------------------------------
print("all", csv.__all__)
print("version", csv.__version__, _csv.__version__)
print("quotes", csv.QUOTE_MINIMAL, csv.QUOTE_ALL, csv.QUOTE_NONNUMERIC,
      csv.QUOTE_NONE, csv.QUOTE_STRINGS, csv.QUOTE_NOTNULL)
print("doc", csv.__doc__ == _csv.__doc__, csv.__doc__.splitlines()[0])
print("vars-_csv", list(vars(_csv)))
print("csv-tail", [k for k in vars(csv) if not k.startswith("__") or k in ("__version__", "__all__")])
print("identity", csv.Error is _csv.Error, csv._Dialect is _csv.Dialect,
      csv.reader is _csv.reader, csv.StringIO is io.StringIO, csv.re.__name__, csv.types.__name__)
print("types", _csv.Reader, _csv.Writer, _csv.Dialect, type(csv.reader([])) is _csv.Reader,
      type(csv.writer(io.StringIO())) is _csv.Writer)
print("modules", csv.Error.__module__, csv.Error.__qualname__, csv.Error.__doc__,
      csv.Dialect.__module__, csv.excel.__module__, csv.DictReader.__module__,
      csv.Sniffer.__module__, _csv.Dialect.__module__, _csv.Dialect.__name__)
print("docs", repr(_csv.Dialect.__doc__), repr(_csv.Reader.__doc__), repr(_csv.Writer.__doc__))
print("docs2", repr(csv.Dialect.__doc__), csv.excel.__doc__, csv.excel_tab.__doc__,
      csv.unix_dialect.__doc__, repr(csv.Sniffer.__doc__), csv.DictReader.__doc__)
print("mro", csv.excel_tab.__mro__, issubclass(csv.Error, Exception))
print("cgi", csv.DictReader[int], csv.DictWriter[str])
print("excel-vars", list(vars(csv.excel)), list(vars(csv.excel_tab)), list(vars(csv.unix_dialect)))
print("excel-inst", csv.excel().delimiter, csv.excel()._valid, csv.Dialect._valid, csv.Dialect._name)

# --- registry -------------------------------------------------------------------
print("list", csv.list_dialects())
print("registry", sorted(_csv._dialects), type(_csv._dialects).__name__)
for name in ("excel", "excel-tab", "unix"):
    d = csv.get_dialect(name)
    print("dialect", name, type(d).__name__, d.delimiter, repr(d.quotechar),
          d.escapechar, d.doublequote, d.skipinitialspace, repr(d.lineterminator),
          d.quoting, d.strict)
show("reg-noargs", lambda: csv.register_dialect())
show("reg-int", lambda: csv.register_dialect(5))
show("reg-ret", lambda: csv.register_dialect("x1"))
show("reg-type", lambda: type(csv.get_dialect("x1")).__name__)
show("reg-cls", lambda: csv.register_dialect("x2", csv.excel_tab) or csv.get_dialect("x2").delimiter)
show("reg-bad", lambda: csv.register_dialect("x3", delimiter=1))
show("reg-bad2", lambda: csv.register_dialect("x3", "nope"))
show("reg-int-dialect", lambda: csv.register_dialect("x3", 5))
show("reg-3pos", lambda: csv.register_dialect("x3", "excel", "x"))
show("reg-kwname", lambda: csv.register_dialect(name="x"))
show("reg-dialect-kw", lambda: (csv.register_dialect("kwd", dialect="unix"), csv.get_dialect("kwd").quoting))
show("reg-override", lambda: (csv.register_dialect("ov", "excel", delimiter="|"),
                              csv.get_dialect("ov").delimiter, csv.get_dialect("excel").delimiter))
show("reg-instance", lambda: (csv.register_dialect("inst", csv.excel()), csv.get_dialect("inst").delimiter))
show("reg-str-subclass", lambda: (csv.register_dialect(type("S", (str,), {})("sub")), csv.get_dialect("sub").delimiter))
show("unreg-unknown", lambda: csv.unregister_dialect("x9"))
show("unreg-int", lambda: csv.unregister_dialect(5))
show("unreg-noargs", lambda: csv.unregister_dialect())
show("unreg-2", lambda: csv.unregister_dialect("a", "b"))
show("unreg-kw", lambda: csv.unregister_dialect(name="zz"))
show("get-int", lambda: csv.get_dialect(5))
show("get-list", lambda: csv.get_dialect([]))
show("get-noargs", lambda: csv.get_dialect())
show("get-2", lambda: csv.get_dialect("a", "b"))
show("get-kw", lambda: csv.get_dialect(name="excel").delimiter)
show("get-badkw", lambda: csv.get_dialect(x="excel"))
show("list-args", lambda: csv.list_dialects(1))
show("list-kw", lambda: csv.list_dialects(a=1))
print("list-after", csv.list_dialects())
for name in ("x1", "x2", "x3", "kwd", "ov", "inst", "sub"):
    csv.unregister_dialect(name)
print("list-final", csv.list_dialects(), sorted(_csv._dialects))

# --- _csv.Dialect validation ------------------------------------------------------
show("d-same", lambda: _csv.Dialect(delimiter=",", quotechar=",").quotechar)
show("d-delim-nl", lambda: _csv.Dialect(delimiter="\n").delimiter)
show("d-lt-int", lambda: _csv.Dialect(lineterminator=1))
show("d-lt-none", lambda: _csv.Dialect(lineterminator=None))
show("d-lt-empty", lambda: _csv.Dialect(lineterminator="").lineterminator)
show("d-delim-int", lambda: _csv.Dialect(delimiter=1))
show("d-delim-none", lambda: _csv.Dialect(delimiter=None))
show("d-delim-2", lambda: _csv.Dialect(delimiter="ab"))
show("d-delim-empty", lambda: _csv.Dialect(delimiter=""))
show("d-q-int", lambda: _csv.Dialect(quotechar=1))
show("d-q-2", lambda: _csv.Dialect(quotechar="ab"))
show("d-q-none", lambda: _csv.Dialect(quotechar=None).quoting)
show("d-q-none-min", lambda: _csv.Dialect(quotechar=None, quoting=csv.QUOTE_MINIMAL))
show("d-esc-int", lambda: _csv.Dialect(escapechar=1))
show("d-esc-sp", lambda: _csv.Dialect(escapechar=" ").escapechar)
show("d-quoting-bad", lambda: _csv.Dialect(quoting=7))
show("d-quoting-neg", lambda: _csv.Dialect(quoting=-1))
show("d-quoting-str", lambda: _csv.Dialect(quoting="1"))
show("d-quoting-bool", lambda: _csv.Dialect(quoting=True))
show("d-quoting-float", lambda: _csv.Dialect(quoting=1.0))
show("d-quoting-big", lambda: _csv.Dialect(quoting=2 ** 40))
show("d-quoting-int-sub", lambda: _csv.Dialect(quoting=type("I", (int,), {})(1)))
show("d-bool", lambda: (lambda d: (d.doublequote, d.strict, d.skipinitialspace))(
    _csv.Dialect(doublequote=0, strict=[1], skipinitialspace="")))
show("d-truthy-err", lambda: _csv.Dialect(strict=type("T", (), {"__bool__": lambda self: 1 / 0})()))
show("d-unknown-kw", lambda: _csv.Dialect(foo=1))
show("d-dup", lambda: _csv.Dialect("excel", dialect="excel"))
show("d-pos", lambda: _csv.Dialect("excel", ";").delimiter)
show("d-too-many", lambda: _csv.Dialect(1, 2, 3, 4, 5, 6, 7, 8, 9, 10))
show("d-unknown-name", lambda: _csv.Dialect("nope"))
show("d-from-int", lambda: _csv.Dialect(5).delimiter)
show("d-from-dict", lambda: _csv.Dialect({"delimiter": ";"}).delimiter)
show("d-getattr-err", lambda: _csv.Dialect(type("B", (), {"delimiter": property(lambda self: 1 / 0)})()).delimiter)
show("d-str-subclass", lambda: _csv.Dialect(delimiter=type("S", (str,), {})(";")).delimiter)
show("d-lt-identity", lambda: (lambda lt: _csv.Dialect(lineterminator=lt).lineterminator is lt)("xy"))
d = _csv.Dialect()
print("d-attrs", d.delimiter, d.doublequote, d.escapechar, repr(d.lineterminator),
      d.quotechar, d.quoting, d.skipinitialspace, d.strict)
show("d-reuse", lambda: _csv.Dialect(d) is d)
show("d-reuse-name", lambda: _csv.Dialect("excel") is csv.get_dialect("excel"))
show("d-reuse-kw", lambda: _csv.Dialect(d, delimiter=";") is d)
show("d-set", lambda: setattr(d, "delimiter", ";"))
show("d-set-quoting", lambda: setattr(d, "quoting", 1))
show("d-set-bool", lambda: setattr(d, "strict", True))
show("d-set-other", lambda: setattr(d, "foo", True))
show("d-pickle", lambda: d.__reduce_ex__(2))
show("d-reduce", lambda: d.__reduce__())
show("d-new-int", lambda: _csv.Dialect.__new__(int))
show("d-new-none", lambda: _csv.Dialect.__new__())
show("d-subclass", lambda: (lambda D: (type(D()).__name__, D(delimiter=";").delimiter))(type("D", (_csv.Dialect,), {})))
show("d-dir", lambda: [a for a in dir(d) if not a.startswith("__")])


class MyD(csv.Dialect):
    delimiter = ";"
    quotechar = "'"
    lineterminator = "\n"
    quoting = csv.QUOTE_MINIMAL
    doublequote = True
    skipinitialspace = False
    strict = True


class BadD(csv.Dialect):
    delimiter = ";;"


show("myd", lambda: MyD()._valid)
show("myd-read", lambda: r(["a;'b;c'"], dialect=MyD))
show("myd-inst-read", lambda: r(["a;'b;c'"], dialect=MyD()))
show("myd-reg", lambda: (csv.register_dialect("myd", MyD), csv.get_dialect("myd").strict, csv.unregister_dialect("myd")))
show("badd", lambda: BadD())
show("Dialect-inst", lambda: csv.Dialect())
show("Dialect-validate", lambda: csv.excel()._validate())
show("excel-arg", lambda: csv.excel(1))
show("excel-kw", lambda: csv.excel(x=1))
show("validate-arg", lambda: csv.excel()._validate(1))

# --- reader / writer objects ----------------------------------------------------
show("reader-noargs", lambda: csv.reader())
show("reader-nonit", lambda: csv.reader(5))
show("reader-3pos", lambda: csv.reader([], "excel", "x"))
show("reader-badkw", lambda: csv.reader([], foo=1))
show("reader-dialect-kw", lambda: csv.reader([], dialect="unix").dialect.quoting)
show("reader-dialect-default", lambda: csv.reader([]).dialect is csv.get_dialect("excel"))
show("reader-dialect-class", lambda: type(csv.reader([], csv.excel).dialect).__name__)
show("reader-none-dialect", lambda: csv.reader([], None).dialect.delimiter)
show("writer-noargs", lambda: csv.writer())
show("writer-nowrite", lambda: csv.writer(5))
show("writer-noncallable", lambda: csv.writer(type("W", (), {"write": 5})()))
show("writer-dialect-name", lambda: csv.writer(io.StringIO(), "unix").dialect is csv.get_dialect("unix"))
show("Reader-new", lambda: _csv.Reader())
show("Writer-new", lambda: _csv.Writer())
show("reader-subclass", lambda: type("X", (_csv.Reader,), {})())
show("writer-subclass", lambda: type("X", (_csv.Writer,), {})())
show("reader-attrs", lambda: [a for a in dir(csv.reader([])) if not a.startswith("__")])
show("writer-attrs", lambda: [a for a in dir(csv.writer(io.StringIO())) if not a.startswith("__")])
rr = csv.reader(["a"])
show("reader-iter", lambda: iter(rr) is rr)
show("reader-set-line_num", lambda: setattr(rr, "line_num", 5))
show("reader-set-dialect", lambda: setattr(rr, "dialect", 5))
show("reader-set-other", lambda: setattr(rr, "other", 5))
show("reader-line_num", lambda: (rr.line_num, next(rr), rr.line_num))
ww = csv.writer(io.StringIO())
show("writer-set-dialect", lambda: setattr(ww, "dialect", 5))
show("writer-set-other", lambda: setattr(ww, "other", 5))
show("writerow-ret", lambda: ww.writerow(["abc"]))
show("writerows-ret", lambda: ww.writerows([["abc"]]))
show("writerows-nonit", lambda: ww.writerows(5))
show("writerows-nonit2", lambda: ww.writerows([5]))
show("writerow-noargs", lambda: ww.writerow())
show("writerow-2", lambda: ww.writerow(1, 2))
show("writerow-kw", lambda: ww.writerow(row=[1]))
show("writerows-noargs", lambda: ww.writerows())
show("writerow-iterable", lambda: w([1]))
show("writerow-str", lambda: w(["abc"]))
show("writerow-gen", lambda: w([(x for x in "abc")]))
show("writerow-iter-err", lambda: w([iter(lambda: 1 / 0, None)]))
show("writer-write-err", lambda: csv.writer(type("W", (), {"write": lambda self, s: 1 / 0})()).writerow(["a"]))
show("writer-write-ret", lambda: csv.writer(type("W", (), {"write": lambda self, s: s.upper()})()).writerow(["a", "b"]))
show("writerows-gen", lambda: (lambda b: (csv.writer(b).writerows([i, i * 2] for i in range(3)), b.getvalue()))(io.StringIO()))
show("reader-iter-err", lambda: r(iter(lambda: 1 / 0, None)))
show("reader-dict-rows", lambda: r([{"a": 1}]))
show("reader-str-subclass", lambda: r([type("S", (str,), {})("a,b")]))

# --- writer quoting rules ---------------------------------------------------------


class Num:
    def __index__(self):
        return 3

    def __str__(self):
        return "3"


class Flt:
    def __float__(self):
        return 1.5

    def __str__(self):
        return "1.5"


show("qs-write", lambda: w([["a", None, "", 3, 2.5]], quoting=csv.QUOTE_STRINGS))
show("qn-write", lambda: w([["a", None, "", 3, 2.5]], quoting=csv.QUOTE_NOTNULL))
show("qn-single-none", lambda: w([[None]], quoting=csv.QUOTE_NOTNULL))
show("qs-single-none", lambda: w([[None]], quoting=csv.QUOTE_STRINGS))
show("qs-single-empty", lambda: w([[""]], quoting=csv.QUOTE_STRINGS))
show("qs-read", lambda: r(['a,,"",3\n'], quoting=csv.QUOTE_STRINGS))
show("qn-read", lambda: r(['a,,"",3\n'], quoting=csv.QUOTE_NOTNULL))
show("nn-numbers", lambda: w([[True, False, 1, 1.5, 1j, None, "x", Num(), Flt(),
                               decimal.Decimal("1.1"), fractions.Fraction(1, 3), b"y"]],
                             quoting=csv.QUOTE_NONNUMERIC))
show("qs-custom", lambda: w([[Num(), "x", None, "", 1]], quoting=csv.QUOTE_STRINGS))
show("lt-char", lambda: w([["aXb", "c"]], lineterminator="X"))
show("lt-crlf-in-field", lambda: w([["a\rb", "c\nd"]], lineterminator="X"))
show("lt-esc", lambda: w([["a|b", "c\\d"]], quoting=csv.QUOTE_NONE, escapechar="\\", lineterminator="|"))
show("space-skip", lambda: w([["", "a", None]], delimiter=" ", skipinitialspace=True))
show("space-skip-none", lambda: w([["", "a"]], delimiter=" ", skipinitialspace=True, quoting=csv.QUOTE_NONE))
show("space-skip-notnull", lambda: w([[None, "a"]], delimiter=" ", skipinitialspace=True, quoting=csv.QUOTE_NOTNULL))
show("space-skip-read", lambda: r(['a  b "c"  d\n'], delimiter=" ", skipinitialspace=True))
show("space-read", lambda: r(['a  b "c"  d\n'], delimiter=" "))
show("quote-none-specials", lambda: w([['a"b']], quoting=csv.QUOTE_NONE, escapechar="\\"))
show("quote-none-noesc", lambda: w([['a"b']], quoting=csv.QUOTE_NONE))
show("quote-none-noquotechar", lambda: w([['a"b']], quoting=csv.QUOTE_NONE, quotechar=None))
show("quotechar-none", lambda: w([["a,b"]], quotechar=None))
show("quotechar-none-esc", lambda: w([["a,b"]], quotechar=None, escapechar="\\"))
show("custom-str", lambda: w([[type("O", (), {"__str__": lambda self: "o,o"})()]]))
show("str-subclass-raw", lambda: w([[type("S", (str,), {"__str__": lambda self: "X"})("a,b")]]))

# --- field_size_limit ---------------------------------------------------------------
show("fsl", lambda: csv.field_size_limit())
show("fsl-set", lambda: csv.field_size_limit(10))
show("fsl-get", lambda: csv.field_size_limit())
show("fsl-read", lambda: r(["a" * 11]))
show("fsl-read-ok", lambda: r(["a" * 10]))
show("fsl-read-q", lambda: r(['"' + "a" * 11 + '"']))
show("fsl-str", lambda: csv.field_size_limit("5"))
show("fsl-float", lambda: csv.field_size_limit(5.0))
show("fsl-bool", lambda: csv.field_size_limit(True))
show("fsl-kw", lambda: csv.field_size_limit(new_limit=5))
show("fsl-badkw", lambda: csv.field_size_limit(limit=5))
show("fsl-2", lambda: csv.field_size_limit(1, 2))
show("fsl-dup", lambda: csv.field_size_limit(5, new_limit=6))
show("fsl-big", lambda: csv.field_size_limit(2 ** 70))
show("fsl-neg", lambda: csv.field_size_limit(-5))
show("fsl-neg-read", lambda: r([""]))
show("fsl-neg-read2", lambda: r(["a"]))
show("fsl-zero", lambda: (csv.field_size_limit(0), r([",,"]), r(['""'])))
show("fsl-reset", lambda: csv.field_size_limit(131072))
show("fsl-final", lambda: csv.field_size_limit())

# --- reader error recovery ------------------------------------------------------------
rd = csv.reader(['"a', "b\n", 'c"x\n', "d\n"], strict=True)
show("strict-mid", lambda: next(rd))
show("strict-after", lambda: (next(rd), rd.line_num))
rd = csv.reader(["a\rb\n", "c\n"])
show("crnl-err", lambda: next(rd))
show("crnl-after", lambda: (next(rd), rd.line_num))
rd = csv.reader(["1,x,2\n", "3,4\n"], quoting=csv.QUOTE_NONNUMERIC)
show("nonnum-err", lambda: next(rd))
show("nonnum-after", lambda: (next(rd), rd.line_num))
show("nonnum-values", lambda: r(['inf,-1e3, 2 ,nan,"q"\n'], quoting=csv.QUOTE_NONNUMERIC))
show("nonnum-msg", lambda: r(["a'b,\t;\n"], quoting=csv.QUOTE_NONNUMERIC))
show("esc-crnl-continue", lambda: r(["a\\\n", "bc", "d\n"], escapechar="\\"))
show("esc-crnl-continue-eof", lambda: r(["a\\\n", "bc"], escapechar="\\"))

# --- state-machine matrix -----------------------------------------------------------------
INPUTS = [
    ['a,"b,c",d\n'],
    ['"a""b",c\n'],
    ['"a"b,c\n'],
    ['a\\,b,c\n'],
    ['"a\\"b",c\n'],
    [' "a", b\n'],
    ['a;b;"c;d"\n'],
    ['"x\n', 'y",z\n'],
    ["a,\r\n", "\r\n", "b\r\n"],
    ['"unterminated\n'],
    ["'q'',r',s\n"],
    ["a\tb\t'c\td'\n"],
]
DIALECTS = [
    {},
    {"strict": True},
    {"doublequote": False},
    {"doublequote": False, "escapechar": "\\"},
    {"escapechar": "\\"},
    {"skipinitialspace": True},
    {"quoting": csv.QUOTE_NONE, "escapechar": "\\"},
    {"quoting": csv.QUOTE_ALL},
    {"delimiter": ";"},
    {"quotechar": "'"},
    {"delimiter": "\t", "quotechar": "'"},
]
for i, lines in enumerate(INPUTS):
    for j, kw in enumerate(DIALECTS):
        rd = csv.reader(lines, **kw)
        out = []
        for _ in range(6):
            try:
                out.append((next(rd), rd.line_num))
            except StopIteration:
                break
            except csv.Error as e:
                out.append(("ERR", str(e), rd.line_num))
        print("matrix", i, j, out)

DATA = [["a", "b,c", 'd"e'], ["", None, "x\r\ny"], [1, 2.5, True], ["multi\n\nline", " lead", "trail "]]
for j, kw in enumerate(DIALECTS + [{"quoting": csv.QUOTE_NONNUMERIC},
                                   {"quoting": csv.QUOTE_STRINGS},
                                   {"quoting": csv.QUOTE_NOTNULL},
                                   {"lineterminator": "\n"}]):
    try:
        text = w(DATA, **kw)
    except csv.Error as e:
        print("write", j, "ERR", e)
        continue
    show("write %d %r" % (j, text), lambda: r(io.StringIO(text, newline=""), **kw))

# --- DictReader / DictWriter -----------------------------------------------------------
show("DR-noargs", lambda: csv.DictReader())
show("DW-noargs", lambda: csv.DictWriter())
show("DW-nofields", lambda: csv.DictWriter(io.StringIO()))
show("DR-kwf", lambda: list(csv.DictReader(f=["a", "1"])))
show("DR-dup", lambda: csv.DictReader(["a"], f=["b"]))
show("DW-dup", lambda: csv.DictWriter(io.StringIO(), ["a"], fieldnames=["b"]))
show("DR-next-arg", lambda: csv.DictReader(["a"]).__next__(1))
show("DW-writerow-noargs", lambda: csv.DictWriter(io.StringIO(), ["a"]).writerow())
show("DW-writerows-2", lambda: csv.DictWriter(io.StringIO(), ["a"]).writerows(1, 2))
show("DW-extras-case", lambda: csv.DictWriter(io.StringIO(), ["a"], extrasaction="RAISE").extrasaction)
show("DW-extras-bad", lambda: csv.DictWriter(io.StringIO(), ["a"], extrasaction="Nope"))
show("DW-extras-int", lambda: csv.DictWriter(io.StringIO(), ["a"], extrasaction=1))
show("DW-gen-fields", lambda: csv.DictWriter(io.StringIO(), (x for x in "ab")).fieldnames)
show("DR-gen-fields", lambda: csv.DictReader([], (x for x in "ab")).fieldnames)
show("DR-tuple-fields", lambda: csv.DictReader([], ("a", "b")).fieldnames)
show("DR-empty", lambda: (lambda d: (d.fieldnames, d.line_num, list(d)))(csv.DictReader([])))
show("DR-attrs", lambda: sorted(vars(csv.DictReader(["a"], restkey="k", restval="v", delimiter=";"))))
show("DR-dialect", lambda: csv.DictReader(["a"], dialect="unix").dialect)
show("DW-attrs", lambda: sorted(vars(csv.DictWriter(io.StringIO(), ["a"]))))
show("DW-writeheader-ret", lambda: csv.DictWriter(io.StringIO(), ["a", "b"]).writeheader())
show("DW-writerows-ret", lambda: csv.DictWriter(io.StringIO(), ["a", "b"]).writerows([{"a": 1}]))
show("DW-dict_to_list", lambda: list(csv.DictWriter(io.StringIO(), ["a", "b"], restval="R")._dict_to_list({"a": 1})))
show("DW-nondict", lambda: csv.DictWriter(io.StringIO(), ["a"]).writerow([1]))
show("DW-ignore", lambda: (lambda b: (csv.DictWriter(b, ["a"], extrasaction="ignore").writerow({"a": 1, "b": 2}), b.getvalue()))(io.StringIO()))
show("DW-kwds", lambda: csv.DictWriter(io.StringIO(), ["a"], bogus=1))
show("DR-extra-pos", lambda: list(csv.DictReader(["a;b", "1;2"], None, None, None, "excel", "x")))
show("DR-fmt", lambda: list(csv.DictReader(["a;b", "1;2"], delimiter=";")))
show("DR-kwds", lambda: list(csv.DictReader(["a;b", "1;2"], delimiter=";", bogus=1)))
show("DR-restkey-None", lambda: list(csv.DictReader(["a", "1,2,3"])))
show("DR-blank-only", lambda: list(csv.DictReader(["a", "", ""])))
show("DR-set-fieldnames", lambda: (lambda d: (setattr(d, "fieldnames", ["x", "y"]), list(d)))(csv.DictReader(["1,2"])))


class UpperDictReader(csv.DictReader):
    @property
    def fieldnames(self):
        names = super().fieldnames
        return [n.upper() for n in names] if names else names


show("DR-subclass", lambda: list(UpperDictReader(["a,b", "1,2", "3,4"])))


class TupleDictWriter(csv.DictWriter):
    def _dict_to_list(self, rowdict):
        return [str(rowdict.get(k, "?")) + "!" for k in self.fieldnames]


show("DW-subclass", lambda: (lambda b: (TupleDictWriter(b, ["a", "b"]).writerows([{"a": 1}, {"b": 2}]), b.getvalue()))(io.StringIO()))

# --- Sniffer ------------------------------------------------------------------------------
s = csv.Sniffer()
print("preferred", s.preferred)
SAMPLES = [
    "a,b,c\n1,2,3\n4,5,6\n",
    "a;b;c\n1;2;3\n4;5;6\n",
    "a\tb\tc\n1\t2\t3\n",
    '"a","b","c"\n"1","2","3"\n',
    "'a';'b';'c'\n'1';'2';'3'\n",
    "name, age\nAlice, 30\nBob, 25\n",
    '"a, b",c\n"d",e\n',
    "a|b|c\n1|2|3\n",
    "abc\ndef\n",
    "",
    "a b c\n1 2 3\n",
    '"he said ""hi""",x\n"y",z\n',
    "x:y:z\n1:2:3\n4:5:6\n7:8:9\n",
    "a,b;c\n1,2;3\n",
    '"a"\n"b"\n',
    "id,name\n" + "".join("%d,n%d\n" % (i, i) for i in range(30)),
    "a,b,c\n1,2\n3,4,5,6\n7,8,9\n",
    "1,'x',2\n3,'y',4\n",
    "a1b2c\n3a4b5\n",
    "\n".join('"%d","row %d","x""y"' % (i, i) for i in range(60)),
]
for i, smp in enumerate(SAMPLES):
    def go(smp=smp):
        d = s.sniff(smp)
        return (d.delimiter, d.quotechar, d.doublequote, d.skipinitialspace, d.lineterminator,
                d.quoting, d._name, d.escapechar, d.__name__, d.__qualname__, d.__module__,
                d.__mro__[1].__name__)
    show("sniff%d" % i, go)
    show("sniff-delims%d" % i, lambda smp=smp: s.sniff(smp, delimiters=";,").delimiter)
    show("hdr%d" % i, lambda smp=smp: s.has_header(smp))
show("sniffed-inst", lambda: (lambda d: (d()._valid, d().delimiter))(s.sniff("a,b\n1,2\n")))
show("sniff-bytes", lambda: s.sniff(b"a,b\n1,2\n"))
show("sniff-int", lambda: s.sniff(5))
show("sniff-noargs", lambda: s.sniff())
show("sniff-many", lambda: s.sniff(1, 2, 3))
show("sniff-dup", lambda: s.sniff("a", sample="b"))
show("sniff-kw", lambda: s.sniff(sample="a,b\n1,2\n").delimiter)
show("unbound-sniff", lambda: csv.Sniffer.sniff())
show("sniffer-init-arg", lambda: csv.Sniffer(1))
show("hh-noargs", lambda: s.has_header())
show("gq", lambda: s._guess_quote_and_delimiter('"a","b"\n"c","d"\n', None))
show("gq-one", lambda: s._guess_quote_and_delimiter("x"))
show("gq-single-col", lambda: s._guess_quote_and_delimiter('"a"\n"b"\n', None))
show("gq-space", lambda: s._guess_quote_and_delimiter("x, 'a', 'b'\n", None))
show("gq-dq", lambda: s._guess_quote_and_delimiter('a,"x""y",b\n', None))
show("gd", lambda: s._guess_delimiter("a,b\n1,2\n", None))
show("gd-noargs", lambda: s._guess_delimiter())
show("gd-int", lambda: s._guess_delimiter(5, None))
show("hdr-cases", lambda: [s.has_header(x) for x in ["name,age\nann,30\nbob,40\n", "a,b\n1,2\n3,4\n",
                                                     "1,2\n3,4\n5,6\n", "xx,yy\naa,bb\ncc,dd\n",
                                                     "h1,h2\n1.5,abc\n2e3,de\n"]])
show("hdr-empty", lambda: s.has_header("a,b\n"))
show("hdr-one", lambda: s.has_header("a\n"))
show("hdr-bad", lambda: s.has_header("abc\n"))


class PickySniffer(csv.Sniffer):
    def __init__(self):
        super().__init__()
        self.preferred = [";"]


show("sniffer-preferred", lambda: PickySniffer().sniff("a,b;c\n1,2;3\n").delimiter)
long_sample = "\n".join('"%d","name %d",%d' % (i, i, i * 3) for i in range(200))
show("sniff-long", lambda: (lambda d: (d.delimiter, d.quotechar, d.doublequote))(s.sniff(long_sample[:1024])))
show("hdr-long", lambda: s.has_header(long_sample[:1024]))

print("csv native ok")
