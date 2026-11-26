# CPython 3.12.3 parity tests for the 100% native `shlex` module (issue #3356).
import io
import os
import shlex
import sys
import tempfile

# Module surface and instance layout
print("module doc:", shlex.__doc__)
print("class doc:", shlex.shlex.__doc__)
print("class keys:", sorted(k for k in shlex.shlex.__dict__ if k not in ("__dict__", "__weakref__")))
lexv = shlex.shlex("a b")
print("vars posix=False:", list(vars(lexv)))
print("eof non-posix:", repr(lexv.eof), "wordchars len:", len(lexv.wordchars))
lexv = shlex.shlex("a b", posix=True, punctuation_chars=True)
print("vars punct:", list(vars(lexv)))
print("eof posix:", repr(lexv.eof), "wordchars:", lexv.wordchars)
print(
    "defaults:",
    lexv.commenters,
    repr(lexv.whitespace),
    lexv.whitespace_split,
    repr(lexv.quotes),
    repr(lexv.escape),
    repr(lexv.escapedquotes),
    lexv.lineno,
    lexv.debug,
    repr(lexv.token),
    lexv.source,
    lexv.state == " ",
    lexv.infile,
)
try:
    lexv.punctuation_chars = "x"
except AttributeError as e:
    print("punctuation_chars setter:", type(e).__name__)
print("iter is self:", iter(lexv) is lexv)
print("custom punct wordchars:", shlex.shlex("", punctuation_chars="a-").wordchars)
print("punct iterable:", shlex.shlex("", punctuation_chars=["a", "b"]).wordchars)
try:
    shlex.shlex("", punctuation_chars=["ab"])
except Exception as e:
    print("punct bad iterable:", type(e).__name__, e)

# Non-POSIX tokenisation
for text in ["a b 'c d' \"e f\" g", "x=1 y='2 3'", "foo;bar", "a#b c", "'unterminated"]:
    lex = shlex.shlex(text)
    try:
        print("nonposix", repr(text), list(lex))
    except ValueError as e:
        print("nonposix", repr(text), "ValueError", e, repr(lex.state), repr(lex.token))

# POSIX with comments, escapes, state after errors
for text in ["a 'b c' \"d \\\" e\" f\\ g # c\nh", "a\\", "\"abc", "'a' 'b", "a #c\n#d\nb"]:
    lex = shlex.shlex(text, posix=True)
    try:
        print("posix", repr(text), list(lex), lex.lineno)
    except ValueError as e:
        print("posix", repr(text), "ValueError", e, repr(lex.state), repr(lex.token), lex.lineno)

# whitespace_split with and without punctuation
lex = shlex.shlex("a;b && c|d", posix=True, punctuation_chars=True)
lex.whitespace_split = True
print("ws_split punct:", list(lex))
lex = shlex.shlex("ls -l | grep 'a b'>out;(x)", posix=True, punctuation_chars=True)
print("punct pipeline:", list(lex))
lex = shlex.shlex("a>b", punctuation_chars=True)
print("punct non-posix:", list(lex))

# get_token / read_token / eof
lex = shlex.shlex("one two", posix=True)
print(lex.read_token(), lex.read_token(), lex.read_token(), lex.read_token(), lex.state)
lex = shlex.shlex("one", posix=False)
print(repr(lex.get_token()), repr(lex.get_token()), repr(lex.get_token()))
lex = shlex.shlex("''", posix=True)
print("empty quoted:", list(lex))
lex = shlex.shlex("a\nb\n\nc", posix=True)
print("lineno:", list(lex), lex.lineno)

# push_token ordering and EOF
lex = shlex.shlex("", posix=True)
lex.push_token("x")
lex.push_token("y")
print("pushed:", list(lex), repr(lex.get_token()))
print("pushback type:", type(lex.pushback).__name__, type(lex.filestack).__name__)

# error_leader
lex = shlex.shlex("a", infile="f.txt")
print(lex.error_leader(), lex.error_leader("g", 7), lex.error_leader(lineno=3), lex.error_leader(infile="h"))
lex = shlex.shlex("a")
print(lex.error_leader())
try:
    lex.error_leader("g", "x")
except TypeError as e:
    print("error_leader TypeError:", e)


# Source inclusion with sourcehook override
class MyLex(shlex.shlex):
    def sourcehook(self, newfile):
        print("hook:", newfile, "infile:", self.infile)
        return (newfile, io.StringIO("inc1 inc2"))


lex = MyLex('a source "x.inc" b', infile="top", posix=True)
lex.source = "source"
print("source include:", list(lex), lex.lineno, lex.infile)


class Failing(shlex.shlex):
    def sourcehook(self, newfile):
        return None


lex = Failing("a source q b", posix=True)
lex.source = "source"
print("source none spec:", list(lex))


class BadSpec(shlex.shlex):
    def sourcehook(self, newfile):
        return (1, 2, 3)


lex = BadSpec("a source q b", posix=True)
lex.source = "source"
try:
    list(lex)
except ValueError as e:
    print("bad spec:", e)

# Real sourcehook behaviour
with tempfile.TemporaryDirectory() as d:
    inc = os.path.join(d, "inc.txt")
    with open(inc, "w") as fh:
        fh.write("from_file1\nfrom_file2\n")
    top = os.path.join(d, "top.txt")
    lex = shlex.shlex('start source "inc.txt" end', infile=top, posix=True)
    lex.source = "source"
    print("sourcehook rel:", list(lex))
    lex = shlex.shlex('start source "%s" end' % inc.replace("\\", "/"), infile=top, posix=True)
    lex.source = "source"
    print("sourcehook quoted abs:", list(lex))
    lex = shlex.shlex("x", infile=None)
    f = lex.sourcehook('"%s"' % inc)
    print("hook tuple:", f[0] == inc, type(f[1]).__name__)
    f[1].close()
    lex = shlex.shlex("x", infile=top)
    f = lex.sourcehook("inc.txt")
    print("hook rel:", f[0] == inc)
    f[1].close()
try:
    shlex.shlex("x").sourcehook("")
except IndexError as e:
    print("sourcehook empty:", e)

# pop_source on an empty stack
lex = shlex.shlex("x")
try:
    lex.pop_source()
except IndexError as e:
    print("pop_source empty:", e)

# push_source with debug output
class NamedStream(io.StringIO):
    def __repr__(self):
        return "<NamedStream>"


lex = shlex.shlex(NamedStream("z"), infile="f", posix=True)
lex.debug = 1
lex.push_source(NamedStream("inner"), "in.txt")
print(lex.get_token())
lex.push_token("t")
print(lex.get_token())
print(lex.get_token())
print(lex.get_token())
lex.push_source(NamedStream("q"))
lex.pop_source()
print("debug done", lex.lineno)

# debug levels 2 and 3
lex = shlex.shlex("a 'b' \\c #x\n", posix=True)
lex.debug = 3
print(list(lex))
lex = shlex.shlex("a;(b)", posix=False, punctuation_chars=True)
lex.debug = 2
print(list(lex))
lex = shlex.shlex("'x", posix=True)
lex.debug = 2
try:
    list(lex)
except ValueError as e:
    print("debug quote eof:", e)
lex = shlex.shlex("x\\", posix=True)
lex.debug = 2
try:
    list(lex)
except ValueError as e:
    print("debug escape eof:", e)
shlex._print_tokens(shlex.shlex("p q"))

# Customised character classes
lex = shlex.shlex("a:b;c", posix=True)
lex.wordchars += ":"
print("custom wordchars:", list(lex))
lex = shlex.shlex("a'b'c", posix=True)
lex.quotes = ""
print("no quotes:", list(lex))
lex = shlex.shlex("a 'b' c", posix=True)
lex.quotes = "'"
lex.whitespace = " "
lex.whitespace_split = True
print("custom ws:", list(lex))
lex = shlex.shlex("x y", posix=True)
lex.whitespace = [" "]
print("list whitespace:", list(lex))


# Subclass hooking read_token / get_token
class Counting(shlex.shlex):
    n = 0

    def read_token(self):
        Counting.n += 1
        return super().read_token()


print("counting:", list(Counting("a b c", posix=True)), Counting.n)


# Streams: non-StringIO instream, str subclass instream
class MyStr(str):
    pass


print("str subclass instream:", list(shlex.shlex(MyStr("m n"), posix=True)))
print("file-like:", list(shlex.shlex(io.StringIO("f g"), posix=True)))
lex = shlex.shlex()
print("default instream is stdin:", lex.instream is sys.stdin, lex.infile)

# split
print("split subclass:", shlex.split(MyStr("a 'b c'")))
print("split comments & posix:", shlex.split("a # c\nb", True, False))
print("split file:", shlex.split(io.StringIO("x 'y z'")))
print("split kwargs:", shlex.split(s="a b", comments=True, posix=True))
try:
    shlex.split(None)
except ValueError as e:
    print("split bad:", e)
try:
    shlex.split(123)
except Exception as e:
    print("split int:", type(e).__name__)
print("split unicode:", shlex.split("é 'ü ö' ß"))
print("split nonposix quotes:", shlex.split("a 'b c'd", posix=False))


# quote / join
class QStr(str):
    pass


safe = QStr("abc")
print("quote returns same obj:", shlex.quote(safe) is safe, type(shlex.quote(QStr("a b"))).__name__)
s = "plain"
print("quote same str:", shlex.quote(s) is s)
for v in [0, None, [], b"", False, 12, 1.5, b"ab", bytearray(b"x"), ["a"], ("a",), object]:
    try:
        print("quote", repr(v), "->", repr(shlex.quote(v)))
    except TypeError as e:
        print("quote", repr(v), "TypeError", e)
print("quote ascii only:", shlex.quote("é"), shlex.quote("a_b"), shlex.quote("a\tb"), shlex.quote("~"))
print("join generator:", shlex.join(x for x in ["a b", "c"]))
print("join tuple/str:", shlex.join(("a", "b c")), shlex.join("ab"))
for bad in [5, None, [1], ["a", None]]:
    try:
        print("join", bad, shlex.join(bad))
    except TypeError as e:
        print("join", bad, "TypeError", e)
real_quote = shlex.quote
shlex.quote = lambda s: "<%s>" % s
print("join uses module quote:", shlex.join(["a", "b"]))
shlex.quote = real_quote
print("shlex native ok")
