"""CPython 3.12 parity tests for the `code` module (issue #3228)."""

import code
import codeop
import io
import sys


# --- Public surface & metadata ---
print("__all__:", code.__all__)
print("compile_command identity:", code.compile_command is codeop.compile_command)
print("InteractiveInterpreter.__module__:", code.InteractiveInterpreter.__module__)
print("InteractiveConsole.__module__:", code.InteractiveConsole.__module__)


class CapturingInterpreter(code.InteractiveInterpreter):
    def __init__(self, locals=None):
        super().__init__(locals)
        self.output = []

    def write(self, data):
        self.output.append(data)

    def get_output(self):
        return "".join(self.output)


class CapturingConsole(code.InteractiveConsole):
    def __init__(self, locals=None, filename="<console>", inputs=None):
        super().__init__(locals, filename)
        self.output = []
        self.prompts = []
        self._inputs = list(inputs) if inputs is not None else []

    def write(self, data):
        self.output.append(data)

    def raw_input(self, prompt=""):
        self.prompts.append(prompt)
        if not self._inputs:
            raise EOFError
        item = self._inputs.pop(0)
        if isinstance(item, BaseException):
            raise item
        return item

    def get_output(self):
        return "".join(self.output)


# --- 1. InteractiveInterpreter default locals & runsource ---
interp_default = code.InteractiveInterpreter()
print("default locals:", interp_default.locals["__name__"], interp_default.locals["__doc__"])

interp = CapturingInterpreter({"base": 10})
print("runsource complete assign:", interp.runsource("x = base + 5"))
print("locals x:", interp.locals["x"])
print("runsource incomplete def:", interp.runsource("def mul(a, b):"))
print("runsource complete def:", interp.runsource("def mul(a, b):\n    return a * b\n"))
print("runsource call mul:", interp.runsource("res = mul(6, 7)"))
print("locals res:", interp.locals["res"])
print("no error output:", repr(interp.get_output()))

# Expression output goes through sys.displayhook / sys.stdout
orig_stdout = sys.stdout
buf_out = io.StringIO()
try:
    sys.stdout = buf_out
    interp.runsource("2 + 3")
finally:
    sys.stdout = orig_stdout
print("expr stdout:", repr(buf_out.getvalue()))

# --- 2. showsyntaxerror & showtraceback ---
interp_err = CapturingInterpreter()
more = interp_err.runsource("x = = 1", filename="<bad_file>")
print("syntax error returns False:", more)
err_text = interp_err.get_output()
print("syntax error contains SyntaxError:", "SyntaxError" in err_text)
print("last_type is SyntaxError:", sys.last_type is SyntaxError)

# Explicit SyntaxError with 4-tuple info gets filename replaced by showsyntaxerror
interp_syn = CapturingInterpreter()
try:
    raise SyntaxError("custom msg", ("old.py", 4, 3, "a = = b\n"))
except SyntaxError:
    interp_syn.showsyntaxerror("stuffed.py")
print("stuffed showsyntaxerror:")
print(interp_syn.get_output(), end="")

# Runtime exception -> showtraceback
interp_rt = CapturingInterpreter()
more_rt = interp_rt.runsource("1 / 0", filename="<divzero>")
print("runtime error returns False:", more_rt)
rt_text = interp_rt.get_output()
print("runtime tb header:", "Traceback (most recent call last):" in rt_text)
print("runtime tb filename:", '<divzero>' in rt_text)
print("runtime tb exc:", "ZeroDivisionError: division by zero" in rt_text)
print("last_type is ZDE:", sys.last_type is ZeroDivisionError)
print("last_exc is last_value:", sys.last_exc is sys.last_value)

# SystemExit must propagate out of runcode / runsource
try:
    interp_rt.runsource("raise SystemExit(7)")
    print("SystemExit NOT raised!")
except SystemExit as exc:
    print("SystemExit propagated:", exc.code)

# Custom sys.excepthook takes precedence over self.write
hook_calls = []
orig_hook = sys.excepthook
try:
    def my_hook(t, v, tb):
        hook_calls.append((t.__name__, str(v)))
    sys.excepthook = my_hook
    interp_hook = CapturingInterpreter()
    interp_hook.runsource("raise ValueError('boom')")
    print("custom hook called:", hook_calls, "write output:", repr(interp_hook.get_output()))
finally:
    sys.excepthook = orig_hook

# Base InteractiveInterpreter.write writes to sys.stderr
orig_stderr = sys.stderr
err_buf = io.StringIO()
try:
    sys.stderr = err_buf
    base_interp = code.InteractiveInterpreter()
    base_interp.write("hello stderr\n")
finally:
    sys.stderr = orig_stderr
print("base write stderr:", repr(err_buf.getvalue()))


# --- 3. InteractiveConsole: push, resetbuffer, multi-line def + call ---
ns_console = {}
console = CapturingConsole(locals=ns_console, filename="<myconsole>")
print("push def header:", console.push("def fact(n):"))
print("buffer len 1:", len(console.buffer))
print("push if:", console.push("    if n <= 1:"))
print("push ret1:", console.push("        return 1"))
print("push ret rec:", console.push("    return n * fact(n - 1)"))
print("push blank terminates:", console.push(""))
print("buffer reset:", console.buffer)
print("push call:", console.push("answer = fact(5)"))
print("fact(5):", ns_console["answer"])

# Syntax error in push resets buffer
print("push open paren:", console.push("x = (1 +"))
print("buffer len before reset:", len(console.buffer))
console.resetbuffer()
print("buffer after resetbuffer:", console.buffer)

print("push bad syntax:", console.push("def"))
print("buffer after syntax error:", console.buffer)
print("console saw SyntaxError:", "SyntaxError" in console.get_output())

# Stateful __future__ annotations inside InteractiveConsole
ns_fut = {}
con_fut = CapturingConsole(locals=ns_fut)
print("push future:", con_fut.push("from __future__ import annotations"))
print("push annotated def 1:", con_fut.push("def greet(x: GhostType) -> OtherGhost:"))
print("push annotated def 2:", con_fut.push("    return x"))
print("push annotated def 3:", con_fut.push(""))
print("future annotations in console:", ns_fut["greet"].__annotations__, ns_fut["greet"](123))


# --- 4. InteractiveConsole.interact() with custom banner, exitmsg, KeyboardInterrupt ---
saved_ps1 = getattr(sys, "ps1", None)
saved_ps2 = getattr(sys, "ps2", None)
had_ps1 = hasattr(sys, "ps1")
had_ps2 = hasattr(sys, "ps2")
try:
    ns_i = {}
    c_int = CapturingConsole(
        locals=ns_i,
        inputs=[
            "a = 21",
            "def dbl(v):",
            "    return v * 2",
            "",
            "b = dbl(a)",
            "incomplete = (",
            KeyboardInterrupt(),
            "c = b + 1",
        ],
    )
    c_int.interact(banner="Welcome!", exitmsg="Goodbye!")
    print("interact locals:", ns_i["a"], ns_i["b"], ns_i["c"])
    print("interact prompts:", c_int.prompts)
    print("interact output:", repr(c_int.get_output()))

    # Empty banner and empty exitmsg suppress both messages
    c_quiet = CapturingConsole(locals={}, inputs=[])
    c_quiet.interact(banner="", exitmsg="")
    print("quiet interact output:", repr(c_quiet.get_output()))

    # Default banner & default exitmsg
    c_def = CapturingConsole(locals={}, inputs=[])
    c_def.interact()
    def_out = c_def.get_output()
    print("default banner starts Python:", def_out.startswith("Python "))
    print("default banner has class:", "(CapturingConsole)\n" in def_out)
    print("default exitmsg:", def_out.endswith("now exiting CapturingConsole...\n"))

    # Top-level code.interact() helper with readfunc & redirected stderr
    lines = iter(["z = 99"])

    def reader(prompt):
        try:
            return next(lines)
        except StopIteration:
            raise EOFError

    ns_top = {}
    top_err = io.StringIO()
    orig_stderr2 = sys.stderr
    try:
        sys.stderr = top_err
        code.interact(banner="TopBanner", readfunc=reader, local=ns_top, exitmsg="TopExit")
    finally:
        sys.stderr = orig_stderr2
    print("code.interact local z:", ns_top["z"])
    print("code.interact stderr:", repr(top_err.getvalue()))
finally:
    if had_ps1:
        sys.ps1 = saved_ps1
    elif hasattr(sys, "ps1"):
        del sys.ps1
    if had_ps2:
        sys.ps2 = saved_ps2
    elif hasattr(sys, "ps2"):
        del sys.ps2

print("code parity ok")
