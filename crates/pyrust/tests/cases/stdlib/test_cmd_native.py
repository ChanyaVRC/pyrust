# CPython 3.12.3 parity tests for the native `cmd` module (issue #3477).
#
# Everything printed here must be identical under CPython 3.12.3 on Linux and
# Windows.  `list(set)` results are always sorted before they are shown.

import cmd
import io
import sys
import types


def err(fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except BaseException as e:
        return "%s: %s" % (type(e).__name__, e)
    return "no error"


# 1. Module surface -----------------------------------------------------------
print("__all__:", cmd.__all__)
print("PROMPT:", repr(cmd.PROMPT))
print("IDENTCHARS:", repr(cmd.IDENTCHARS))
print("Cmd.__module__:", cmd.Cmd.__module__, cmd.Cmd.__name__, cmd.Cmd.__qualname__)
print("module doc head:", cmd.__doc__.splitlines()[0])
print("module doc len:", len(cmd.__doc__))
print("class doc head:", cmd.Cmd.__doc__.splitlines()[0])
print("class doc len:", len(cmd.Cmd.__doc__))
for name in (
    "prompt", "identchars", "ruler", "lastcmd", "intro", "doc_leader",
    "doc_header", "misc_header", "undoc_header", "nohelp", "use_rawinput",
):
    print("class attr:", name, repr(getattr(cmd.Cmd, name)))
print("sys is sys:", cmd.sys is sys, "string:", cmd.string.ascii_letters[:3])
for name in (
    "__init__", "cmdloop", "precmd", "postcmd", "preloop", "postloop",
    "parseline", "onecmd", "emptyline", "default", "completedefault",
    "completenames", "complete", "get_names", "complete_help", "do_help",
    "print_topics", "columnize",
):
    doc = getattr(cmd.Cmd, name).__doc__
    print("doc:", name, None if doc is None else (len(doc), doc.splitlines()[0]))
print("dir(Cmd) public:", [n for n in dir(cmd.Cmd) if not n.startswith("__")])

# 2. __init__ -----------------------------------------------------------------
c = cmd.Cmd()
print("init defaults:", c.stdin is sys.stdin, c.stdout is sys.stdout,
      c.cmdqueue, c.completekey)
inp, out = io.StringIO("x"), io.StringIO()
c = cmd.Cmd("tab2", inp, out)
print("init explicit:", c.stdin is inp, c.stdout is out, c.completekey)
c = cmd.Cmd(completekey=None, stdout=out)
print("init kw:", c.completekey, c.stdout is out, c.stdin is sys.stdin)
print("init cmdqueue distinct:", cmd.Cmd().cmdqueue is not cmd.Cmd().cmdqueue)
print("init too many:", err(cmd.Cmd, "a", None, None, "d"))
print("init bad kw:", err(cmd.Cmd, bogus=1))
print("init dup kw:", err(cmd.Cmd, "tab", completekey="x"))
real_stdout = sys.stdout
sys.stdout = io.StringIO()
c = cmd.Cmd()
print("init dynamic stdout:", c.stdout is sys.stdout, file=real_stdout)
sys.stdout = real_stdout
print("init stdout restored:", c.stdout is not sys.stdout)

# 3. parseline ----------------------------------------------------------------
c = cmd.Cmd()
for line in ("", "   ", "foo bar", "  foo   bar baz  ", "?foo", "?", "? foo",
             "!ls", "!", "foo", "foo_bar1 x", "foo-bar", "foo.bar x", "1abc",
             "@x", "x@y z", "\tfoo\t"):
    print("parseline no-shell:", repr(line), c.parseline(line))


class WithShell(cmd.Cmd):
    def do_shell(self, arg):
        pass


w = WithShell()
for line in ("!ls", "!", "! ls -l", "  !x y", "?foo"):
    print("parseline shell:", repr(line), w.parseline(line))


class IdentCmd(cmd.Cmd):
    identchars = cmd.IDENTCHARS + "-."


i = IdentCmd()
for line in ("foo-bar x", "foo.bar x", "a-b-c"):
    print("parseline identchars:", repr(line), i.parseline(line))


class ListIdent(cmd.Cmd):
    identchars = ["a", "b"]


print("parseline list identchars:", ListIdent().parseline("abba cc"))
print("parseline non-str:", err(c.parseline, None))
print("parseline non-str int:", err(c.parseline, 5))
print("parseline missing:", err(c.parseline))
print("parseline kw:", c.parseline(line="x y"))

# 4. onecmd / emptyline / default / lastcmd ------------------------------------


class Rec(cmd.Cmd):
    def __init__(self, **kw):
        super().__init__(**kw)
        self.calls = []

    def do_a(self, arg):
        """Doc for a."""
        self.calls.append(("a", arg))

    def do_ret(self, arg):
        self.calls.append(("ret", arg))
        return "ret-value"

    def do_EOF(self, arg):
        self.calls.append(("EOF", arg))
        return True


out = io.StringIO()
r = Rec(stdout=out)
print("onecmd a:", r.onecmd("a  1 2"), r.calls, repr(r.lastcmd))
print("onecmd ret:", r.onecmd("ret x"), repr(r.lastcmd))
print("onecmd empty repeats:", r.onecmd(""), r.calls[-1], repr(r.lastcmd))
print("onecmd blank repeats:", r.onecmd("   "), r.calls[-1])
print("onecmd unknown:", r.onecmd("nosuch x"), repr(out.getvalue()),
      repr(r.lastcmd))
out.truncate(0), out.seek(0)
print("onecmd '!x' no shell:", r.onecmd("!x"), repr(out.getvalue()),
      repr(r.lastcmd))
out.truncate(0), out.seek(0)
print("onecmd '?a':", r.onecmd("?a"), repr(out.getvalue()))
out.truncate(0), out.seek(0)
print("onecmd EOF:", r.onecmd("EOF"), repr(r.lastcmd), r.calls[-1])
print("onecmd after EOF empty:", r.onecmd(""), len(r.calls))
out.truncate(0), out.seek(0)
r2 = Rec(stdout=out)
print("onecmd empty no lastcmd:", r2.onecmd(""), repr(out.getvalue()))
print("onecmd digits:", r2.onecmd("12"), repr(out.getvalue()))
out.truncate(0), out.seek(0)
print("onecmd '-' cmd empty:", r2.onecmd("-x"), repr(out.getvalue()),
      repr(r2.lastcmd))
print("onecmd arg none:", err(r2.onecmd, None))
print("onecmd missing:", err(r2.onecmd))
print("emptyline direct:", r2.emptyline(), repr(r2.lastcmd))
r2.lastcmd = "a hello"
print("emptyline repeats:", r2.emptyline(), r2.calls[-1])
print("default direct:", r2.default("zzz"), repr(out.getvalue()))
out.truncate(0), out.seek(0)
print("default tuple:", err(r2.default, ("t", 1)))
print("default missing:", err(r2.default))


class Overrides(cmd.Cmd):
    def parseline(self, line):
        return "a", "overridden:" + line, line

    def do_a(self, arg):
        return "got " + arg


print("onecmd override parseline:", Overrides().onecmd("hello"))


class OverEmpty(cmd.Cmd):
    def emptyline(self):
        return "custom-empty"

    def default(self, line):
        return "custom-default:" + line


oe = OverEmpty()
print("override emptyline/default:", oe.onecmd(""), oe.onecmd("qq 1"))
print("override lastcmd after unknown:", repr(oe.lastcmd))


class AttrErr(cmd.Cmd):
    def do_boom(self, arg):
        raise AttributeError("inner failure")


print("onecmd inner AttributeError propagates:", err(AttrErr().onecmd, "boom"))


class GetattrCmd(cmd.Cmd):
    def __getattr__(self, name):
        if name == "do_dyn":
            return lambda arg: "dyn:" + arg
        raise AttributeError(name)


print("onecmd __getattr__ do_:", GetattrCmd().onecmd("dyn zz"))

# 5. cmdloop ------------------------------------------------------------------


class Loop(cmd.Cmd):
    prompt = "(loop) "
    intro = "Class intro."
    use_rawinput = False

    def __init__(self, text="", **kw):
        super().__init__(stdin=io.StringIO(text), stdout=io.StringIO(), **kw)
        self.events = []

    def preloop(self):
        self.events.append("preloop")

    def postloop(self):
        self.events.append("postloop")

    def precmd(self, line):
        self.events.append(("precmd", line))
        return line

    def postcmd(self, stop, line):
        self.events.append(("postcmd", stop, line))
        return stop

    def do_hi(self, arg):
        """Say hi."""
        self.stdout.write("hi %s\n" % arg)

    def do_stop(self, arg):
        return arg or True

    def do_EOF(self, arg):
        self.stdout.write("<EOF>\n")
        return True


lp = Loop("hi there\n\nhi again\r\nstop\nhi never\n")
print("cmdloop ret:", lp.cmdloop())
print("cmdloop out:", repr(lp.stdout.getvalue()))
for ev in lp.events:
    print("  event:", ev)
print("cmdloop remaining stdin:", repr(lp.stdin.read()))

lp = Loop("hi x\n")
lp.cmdloop("Passed intro")
print("cmdloop eof out:", repr(lp.stdout.getvalue()))
print("cmdloop intro stored:", repr(lp.intro))

lp = Loop("")
lp.intro = None
lp.cmdloop()
print("cmdloop no intro eof:", repr(lp.stdout.getvalue()), lp.events)

lp = Loop("hi 1\n")
lp.intro = ""
lp.cmdloop()
print("cmdloop empty intro:", repr(lp.stdout.getvalue()))

lp = Loop("hi 1\nhi 2\n")
lp.cmdqueue.extend(["hi q1", "hi q2", "stop"])
lp.cmdloop()
print("cmdloop cmdqueue:", repr(lp.stdout.getvalue()), repr(lp.stdin.read()),
      lp.cmdqueue)

lp = Loop("hi 1\nhi 2\n")
lp.cmdqueue.append("hi from queue")
lp.cmdqueue.append("EOF")
lp.cmdloop()
print("cmdloop queue EOF:", repr(lp.stdout.getvalue()), repr(lp.stdin.read()))


class PostStop(Loop):
    def postcmd(self, stop, line):
        return line == "hi 2"


lp = PostStop("hi 1\nhi 2\nhi 3\n")
lp.cmdloop()
print("cmdloop postcmd stop:", repr(lp.stdout.getvalue()))


class PreMod(Loop):
    def precmd(self, line):
        return line.upper() if line.startswith("hi") else line


lp = PreMod("hi a\nEOF\n")
lp.cmdloop()
print("cmdloop precmd mod:", repr(lp.stdout.getvalue()))

lp = Loop("hi 1\n")
lp.cmdloop()
print("cmdloop stdin lines w/ unknown:", repr(lp.stdout.getvalue()))


class Failing(Loop):
    def do_hi(self, arg):
        raise ValueError("boom " + arg)


lp = Failing("hi 1\nhi 2\n")
print("cmdloop exception:", err(lp.cmdloop))
print("cmdloop exception events:", lp.events)


class RawLoop(cmd.Cmd):
    prompt = "RAW> "
    use_rawinput = 1
    completekey = None

    def do_x(self, arg):
        print("x:", arg)

    def do_quit(self, arg):
        return True


real_stdin = sys.stdin
sys.stdin = io.StringIO("x one\nquit\n")
buf = io.StringIO()
real = sys.stdout
sys.stdout = buf
try:
    RawLoop().cmdloop()
finally:
    sys.stdout = real
    sys.stdin = real_stdin
print("cmdloop rawinput:", repr(buf.getvalue()))

sys.stdin = io.StringIO("")
buf = io.StringIO()
sys.stdout = buf
try:
    class RawEOF(RawLoop):
        def do_EOF(self, arg):
            print("saw EOF")
            return True
    RawEOF().cmdloop()
finally:
    sys.stdout = real
    sys.stdin = real_stdin
print("cmdloop rawinput EOF:", repr(buf.getvalue()))


class FakeReadline(types.ModuleType):
    def __init__(self):
        super().__init__("readline")
        self.log = []
        self.completer = "orig"
        self.line = ""
        self.beg = 0
        self.end = 0

    def get_completer(self):
        self.log.append("get_completer")
        return self.completer

    def set_completer(self, fn):
        self.log.append(("set_completer", getattr(fn, "__name__", fn)))
        self.completer = fn

    def parse_and_bind(self, s):
        self.log.append(("parse_and_bind", s))

    def get_line_buffer(self):
        return self.line

    def get_begidx(self):
        return self.beg

    def get_endidx(self):
        return self.end


saved_readline = sys.modules.get("readline", None)
fake = FakeReadline()
sys.modules["readline"] = fake


class CompLoop(cmd.Cmd):
    prompt = ""

    def do_quit(self, arg):
        return True


sys.stdin = io.StringIO("quit\n")
buf = io.StringIO()
sys.stdout = buf
try:
    CompLoop().cmdloop()
finally:
    sys.stdout = real
    sys.stdin = real_stdin
print("cmdloop readline log:", fake.log, repr(fake.completer))

fake.log.clear()
fake.completer = "orig2"
sys.stdin = io.StringIO("quit\n")
sys.stdout = io.StringIO()
try:
    CompLoop(completekey="ctrl-i").cmdloop()
finally:
    sys.stdout = real
    sys.stdin = real_stdin
print("cmdloop custom key log:", fake.log)

fake.log.clear()
sys.stdin = io.StringIO("quit\n")
sys.stdout = io.StringIO()
try:
    CompLoop(completekey=None).cmdloop()
finally:
    sys.stdout = real
    sys.stdin = real_stdin
print("cmdloop no completekey log:", fake.log)

fake.log.clear()


class RawBoom(CompLoop):
    def do_quit(self, arg):
        raise RuntimeError("nope")


sys.stdin = io.StringIO("quit\n")
sys.stdout = io.StringIO()
try:
    result = err(RawBoom().cmdloop)
finally:
    sys.stdout = real
    sys.stdin = real_stdin
print("cmdloop error restores completer:", result, fake.log[-1])

# readline unimportable -> ImportError ignored
sys.modules["readline"] = None
sys.stdin = io.StringIO("quit\n")
sys.stdout = io.StringIO()
try:
    result = err(CompLoop().cmdloop)
finally:
    sys.stdout = real
    sys.stdin = real_stdin
print("cmdloop readline missing:", result)

# 6. completion -----------------------------------------------------------------
sys.modules["readline"] = fake


class Comp(cmd.Cmd):
    def do_alpha(self, arg):
        pass

    def do_alpine(self, arg):
        pass

    def do_beta(self, arg):
        pass

    def help_alpha(self):
        pass

    def help_topic(self):
        pass

    def help_alp(self):
        pass

    def complete_beta(self, text, line, begidx, endidx):
        return [x for x in ("one", "two", "three") if x.startswith(text)] + [
            (line, begidx, endidx)
        ]


cp = Comp()
print("completenames al:", sorted(cp.completenames("al")))
print("completenames '':", sorted(cp.completenames("")))
print("completenames none:", cp.completenames("zzz"))
print("completenames extra args:", sorted(cp.completenames("be", "line", 0, 2)))
print("completenames missing:", err(cp.completenames))
print("completenames kw ignored:", err(cp.completenames, "a", foo=1))
print("completenames non-str:", err(cp.completenames, 5))
print("complete_help al:", sorted(cp.complete_help("al")))
print("complete_help t:", sorted(cp.complete_help("t", "help t", 5, 6)))
print("complete_help '':", sorted(cp.complete_help("")))
print("complete_help empty args:", err(cp.complete_help))
print("complete_help kw:", err(cp.complete_help, "a", x=1))
print("completedefault:", cp.completedefault(), cp.completedefault("a", "b", 1),
      err(cp.completedefault, x=1))
print("get_names type:", type(cp.get_names()).__name__,
      [n for n in cp.get_names() if n.startswith(("do_", "help_", "complete_"))])
print("get_names kw:", err(cp.get_names, x=1))

# complete(): state 0 command-name completion
fake.line, fake.beg, fake.end = "al", 0, 2
res = []
for s in range(5):
    res.append(cp.complete("al", s))
print("complete state seq:", sorted(x for x in res if x is not None),
      res.count(None))
print("complete state beyond:", cp.complete("al", 99), cp.complete("al", 2) in (None, "alpha", "alpine"))
# arg completion with a complete_<cmd>
fake.line, fake.beg, fake.end = "beta t", 5, 6
print("complete arg:", [cp.complete("t", s) for s in range(4)])
fake.line, fake.beg, fake.end = "   beta t", 8, 9
print("complete arg lstrip:", [cp.complete("t", s) for s in range(4)])
# arg completion without complete_<cmd> -> completedefault
fake.line, fake.beg, fake.end = "alpha x", 6, 7
print("complete default:", [cp.complete("x", s) for s in range(2)])
# begidx > 0 but command is empty
fake.line, fake.beg, fake.end = "-x y", 3, 4
print("complete empty cmd:", [cp.complete("y", s) for s in range(2)])
# state != 0 reuses completion_matches
cp.completion_matches = ["m0", "m1"]
print("complete reuse:", cp.complete("q", 1), cp.complete("q", 2), cp.complete("q", 5))
print("complete bad state:", err(cp.complete, "q", "x"))
print("complete negative:", cp.complete("q", -1), err(cp.complete, "q", -3))
fresh = cmd.Cmd()
print("complete no matches attr:", err(fresh.complete, "q", 1))
fake.line, fake.beg, fake.end = "!x y", 3, 4
print("complete bang cmd:", err(cmd.Cmd().complete, "y", 0))

# complete via real readline-less path
sys.modules["readline"] = None
print("complete no readline:", err(cp.complete, "al", 0))
print("complete no readline state1:", err(fresh.complete, "al", 1))
if saved_readline is None:
    del sys.modules["readline"]
else:
    sys.modules["readline"] = saved_readline

# 7. do_help --------------------------------------------------------------------


class Helpful(cmd.Cmd):
    doc_leader = "LEADER"

    def do_documented(self, arg):
        """Documented command."""

    def do_other(self, arg):
        """Other documented."""

    def do_undoc(self, arg):
        pass

    def do_a_very_long_command_name_here(self, arg):
        pass

    def help_topic(self):
        self.stdout.write("Topic help text\n")

    def help_other(self):
        self.stdout.write("Custom other help\n")

    def help_zeta(self):
        self.stdout.write("zeta\n")

    def help_alpha(self):
        self.stdout.write("alpha\n")


def run_help(obj, arg=""):
    obj.stdout = io.StringIO()
    ret = obj.do_help(arg)
    return ret, obj.stdout.getvalue()


h = Helpful()
ret, text = run_help(h)
print("do_help all ret:", ret)
print(text, end="")
print("--- end ---")
print("do_help documented:", run_help(h, "documented"))
print("do_help help_other beats doc:", run_help(h, "other"))
print("do_help help_topic:", run_help(h, "topic"))
print("do_help undoc:", run_help(h, "undoc"))
print("do_help missing:", run_help(h, "missing"))
print("do_help help itself:", run_help(h, "help"))
print("do_help via onecmd:", h.onecmd("help documented"), repr(h.stdout.getvalue()))
h.stdout = io.StringIO()
h.onecmd("?topic")
print("do_help via ?:", repr(h.stdout.getvalue()))
print("do_help non-str arg:", err(h.do_help, 5))
print("do_help missing arg:", err(h.do_help))
print("do_help kw:", run_help(h, arg="documented"))

h2 = Helpful()
h2.ruler = ""
h2.doc_header = "Docs:"
h2.misc_header = "Misc:"
h2.undoc_header = "Undoc:"
ret, text = run_help(h2)
print(text, end="")
print("--- end no ruler ---")
h2.ruler = "-~"
h2.nohelp = "No help for [%s]"
ret, text = run_help(h2)
print(text, end="")
print(run_help(h2, "nope"))
print("--- end custom ruler ---")


class Empty(cmd.Cmd):
    pass


print("do_help bare Cmd:", run_help(Empty()))
print("do_help bare Cmd arg:", run_help(Empty(), "x"))


class DocOnly(cmd.Cmd):
    def do_x(self, arg):
        """x doc"""


print("do_help doc-only:", run_help(DocOnly()))


class Dup(Helpful):
    def do_documented(self, arg):
        """Overridden."""


print("do_help dup:", run_help(Dup())[1].count("documented"))


class CustomNames(cmd.Cmd):
    def get_names(self):
        return ["do_zz", "do_aa", "do_zz", "help_mm", "help_aa", "do_aa", "other"]

    do_zz = lambda self, arg: None  # noqa: E731

    def do_aa(self, arg):
        """aa"""


print("do_help custom get_names:", run_help(CustomNames()))


class CustomHeaders(cmd.Cmd):
    doc_leader = 7
    doc_header = ("a", "b")

    def do_x(self, arg):
        """x"""


print("do_help non-str headers:", run_help(CustomHeaders()))


class WidePrint(cmd.Cmd):
    def print_topics(self, header, cmds, cmdlen, maxcol):
        self.stdout.write("T:%s:%s:%s:%s\n" % (header, cmds, cmdlen, maxcol))


wp = WidePrint()
wp.stdout = io.StringIO()
wp.do_help("")
print("print_topics override:", repr(wp.stdout.getvalue()))

wp.stdout = io.StringIO()
cmd.Cmd.print_topics(wp, "H", [], 15, 80)
print("print_topics empty:", repr(wp.stdout.getvalue()))
print("print_topics kw:", err(wp.print_topics, "h"))


class ColumnOverride(cmd.Cmd):
    def columnize(self, list, displaywidth=80):
        self.stdout.write("COLS %r %r\n" % (list, displaywidth))


co = ColumnOverride()
co.stdout = io.StringIO()
cmd.Cmd.print_topics(co, "Head", ["a", "b"], 15, 80)
print("print_topics columnize hook:", repr(co.stdout.getvalue()))
co.stdout = io.StringIO()
cmd.Cmd.print_topics(co, "Head", ["a"], 15, 100)
co.ruler = None
cmd.Cmd.print_topics(co, "Head", ["a"], 15, 10)
print("print_topics maxcol:", repr(co.stdout.getvalue()))

# 8. columnize ------------------------------------------------------------------


def cols(items, *args, **kwargs):
    c = cmd.Cmd(stdout=io.StringIO())
    ret = c.columnize(items, *args, **kwargs)
    return ret, c.stdout.getvalue()


print("columnize empty:", cols([]))
print("columnize empty tuple:", cols(()))
print("columnize single:", cols(["only"]))
print("columnize two:", cols(["a", "b"]))
print("columnize narrow 8:", cols(["a", "bb", "ccc", "dddd"], 8))
print("columnize narrow 6:", cols(["a", "bb", "ccc", "dddd"], 6))
print("columnize narrow 1:", cols(["a", "bb", "ccc", "dddd"], 1))
print("columnize narrow 0:", cols(["aaa", "bbb"], 0))
print("columnize negative:", cols(["aaa", "bbb"], -5))
words = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
         "iota", "kappa", "lambda", "mu", "nu", "xi", "omicron", "pi", "rho",
         "sigma", "tau", "upsilon", "phi", "chi", "psi", "omega"]
for width in (20, 30, 40, 60, 79, 80, 100, 200):
    print("columnize width", width)
    print(cols(words, width)[1], end="")
print("columnize default width:")
print(cols(words)[1], end="")
print("columnize kw:")
print(cols(words, displaywidth=45)[1], end="")
print("columnize tuple:", cols(("x", "yy", "zzz"), 5))
print("columnize 3 items w15:", cols(["one", "two", "three"], 15))
print("columnize ragged last:", cols(["a", "b", "c", "d", "e"], 7))
print("columnize ragged last2:", cols(list("abcdefg"), 9))
print("columnize with empty strings:", cols(["a", "", "c", "", ""], 20))
print("columnize trailing empty:", cols(["a", "b", ""], 20))
print("columnize unicode:", cols(["é", "ab", "日本語", "x"], 8))
print("columnize long item:", cols(["x" * 100, "y"], 80))
print("columnize long items:", cols(["x" * 50, "y" * 50, "z"], 80))
print("columnize nonstr:", err(cols, ["a", 1, "b", None, 3]))
print("columnize nonstr single:", err(cols, [5]))
print("columnize nonstr one:", err(cols, ["a", 2]))
print("columnize bytes:", err(cols, [b"a", "b"]))
print("columnize str arg:", cols("abc", 80))
print("columnize missing:", err(cmd.Cmd().columnize))
print("columnize width None:", err(cols, ["a", "b"], None))


class StrSub(str):
    pass


print("columnize str subclass:", cols([StrSub("aa"), "b", StrSub("ccc")], 10))
print("columnize float width:", cols(["aa", "bb", "cc"], 4.5))
print("columnize returns None:", cmd.Cmd(stdout=io.StringIO()).columnize(["a"]))

# 9. subclass attribute / method overrides -------------------------------------


class AttrOverride(cmd.Cmd):
    nohelp = "NO HELP FOR %s!"
    ruler = "*"
    doc_header = "DOCS"
    undoc_header = "UNDOCS"
    misc_header = "MISC"
    prompt = "$ "
    use_rawinput = False
    intro = "attr intro"

    def do_a(self, arg):
        """a"""

    def do_b(self, arg):
        pass

    def help_c(self):
        pass

    def do_EOF(self, arg):
        return True


ao = AttrOverride(stdin=io.StringIO("help\nhelp zz\nhelp a\nhelp b\n?c\n"),
                  stdout=io.StringIO())
ao.cmdloop()
print(ao.stdout.getvalue(), end="")
print("--- attr override ---")

# instance attribute overriding class attribute
ia = cmd.Cmd(stdout=io.StringIO())
ia.nohelp = "inst %s"
ia.do_help("q")
print("instance nohelp:", repr(ia.stdout.getvalue()))
ia.do_x = lambda arg: "instance do_x:" + arg
print("instance do_x:", ia.onecmd("x 1"))
ia.stdout = io.StringIO()
ia.do_help("x")
print("instance do_x help:", repr(ia.stdout.getvalue()))
ia.help_x = lambda: ia.stdout.write("instance help_x\n")
ia.do_help("x")
print("instance help_x:", repr(ia.stdout.getvalue()))

# super().__init__ interplay and attribute shadowing
class NoInit(cmd.Cmd):
    def __init__(self):
        pass


ni = NoInit()
print("no init attrs:", [hasattr(ni, n) for n in ("stdin", "stdout", "cmdqueue", "completekey")])
print("no init onecmd default:", err(ni.onecmd, "zzz"))

# method resolution through the class (not captured at import)
orig_default = cmd.Cmd.default
cmd.Cmd.default = lambda self, line: "patched:" + line
print("patched default:", cmd.Cmd().onecmd("qqq"))
cmd.Cmd.default = orig_default
print("restored default:", cmd.Cmd(stdout=io.StringIO()).onecmd("qqq"))

# 10. misc module-level behaviour ----------------------------------------------
print("subclass isinstance:", isinstance(Helpful(), cmd.Cmd), issubclass(Helpful, cmd.Cmd))
print("Cmd() repr type:", type(cmd.Cmd()).__name__)
print("Cmd mro:", [k.__name__ for k in cmd.Cmd.__mro__])
print("star import:", sorted(n for n in {} if False))
ns = {}
exec("from cmd import *", ns)
print("star import names:", sorted(k for k in ns if not k.startswith("__")))
