# Issue #2245: errors raised *inside* exec/eval/compile'd source must report
# the correct internal line number (and a `<string>` <module> traceback frame).
# Previously the exec parse path discarded the lexer's physical-line table via
# `into_tokens()`, so the inner bytecode had no line info and tracebacks pointed
# at line 1 (or the host's line).
#
# The parity harness strips "Traceback"/"File" lines, so the line numbers are
# asserted here via `sys.exc_info()` / `tb_lineno`, which both interpreters
# expose and which is portable across machines (no absolute paths).
import sys


def tb_lines(tb):
    out = []
    while tb is not None:
        out.append((tb.tb_lineno, tb.tb_frame.f_code.co_name))
        tb = tb.tb_next
    return out


# 1. exec'd string: error on physical line 4.
src = "x = 1\n\n\nraise ValueError('boom')\n"
try:
    exec(src)
except ValueError as e:
    print("exec:", type(e).__name__, str(e))
    print("exec lines:", tb_lines(sys.exc_info()[2]))

# 2. eval'd expression: error on line 1.
try:
    eval("1 / 0")
except ZeroDivisionError as e:
    print("eval:", type(e).__name__)
    print("eval lines:", tb_lines(sys.exc_info()[2]))

# 3. compile() then exec(): line number flows through the code object.
code = compile("a = 1\n\n\n\nraise RuntimeError('z')\n", "<string>", "exec")
try:
    exec(code)
except RuntimeError as e:
    print("compile:", type(e).__name__, str(e))
    print("compile lines:", tb_lines(sys.exc_info()[2]))

# 4. exec with an explicit globals dict: line numbers still correct.
try:
    exec("\n\nraise KeyError('k')\n", {})
except KeyError as e:
    print("exec-globals:", type(e).__name__)
    print("exec-globals lines:", tb_lines(sys.exc_info()[2]))

# 5. function defined and called inside exec'd code: inner module frame at the
#    call site line, then the function frame at the raising line.
src2 = "def f():\n    raise IndexError('i')\n\n\nf()\n"
try:
    exec(src2)
except IndexError as e:
    print("exec-func:", type(e).__name__)
    print("exec-func lines:", [ln for (ln, _) in tb_lines(sys.exc_info()[2])])

# 6. A successful exec leaves no stale traceback state behind: a later
#    host-level error reports the host line, not a `<string>` line.
exec("ok = 42\n")
try:
    raise TypeError("host")
except TypeError:
    print("host lines:", [name for (_, name) in tb_lines(sys.exc_info()[2])])

print("done")
