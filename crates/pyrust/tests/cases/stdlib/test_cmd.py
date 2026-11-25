"""CPython 3.12 parity tests for the `cmd` module (issue #3228)."""

import cmd
import io
import sys
import types


# --- Public surface & constants ---
print("__all__:", cmd.__all__)
print("PROMPT:", repr(cmd.PROMPT))
print("IDENTCHARS:", repr(cmd.IDENTCHARS))
print("Cmd.__module__:", cmd.Cmd.__module__)
print(
    "class defaults:",
    repr(cmd.Cmd.prompt),
    repr(cmd.Cmd.ruler),
    repr(cmd.Cmd.lastcmd),
    repr(cmd.Cmd.intro),
    repr(cmd.Cmd.doc_leader),
    repr(cmd.Cmd.doc_header),
    repr(cmd.Cmd.misc_header),
    repr(cmd.Cmd.undoc_header),
    repr(cmd.Cmd.nohelp),
    cmd.Cmd.use_rawinput,
)


class DemoShell(cmd.Cmd):
    prompt = "demo> "
    intro = "Default intro"

    def __init__(self, stdin=None, stdout=None):
        super().__init__(stdin=stdin, stdout=stdout)
        self.use_rawinput = False
        self.events = []

    def preloop(self):
        self.events.append("preloop")

    def postloop(self):
        self.events.append("postloop")

    def precmd(self, line):
        self.events.append(("precmd", line))
        return line

    def postcmd(self, stop, line):
        self.events.append(("postcmd", bool(stop), line))
        return stop

    def do_greet(self, arg):
        """Greet someone by name."""
        self.stdout.write(f"Hello, {arg or 'world'}!\n")

    def complete_greet(self, text, line, begidx, endidx):
        names = ["alice", "alex", "bob"]
        return [n for n in names if n.startswith(text)]

    def do_echo(self, arg):
        self.stdout.write(f"{arg}\n")

    def do_shell(self, arg):
        """Run a shell command."""
        self.stdout.write(f"SHELL:{arg}\n")

    def help_topics_misc(self):
        self.stdout.write("Misc topic help text.\n")

    def help_echo(self):
        self.stdout.write("Custom help for echo.\n")

    def do_undoc(self, arg):
        pass

    def do_quit(self, arg):
        """Exit the shell."""
        self.stdout.write("Bye!\n")
        return True

    def do_EOF(self, arg):
        self.stdout.write("EOF reached\n")
        return True


# --- 1. parseline ---
sh = DemoShell(stdin=io.StringIO(), stdout=io.StringIO())
print("parseline empty:", sh.parseline("   "))
print("parseline cmd+arg:", sh.parseline("  greet  Alice  "))
print("parseline ?:", sh.parseline("?greet"))
print("parseline ? bare:", sh.parseline("?"))
print("parseline ! with do_shell:", sh.parseline("!ls -la"))
print("parseline punct:", sh.parseline("@@@ foo"))

# Without do_shell, `!` returns (None, None, line)
base_cmd = cmd.Cmd(stdin=io.StringIO(), stdout=io.StringIO())
print("parseline ! without do_shell:", base_cmd.parseline("!ls -la"))


# --- 2. onecmd, emptyline (repeats lastcmd), default, ? and ! shortcuts ---
out = io.StringIO()
sh2 = DemoShell(stdin=io.StringIO(), stdout=out)
sh2.onecmd("greet Alice")
sh2.onecmd("")  # repeats `greet Alice`
sh2.onecmd("!whoami")
sh2.onecmd("?greet")
sh2.onecmd("?echo")
sh2.onecmd("?topics_misc")
sh2.onecmd("?nonexistent")
sh2.onecmd("bogus 123")
sh2.onecmd("@@@")
sh2.onecmd("help")
print("=== onecmd output ===")
print(out.getvalue(), end="")


# --- 3. cmdloop with io.StringIO stdin/stdout, cmdqueue, hooks, EOF ---
script = "greet Bob\n\n!date\nquit\n"
in_buf = io.StringIO(script)
out_buf = io.StringIO()
sh3 = DemoShell(stdin=in_buf, stdout=out_buf)
sh3.cmdqueue.append("echo queued_first")
sh3.cmdloop(intro="Custom Intro!")
print("=== cmdloop output ===")
print(out_buf.getvalue(), end="")
print("cmdloop events:", sh3.events)

# EOF handling in cmdloop (empty stdin -> dispatches do_EOF)
out_eof = io.StringIO()
sh_eof = DemoShell(stdin=io.StringIO(""), stdout=out_eof)
sh_eof.intro = ""
sh_eof.cmdloop()
print("EOF output:", repr(out_eof.getvalue()))
print("EOF lastcmd cleared:", repr(sh_eof.lastcmd))


# --- 4. columnize & print_topics ---
col_out = io.StringIO()
sh_col = DemoShell(stdin=io.StringIO(), stdout=col_out)
sh_col.columnize([])
sh_col.columnize(["onlyone"])
sh_col.columnize(["a", "bb", "ccc", "dddd", "eeeee", "ff", "g", "hh"], displaywidth=15)
sh_col.columnize(["verylongword1", "verylongword2"], displaywidth=10)
sh_col.ruler = "-"
sh_col.print_topics("My Header", ["alpha", "beta", "gamma"], 15, 40)
sh_col.ruler = ""
sh_col.print_topics("No Ruler", ["one", "two"], 15, 40)
print("=== columnize & print_topics ===")
print(col_out.getvalue(), end="")

# Non-string in columnize raises TypeError
try:
    sh_col.columnize(["ok", 123, "also_ok", None])
except TypeError as exc:
    print("columnize TypeError:", str(exc))


# --- 5. completenames, complete_help, completedefault, complete() with stub readline ---
print("completenames 'g':", sorted(sh.completenames("g")))
print("completenames '':", sorted(sh.completenames("")))
print("complete_help 't':", sorted(sh.complete_help("t")))
print("complete_help 'e':", sorted(sh.complete_help("e")))
print("completedefault:", sh.completedefault("x", "cmd x", 4, 5))

fake_rl = types.ModuleType("readline")
state_rl = {"buf": "", "beg": 0, "end": 0}
fake_rl.get_line_buffer = lambda: state_rl["buf"]
fake_rl.get_begidx = lambda: state_rl["beg"]
fake_rl.get_endidx = lambda: state_rl["end"]

had_rl = "readline" in sys.modules
orig_rl = sys.modules.get("readline")
sys.modules["readline"] = fake_rl
try:
    # Command name completion (begidx == 0)
    state_rl.update(buf="gr", beg=0, end=2)
    c0 = sh.complete("gr", 0)
    c1 = sh.complete("gr", 1)
    print("complete cmd 'gr':", c0, c1)

    # Command argument completion (begidx > 0 -> complete_greet)
    state_rl.update(buf="greet al", beg=6, end=8)
    matches = []
    idx = 0
    while True:
        m = sh.complete("al", idx)
        if m is None:
            break
        matches.append(m)
        idx += 1
    print("complete greet 'al':", matches)

    # Leading spaces stripped properly before computing begidx
    state_rl.update(buf="   greet b", beg=9, end=10)
    print("complete leading spaces:", sh.complete("b", 0), sh.complete("b", 1))

    # Unknown command falls back to completedefault
    state_rl.update(buf="unknown x", beg=8, end=9)
    print("complete unknown:", sh.complete("x", 0))
finally:
    if had_rl:
        sys.modules["readline"] = orig_rl
    else:
        sys.modules.pop("readline", None)

print("cmd parity ok")
