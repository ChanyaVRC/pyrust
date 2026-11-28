"""Parity fixture for the native `textwrap` (issue #3488): every public
behaviour of CPython 3.12.3's Lib/textwrap.py."""
import textwrap
from textwrap import TextWrapper


def show(label, thunk):
    try:
        print(label, repr(thunk()))
    except BaseException as exc:
        print(label, "!", type(exc).__name__, exc)


# -- module metadata ---------------------------------------------------------
print(textwrap.__all__)
print(repr(textwrap.__doc__))
print(sorted(n for n in dir(textwrap) if not n.startswith("__")))
print(textwrap.wrap.__name__, textwrap.TextWrapper.__name__)
print(textwrap.TextWrapper.__module__)
print(textwrap.TextWrapper.wrap.__doc__ == TextWrapper.wrap.__doc__)
print(repr(textwrap.wrap.__doc__.splitlines()[0]))
print(repr(textwrap.TextWrapper._split.__doc__.splitlines()[0]))
print(repr(textwrap.dedent.__doc__.splitlines()[0]))
print(repr(textwrap.indent.__doc__.splitlines()[0]))
print(repr(textwrap.shorten.__doc__.splitlines()[0]))
print(textwrap.TextWrapper.__doc__.splitlines()[1].strip())
print(textwrap._whitespace == "\t\n\x0b\x0c\r ")
print(sorted(textwrap.TextWrapper.unicode_whitespace_trans.items()))
print(textwrap.TextWrapper.wordsep_simple_re.pattern == r'([\%s]+)' % "\\".join("\t\n\x0b\x0c\r "))
print(textwrap.TextWrapper.sentence_end_re.pattern)
print(isinstance(textwrap.TextWrapper.wordsep_re.pattern, str))
print(textwrap.re.__name__)

# -- TextWrapper defaults and attributes ---------------------------------------
w = TextWrapper()
print(vars(w))
w = TextWrapper(30, "> ", "  ", False, False, True, False, False, False, 4,
                max_lines=3, placeholder="...")
print(vars(w))
show("bad-kw", lambda: TextWrapper(foo=1))
show("too-many", lambda: TextWrapper(*range(12)))
show("kwonly-positional", lambda: TextWrapper(1, "", "", True, True, False, True, True, True, 8, 2))
show("wrap-arity", lambda: textwrap.wrap())
show("wrap-bad-kw", lambda: textwrap.wrap("x", bogus=1))
show("fill-arity", lambda: textwrap.fill("a", 5, 6))
show("shorten-arity", lambda: textwrap.shorten("a"))
show("dedent-arity", lambda: textwrap.dedent())
show("indent-arity", lambda: textwrap.indent("a"))
show("method-arity", lambda: TextWrapper().wrap())

# -- wrap / fill --------------------------------------------------------------
TEXT = "The quick brown fox jumped over the lazy dog. It was a sunny day, and nobody minded."
for width in (1, 5, 10, 20, 33, 70, 200):
    show("wrap w=%d" % width, lambda: textwrap.wrap(TEXT, width))
print(textwrap.fill(TEXT, 25))
print(textwrap.wrap(""), textwrap.wrap("   \n\t "), textwrap.fill(""))
print(textwrap.wrap("single"), textwrap.fill("single", width=3))
show("width=0", lambda: textwrap.wrap("abc", width=0))
show("width=-3", lambda: textwrap.fill("abc", width=-3))
show("width-str", lambda: textwrap.wrap("abc", width="x"))
show("width-float", lambda: textwrap.wrap("abc def", width=5.5))
show("width-None", lambda: textwrap.wrap("abc", width=None))
show("text-int", lambda: textwrap.wrap(5))
show("text-None", lambda: textwrap.fill(None))
show("text-bytes", lambda: textwrap.wrap(b"abc"))

# initial_indent / subsequent_indent
show("indents", lambda: textwrap.wrap(TEXT, 30, initial_indent="* ", subsequent_indent="    "))
show("indent-fill", lambda: textwrap.fill(TEXT, 30, initial_indent=">>> ", subsequent_indent="... "))
show("indent-only", lambda: textwrap.wrap("a b c", 8, initial_indent="    ", subsequent_indent="  "))
show("indent-too-wide", lambda: textwrap.wrap("abcdef ghijkl", 4, subsequent_indent="  "))

# expand_tabs / tabsize / replace_whitespace
TABS = "a\tb\n\tc\r\nd\x0be\x0cf  g"
show("tabs default", lambda: textwrap.wrap(TABS, 40))
show("tabs no-expand", lambda: textwrap.wrap(TABS, 40, expand_tabs=False))
show("tabs tabsize=2", lambda: textwrap.wrap(TABS, 40, tabsize=2))
show("tabs tabsize=0", lambda: textwrap.wrap(TABS, 40, tabsize=0))
show("tabs no-replace", lambda: textwrap.wrap(TABS, 40, replace_whitespace=False))
show("tabs no-expand/no-replace", lambda: textwrap.wrap(TABS, 40, expand_tabs=False, replace_whitespace=False))
show("tabs both off narrow", lambda: textwrap.wrap(TABS, 4, expand_tabs=False, replace_whitespace=False))

# fix_sentence_endings
SENT = "Hello there.  This is one. And two!  Three?\nFour.\" Five.' Six. seven. Z. a.b"
show("sentence off", lambda: textwrap.wrap(SENT, 100))
show("sentence on", lambda: textwrap.wrap(SENT, 100, fix_sentence_endings=True))
show("sentence on narrow", lambda: textwrap.wrap(SENT, 20, fix_sentence_endings=True))
show("sentence on no-replace", lambda: textwrap.wrap("a.\nb. c.", 40, fix_sentence_endings=True, replace_whitespace=False))

# break_long_words / drop_whitespace / break_on_hyphens
LONG = "a verylongwordthatexceedsthewidth b supercalifragilisticexpialidocious"
show("long break", lambda: textwrap.wrap(LONG, 10))
show("long nobreak", lambda: textwrap.wrap(LONG, 10, break_long_words=False))
show("long nobreak w1", lambda: textwrap.wrap("ab cd", 1, break_long_words=False))
show("long break w1", lambda: textwrap.wrap("ab cd", 1))
show("long hyphen", lambda: textwrap.wrap("aaaaa-bbbbb-ccccc-ddddd", 8))
show("long hyphen off", lambda: textwrap.wrap("aaaaa-bbbbb-ccccc-ddddd", 8, break_on_hyphens=False))
show("long hyphen nobreak", lambda: textwrap.wrap("aaaaa-bbbbb-ccccc-ddddd", 8, break_long_words=False))
show("long leading hyphens", lambda: textwrap.wrap("-----aaaaaaaaaaa", 6))
show("long digits-hyphen", lambda: textwrap.wrap("1234-5678-9012-3456", 6))
show("drop on", lambda: textwrap.wrap("  a  b   c  ", 3))
show("drop off", lambda: textwrap.wrap("  a  b   c  ", 3, drop_whitespace=False))
show("drop off wide", lambda: textwrap.wrap("  a  b   c  ", 50, drop_whitespace=False))
show("drop off lines", lambda: textwrap.wrap("one two three four five six", 9, drop_whitespace=False))
show("drop off indent", lambda: textwrap.wrap("one two three", 9, drop_whitespace=False, subsequent_indent="--"))

# hyphenated words
for text in ("foo-bar-baz", "em--dash", "em---dash", "-leading", "trailing-", "a-b", "a-b-c-d",
             "well-known self-aware non-existent", "x - y -- z --- w", "--a --b a-- a--b",
             "e-mail 1-2-3 co-op 3d-printing", "\u00e9t\u00e9-\u00e0-\u00e9t\u00e9 \u65e5\u672c-\u8a9e-\u6587",
             "snake_case-word dash-_x", "a.-b ,--c !--d", "mid--word--again", "x-\u00b2y-z", "ab-cd-"):
    chunks = TextWrapper()._split(text)
    show("split %r" % text, lambda: chunks)
    show("wrap %r" % text, lambda: textwrap.wrap(text, 5))
    show("nohyph %r" % text, lambda: textwrap.wrap(text, 5, break_on_hyphens=False))
show("goof-ball", lambda: TextWrapper()._split("Look, goof-ball -- use the -b option!"))
show("goof-ball off", lambda: TextWrapper(break_on_hyphens=False)._split("Look, goof-ball -- use the -b option!"))
show("break_on_hyphens=1", lambda: TextWrapper(break_on_hyphens=1)._split("a-b-c d-e"))
show("break_on_hyphens=None", lambda: TextWrapper(break_on_hyphens=None)._split("a-b-c d-e"))
show("simple split", lambda: TextWrapper(break_on_hyphens=False)._split("  a\t b \n"))
show("split empty", lambda: TextWrapper()._split(""))
show("split_chunks", lambda: TextWrapper()._split_chunks("a\tb-c\nd"))
show("munge", lambda: TextWrapper()._munge_whitespace(" foo\tbar\n\nbaz"))
show("munge tabsize", lambda: TextWrapper(tabsize=3)._munge_whitespace("a\tb"))

# max_lines / placeholder
for ml in (None, 0, 1, 2, 3, 10):
    show("max_lines=%s" % ml, lambda: textwrap.wrap(TEXT, 20, max_lines=ml))
show("max_lines custom ph", lambda: textwrap.wrap(TEXT, 20, max_lines=2, placeholder=" (more)"))
show("max_lines empty ph", lambda: textwrap.wrap(TEXT, 20, max_lines=2, placeholder=""))
show("max_lines ph nospace", lambda: textwrap.wrap(TEXT, 20, max_lines=2, placeholder="..."))
show("max_lines indent", lambda: textwrap.wrap(TEXT, 24, max_lines=3, initial_indent="> ", subsequent_indent="  "))
show("max_lines ph too large", lambda: textwrap.wrap(TEXT, 5, max_lines=2))
show("max_lines ph too large indent", lambda: textwrap.wrap(TEXT, 8, max_lines=1, initial_indent="    "))
show("max_lines ph too large sub", lambda: textwrap.wrap(TEXT, 8, max_lines=2, subsequent_indent="    "))
show("max_lines=1 exact", lambda: textwrap.wrap("abcde", 5, max_lines=1))
show("max_lines trailing ws", lambda: textwrap.wrap("abc def   ", 7, max_lines=1))
show("max_lines trailing ws off", lambda: textwrap.wrap("abc def   ", 7, max_lines=1, drop_whitespace=False))
show("max_lines long word", lambda: textwrap.wrap("aaaaaaaaaaaaaaaaaaaa b", 8, max_lines=2, placeholder="~"))
show("max_lines bad", lambda: textwrap.wrap(TEXT, 20, max_lines="2"))
show("max_lines float", lambda: textwrap.wrap(TEXT, 20, max_lines=2.5))
show("placeholder bad", lambda: textwrap.wrap(TEXT, 20, max_lines=2, placeholder=5))
show("max_lines=-1", lambda: textwrap.wrap(TEXT, 20, max_lines=-1))

# -- shorten --------------------------------------------------------------------
show("shorten fits", lambda: textwrap.shorten("Hello  world!", width=12))
show("shorten cut", lambda: textwrap.shorten("Hello  world!", width=11))
show("shorten tiny", lambda: textwrap.shorten("Hello  world!", width=6))
show("shorten ph-size", lambda: textwrap.shorten("Hello  world!", width=5))
show("shorten too small", lambda: textwrap.shorten("Hello  world!", width=4))
show("shorten ws", lambda: textwrap.shorten("  \tHello \n\n world \t again  ", width=40))
show("shorten collapse", lambda: textwrap.shorten("a\tb\nc   d", width=20))
show("shorten ph", lambda: textwrap.shorten("Hello world, how are you", width=15, placeholder="..."))
show("shorten ph=''", lambda: textwrap.shorten("Hello world, how are you", width=15, placeholder=""))
show("shorten ph big", lambda: textwrap.shorten("Hello world", width=5, placeholder="xxxxxxxxxx"))
show("shorten empty", lambda: textwrap.shorten("", width=5))
show("shorten ws-only", lambda: textwrap.shorten(" \t\n ", width=5))
show("shorten max_lines", lambda: textwrap.shorten("a b c", width=10, max_lines=2))
show("shorten extra", lambda: textwrap.shorten("a b c d e f g", width=9, break_long_words=False))
show("shorten indent", lambda: textwrap.shorten("alpha beta gamma", width=14, initial_indent="- "))
show("shorten bad kw", lambda: textwrap.shorten("a b", width=5, bogus=1))
show("shorten width zero", lambda: textwrap.shorten("a b", width=0))
show("shorten text int", lambda: textwrap.shorten(5, width=5))
show("shorten text None", lambda: textwrap.shorten(None, width=5))

# -- dedent ---------------------------------------------------------------------
DEDENTS = [
    "", "x", "  x", "\tx", "  x\n", "  x\n  y", "  x\n  y\n", "\n", "\n\n", "  \n", "  \n  x",
    "    a\n  b\n    c\n", "    a\n\n    b\n", "    a\n    \n    b\n", "    a\n \t \n    b\n",
    "  \ta\n  \t  b\n", "  a\n\tb\n", "\ta\n  b\n", "\t a\n \tb\n", " \ta\n \tb\n",
    "  a\n    b\n  c", "\t\ta\n\t\t\tb\n\t\tc", "  a\r\n  b\r\n", "  a\r\n   \r\n  b", "  a\r\n\r\n  b",
    "   \t  \n", "a\n  b", "  a\nb", "\n  a\n  b\n\n", "  a\n  \n\n  b\n  ", "\r  a\n  b",
    " \x0ba\n \x0bb", "\xa0a\n\xa0b", "  \u3000a\n  \u3000b", "   a\n  b\n a\n",
]
for text in DEDENTS:
    show("dedent %r" % text, lambda: textwrap.dedent(text))
show("dedent int", lambda: textwrap.dedent(5))
show("dedent None", lambda: textwrap.dedent(None))
show("dedent bytes", lambda: textwrap.dedent(b"  a"))
show("dedent bytearray", lambda: textwrap.dedent(bytearray(b"  a")))
show("dedent list", lambda: textwrap.dedent(["  a"]))
show("dedent docstring", lambda: textwrap.dedent('''\
    Hello there.
      This is indented.

    Done.
    '''))
class MyStr(str):
    pass
show("dedent subclass", lambda: textwrap.dedent(MyStr("  a\n  b")))
show("dedent subclass type", lambda: type(textwrap.dedent(MyStr("  a\n  b"))).__name__)

# -- indent ---------------------------------------------------------------------
INDENTS = ["", "a", "a\n", "a\nb", "a\n\nb\n", "a\n  \nb", "a\n \t \nb\n", "\n", "  ", "a\r\nb\r\n", "a\rb",
           "a\r\n\r\nb", "a\x0bb\x0cc", "a\u2028b\u2029c\x85d", "\n\n\n", "a\n\n\n", "  a\n b\n"]
for text in INDENTS:
    show("indent %r" % text, lambda: textwrap.indent(text, "> "))
    show("indent-all %r" % text, lambda: textwrap.indent(text, "> ", predicate=lambda line: True))
    show("indent-none %r" % text, lambda: textwrap.indent(text, "> ", lambda line: False))
    show("indent-long %r" % text, lambda: textwrap.indent(text, "+", lambda line: len(line) > 2))
    show("indent-pre='' %r" % text, lambda: textwrap.indent(text, ""))
show("indent int", lambda: textwrap.indent(5, "x"))
show("indent bytes", lambda: textwrap.indent(b"a\nb", "x"))
show("indent prefix int", lambda: textwrap.indent("a\n", 5))
show("indent prefix int nopred", lambda: textwrap.indent("a\n", 5, lambda l: False))
show("indent predicate int", lambda: textwrap.indent("a", "x", 5))
show("indent predicate raises", lambda: textwrap.indent("a\nb", "x", lambda l: 1 / 0))
seen = []
show("indent predicate sees", lambda: textwrap.indent("a\n\nb  \n  \n", "x", lambda l: seen.append(l) or True))
print(seen)
show("indent predicate result", lambda: textwrap.indent("a\nb\nc\n", "x", lambda l: l if l != "b\n" else ""))
show("indent subclass", lambda: textwrap.indent(MyStr("a\nb"), "x"))
show("indent prefix subclass", lambda: textwrap.indent("a\nb", MyStr("x")))
show("indent unicode ws", lambda: textwrap.indent("\xa0\n\u3000\n a", "+"))

# -- subclassing and dynamic mutation ----------------------------------------------
class Tracing(TextWrapper):
    calls = []

    def _munge_whitespace(self, text):
        self.calls.append("munge")
        return super()._munge_whitespace(text)

    def _split(self, text):
        self.calls.append("split")
        return super()._split(text)

    def _fix_sentence_endings(self, chunks):
        self.calls.append("fix")
        return super()._fix_sentence_endings(chunks)

    def _handle_long_word(self, reversed_chunks, cur_line, cur_len, width):
        self.calls.append(("long", cur_len, width))
        return super()._handle_long_word(reversed_chunks, cur_line, cur_len, width)

    def _wrap_chunks(self, chunks):
        self.calls.append(("wrap", len(chunks)))
        return super()._wrap_chunks(chunks)

    def _split_chunks(self, text):
        self.calls.append("split_chunks")
        return super()._split_chunks(text)


t = Tracing(width=10, fix_sentence_endings=True)
print(t.wrap("Hi there. A verylongwordhere ok"))
print(Tracing.calls)
Tracing.calls.clear()
print(t.fill("x y"), Tracing.calls)


class Upper(TextWrapper):
    def _split(self, text):
        return [c.upper() for c in super()._split(text)]

    def wrap(self, text):
        return ["<%s>" % line for line in super().wrap(text)]


print(Upper(width=8).wrap("alpha beta gamma"))
print(Upper(width=8).fill("alpha beta gamma"))


class NoHyphenSplit(TextWrapper):
    def _split(self, text):
        return text.replace("-", " ").split(" ")


print(NoHyphenSplit(width=5).wrap("a-b c"))


class Chars(TextWrapper):
    def _split(self, text):
        return list(text)


print(Chars(width=3).wrap("abcdefgh"))
print(Chars(width=4, break_long_words=False).wrap("ab cd ef"))

tw = TextWrapper(width=10)
print(tw.wrap("one two three four five"))
tw.width = 14
print(tw.wrap("one two three four five"))
tw.initial_indent = "* "
tw.subsequent_indent = "  "
print(tw.wrap("one two three four five"))
tw.max_lines = 2
print(tw.wrap("one two three four five six seven"))
tw.placeholder = "..."
print(tw.wrap("one two three four five six seven"))
tw.max_lines = None
tw.break_on_hyphens = False
print(tw.wrap("a-b-c-d-e-f-g-h-i-j-k-l"))
tw.break_on_hyphens = True
print(tw.wrap("a-b-c-d-e-f-g-h-i-j-k-l"))
tw.expand_tabs = False
tw.replace_whitespace = False
print(tw.wrap("a\tb\nc"))
tw.width = 0
show("mutated width 0", lambda: tw.wrap("abc"))
tw.width = "wide"
show("mutated width str", lambda: tw.wrap("abc"))
del tw.placeholder
tw.width = 10
tw.max_lines = 1
show("deleted placeholder", lambda: tw.wrap("abc def ghi jkl mno"))
tw.placeholder = " [...]"
print(tw.wrap("abc def ghi jkl mno"))
delattr(tw, "initial_indent")
show("deleted initial_indent", lambda: tw.wrap("abc"))

# class-level overrides
class Wide(TextWrapper):
    unicode_whitespace_trans = {ord("_"): ord(" ")}


print(Wide(width=20).wrap("a_b_c\nd"))
print(Wide(width=20, replace_whitespace=False).wrap("a_b\tc"))

# patched regexes
class Dots(TextWrapper):
    wordsep_simple_re = __import__("re").compile(r"(\.+)")
    sentence_end_re = __import__("re").compile(r"x\Z")


print(Dots(width=4, break_on_hyphens=False).wrap("ab...cd.ef"))
print(Dots(width=20, break_on_hyphens=False, fix_sentence_endings=True).wrap("ax.a b"))

# direct use of the private hooks
w = TextWrapper(width=10)
chunks = w._split_chunks("aaa bbb ccc ddd")
print(chunks)
print(w._wrap_chunks(chunks), chunks)
chunks = ["foo.", " ", "Bar", " ", "baz?", " ", "x"]
w._fix_sentence_endings(chunks)
print(chunks)
chunks = ["a.", " ", "b", " ", "c.", "  ", "d", " ", "e!'", " ", "f"]
w._fix_sentence_endings(chunks)
print(chunks)
show("fix none", lambda: w._fix_sentence_endings([]))
show("fix non-str", lambda: w._fix_sentence_endings([None, " "]))
show("fix tuple", lambda: w._fix_sentence_endings(("a.", " ")))

rev = ["xyz"]
line = []
w._handle_long_word(rev, line, 0, 2)
print(rev, line)
rev = ["abcdef-ghi"]
line = ["q"]
w._handle_long_word(rev, line, 1, 8)
print(rev, line)
rev = ["abcdef"]
line = []
w.break_long_words = False
w._handle_long_word(rev, line, 0, 2)
print(rev, line)
rev = ["abcdef"]
line = ["z"]
w._handle_long_word(rev, line, 1, 2)
print(rev, line)
w.break_long_words = True
rev = ["abcdef"]
line = ["z"]
w._handle_long_word(rev, line, 5, 0)
print(rev, line)
rev = ["abcdef"]
line = []
w._handle_long_word(rev, line, 3, 3)
print(rev, line)
show("handle empty", lambda: w._handle_long_word([], [], 0, 5))
show("handle args", lambda: w._handle_long_word([]))

# the module-level functions dispatch through the module's TextWrapper
orig = textwrap.TextWrapper


class Marked(orig):
    def wrap(self, text):
        return ["marked"] + super().wrap(text)

    def fill(self, text):
        return "FILL:" + super().fill(text)


textwrap.TextWrapper = Marked
try:
    print(textwrap.wrap("a b c", 3))
    print(textwrap.fill("a b c", 3))
    print(textwrap.shorten("a b c", 30))
finally:
    textwrap.TextWrapper = orig
print(textwrap.wrap("a b c", 3))

# __init__ is a plain function: subclasses chain it through super()
class WithInit(TextWrapper):
    def __init__(self, *args, extra=0, **kwargs):
        super().__init__(*args, **kwargs)
        self.extra = extra


wi = WithInit(12, extra=3, initial_indent="-")
print(wi.extra, wi.width, wi.initial_indent, wi.wrap("hello world foo"))
print(TextWrapper.__init__(wi, 7) is None, wi.width)

# -- longer realistic text --------------------------------------------------------
PARA = ("Python's textwrap module -- the standard library's text-wrapping helper -- "
        "handles well-known edge cases: hyphenated-words, em--dashes, and very-very-very-"
        "long-compound-words-with-many-hyphens. It also supports sentence-ending fixes!  "
        "Does it work? \"Yes.\" 'Maybe.' Indeed.")
for width in (12, 30, 60):
    print("\n".join(textwrap.wrap(PARA, width)))
    print("\n".join(textwrap.wrap(PARA, width, fix_sentence_endings=True, break_long_words=False)))
    print(textwrap.fill(PARA, width, initial_indent="   ", subsequent_indent="  ", max_lines=4))
print(textwrap.shorten(PARA, 60), "|", textwrap.shorten(PARA, 120, placeholder="[..]"))

# -- interplay with other stdlib users -------------------------------------------------
import argparse

parser = argparse.ArgumentParser(prog="demo", description="A " + "very long description " * 8,
                                 formatter_class=argparse.HelpFormatter)
parser.add_argument("--flag", help="some help text " * 10)
print(parser.format_help())
