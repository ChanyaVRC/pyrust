import io
import os
import sys
import traceback


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, str(e))


# 1. Module metadata and __all__
print("in sys.modules:", "traceback" in sys.modules)
print("__all__ sorted:", sorted(traceback.__all__))
for name in traceback.__all__:
    assert hasattr(traceback, name), f"missing {name}"
print("format_exc outside:", repr(traceback.format_exc()))


# 2. FrameSummary
fs = traceback.FrameSummary("demo.py", 12, "my_func", line="   x = 1 + 2\n", locals={"x": 3, "s": "hi"})
print("FrameSummary repr:", repr(fs))
print("FrameSummary fields:", fs.filename, fs.lineno, fs.name, repr(fs.line), fs.locals)
print("FrameSummary seq:", len(fs), fs[0], fs[1], fs[2], fs[3], list(fs))
assert fs == ("demo.py", 12, "my_func", "x = 1 + 2")
assert fs == traceback.FrameSummary("demo.py", 12, "my_func", line="x = 1 + 2", locals={"x": 3, "s": "hi"})
assert fs != traceback.FrameSummary("demo.py", 13, "my_func", line="x = 1 + 2")

fs_nolookup = traceback.FrameSummary("nonexistent_file_xyz.py", 99, "f", lookup_line=False)
print("FrameSummary missing file line:", repr(fs_nolookup.line))


# 3. StackSummary.from_list, format, format_frame_summary, recursive cutoff
ss = traceback.StackSummary.from_list([
    ("a.py", 10, "outer", "middle()"),
    traceback.FrameSummary("b.py", 20, "middle", line="inner()", locals={"k": [1, 2]}),
])
fmt_ss = ss.format()
assert len(fmt_ss) == 2
assert fmt_ss[0] == '  File "a.py", line 10, in outer\n    middle()\n'
assert fmt_ss[1] == '  File "b.py", line 20, in middle\n    inner()\n    k = [1, 2]\n'
print("StackSummary.from_list ok:", len(fmt_ss))

# Recursive repetition folding (> 3 identical frames)
rep6 = traceback.StackSummary.from_list([("r.py", 5, "rec", "rec()")] * 6).format()
assert len(rep6) == 4
assert rep6[-1] == "  [Previous line repeated 3 more times]\n"
rep4 = traceback.StackSummary.from_list([("r.py", 5, "rec", "rec()")] * 4).format()
assert len(rep4) == 4
assert rep4[-1] == "  [Previous line repeated 1 more time]\n"
print("recursive folding ok:", rep4[-1].strip(), "|", rep6[-1].strip())


# 4. extract_tb, walk_tb, format_tb, format_list, print_tb, print_list
def tb_inner(val):
    local_inner = val * 2
    raise ValueError(f"boom {local_inner}")


def tb_middle(val):
    return tb_inner(val + 1)


def tb_outer(val):
    return tb_middle(val + 10)


try:
    tb_outer(5)
except ValueError as caught_exc:
    caught_tb = caught_exc.__traceback__

    walked = [(f.f_code.co_name, ln) for f, ln in traceback.walk_tb(caught_tb)]
    print("walk_tb names:", [n for n, _ in walked])

    extracted = traceback.extract_tb(caught_tb)
    assert isinstance(extracted, traceback.StackSummary)
    print("extract_tb names:", [fs.name for fs in extracted])
    print("extract_tb lines:", [fs.line for fs in extracted])
    assert extracted[-1].name == "tb_inner"
    assert "raise ValueError" in extracted[-1].line

    extracted_pos = traceback.extract_tb(caught_tb, limit=2)
    extracted_neg = traceback.extract_tb(caught_tb, limit=-2)
    extracted_zero = traceback.extract_tb(caught_tb, limit=0)
    print("extract_tb limits:", [fs.name for fs in extracted_pos], [fs.name for fs in extracted_neg], len(extracted_zero))

    ss_locals = traceback.StackSummary.extract(traceback.walk_tb(caught_tb), capture_locals=True)
    assert ss_locals[-1].locals["val"] == "16"
    assert ss_locals[-1].locals["local_inner"] == "32"
    print("capture_locals inner:", ss_locals[-1].locals["val"], ss_locals[-1].locals["local_inner"])

    ftb = traceback.format_tb(caught_tb)
    assert ftb == traceback.format_list(extracted)
    buf = io.StringIO()
    traceback.print_tb(caught_tb, file=buf)
    assert buf.getvalue() == "".join(ftb)
    print("print_tb matches format_tb:", True)


# 5. extract_stack, walk_stack, format_stack, print_stack
def stack_inner():
    s_all = traceback.extract_stack()
    s_lim = traceback.extract_stack(limit=2)
    s_explicit = traceback.extract_stack(sys._getframe(1), limit=2)
    caller_f = sys._getframe(1)
    f_stack = traceback.format_stack(caller_f, limit=2)
    buf = io.StringIO()
    traceback.print_stack(caller_f, limit=2, file=buf)
    assert buf.getvalue() == "".join(f_stack)
    return [x.name for x in s_all[-3:]], [x.name for x in s_lim], [x.name for x in s_explicit]


def stack_outer():
    return stack_inner()


st_all, st_lim, st_exp = stack_outer()
print("extract_stack:", st_all, st_lim, st_exp)
assert st_all[-2:] == ["stack_outer", "stack_inner"]
assert st_lim == ["stack_outer", "stack_inner"]
assert st_exp[-1] == "stack_outer"


# 6. format_exception_only (1-arg, 2-arg, None, custom class, SyntaxError, __notes__)
print("only 1-arg:", traceback.format_exception_only(ValueError("bad")))
print("only empty:", traceback.format_exception_only(ValueError()))
print("only 2-arg:", traceback.format_exception_only(ValueError, ValueError("bad")))
print("only None:", traceback.format_exception_only(None))
print("only 2-arg None:", traceback.format_exception_only(None, None))


class CustomErr(Exception):
    pass


print("only custom:", traceback.format_exception_only(CustomErr("hello")))

# SyntaxError with text and offset
se1 = SyntaxError("invalid syntax", ("sample.py", 4, 9, "    x = !\n"))
se1_lines = traceback.format_exception_only(se1)
assert se1_lines == [
    '  File "sample.py", line 4\n',
    '    x = !\n',
    '        ^\n',
    'SyntaxError: invalid syntax\n',
], repr(se1_lines)
print("SyntaxError formatted ok:", len(se1_lines), se1_lines[-1].strip())

# SyntaxError with end_offset span
se2 = SyntaxError("bad span", ("sample.py", 2, 5, "    abcd\n", 2, 9))
se2_lines = traceback.format_exception_only(se2)
assert se2_lines == [
    '  File "sample.py", line 2\n',
    '    abcd\n',
    '    ^^^^\n',
    'SyntaxError: bad span\n',
], repr(se2_lines)
print("SyntaxError span ok:", se2_lines[2].strip())

# SyntaxError with filename only (no lineno)
se3 = SyntaxError("oops", ("onlyfile.py", None, None, None))
print("SyntaxError filename only:", traceback.format_exception_only(se3))

# Notes
ne = KeyError("missing")
ne.add_note("first note")
ne.add_note("line one\nline two")
print("notes:", traceback.format_exception_only(ne))


# 7. format_exception, print_exception, chaining (cause / context / suppress / cycle)
print("bad value/tb:", expect_exc(traceback.format_exception, ValueError("x"), ValueError("x")))
print("non-exc 1-arg:", expect_exc(traceback.format_exception, 123))

# Explicit cause (`raise ... from ...`)
try:
    try:
        raise ValueError("root cause")
    except ValueError as e1:
        raise RuntimeError("wrapper") from e1
except RuntimeError as chained_cause:
    fe_1 = traceback.format_exception(chained_cause)
    fe_3 = traceback.format_exception(type(chained_cause), chained_cause, chained_cause.__traceback__)
    assert fe_1 == fe_3
    joined = "".join(fe_1)
    assert "ValueError: root cause\n" in joined
    assert "The above exception was the direct cause of the following exception:" in joined
    assert "RuntimeError: wrapper\n" in joined

    fe_nochain = "".join(traceback.format_exception(chained_cause, chain=False))
    assert "ValueError: root cause" not in fe_nochain
    assert "RuntimeError: wrapper\n" in fe_nochain

    buf = io.StringIO()
    traceback.print_exception(chained_cause, file=buf)
    assert buf.getvalue() == joined
    print("cause chaining ok:", True)

# Implicit context
try:
    try:
        raise KeyError("orig")
    except KeyError:
        raise RuntimeError("during handling")
except RuntimeError as chained_ctx:
    joined_ctx = "".join(traceback.format_exception(chained_ctx))
    assert "KeyError: 'orig'\n" in joined_ctx
    assert "During handling of the above exception, another exception occurred:" in joined_ctx
    assert "RuntimeError: during handling\n" in joined_ctx
    print("context chaining ok:", True)

# Suppressed context (`raise ... from None`)
try:
    try:
        raise KeyError("hidden")
    except KeyError:
        raise RuntimeError("clean") from None
except RuntimeError as suppressed:
    joined_sup = "".join(traceback.format_exception(suppressed))
    assert "KeyError" not in joined_sup
    assert "RuntimeError: clean\n" in joined_sup
    print("suppress context ok:", True)

# Cycle in __context__ does not loop infinitely
cyc_a = ValueError("a")
cyc_b = RuntimeError("b")
cyc_a.__context__ = cyc_b
cyc_b.__context__ = cyc_a
cyc_out = "".join(traceback.format_exception(cyc_a))
assert "ValueError: a\n" in cyc_out and "RuntimeError: b\n" in cyc_out
print("cycle detection ok:", True)


# 8. format_exc and print_exc
try:
    tb_outer(1)
except ValueError:
    fe_str = traceback.format_exc()
    buf = io.StringIO()
    traceback.print_exc(file=buf)
    assert buf.getvalue() == fe_str
    assert "Traceback (most recent call last):" in fe_str
    assert "ValueError: boom 24\n" in fe_str
    print("format_exc inside ok:", fe_str.splitlines()[-1])


# 9. TracebackException
try:
    raise TypeError("bad type")
except TypeError as te_exc:
    te_exc.add_note("extra context")
    te1 = traceback.TracebackException.from_exception(te_exc)
    te2 = traceback.TracebackException(type(te_exc), te_exc, te_exc.__traceback__)
    assert te1 == te2
    assert te1.exc_type is TypeError
    assert str(te1) == "bad type"
    assert te1.__notes__ == ["extra context"]
    assert list(te1.format_exception_only()) == ["TypeError: bad type\n", "extra context\n"]
    buf = io.StringIO()
    te1.print(file=buf)
    assert buf.getvalue() == "".join(te1.format())
    print("TracebackException ok:", str(te1), te1.__notes__)


# 10. BaseExceptionGroup / ExceptionGroup formatting
eg_flat = ExceptionGroup("top", [ValueError("v1"), TypeError("t2")])
eg_flat_lines = "".join(traceback.format_exception(eg_flat))
expected_flat = (
    "  | ExceptionGroup: top (2 sub-exceptions)\n"
    "  +-+---------------- 1 ----------------\n"
    "    | ValueError: v1\n"
    "    +---------------- 2 ----------------\n"
    "    | TypeError: t2\n"
    "    +------------------------------------\n"
)
assert eg_flat_lines == expected_flat, repr(eg_flat_lines)
print("ExceptionGroup flat ok")

eg_nested = ExceptionGroup(
    "outer",
    [
        KeyError("k"),
        ExceptionGroup("inner", [RuntimeError("r1"), ValueError("r2")]),
    ],
)
eg_nested_lines = "".join(traceback.format_exception(eg_nested))
expected_nested = (
    "  | ExceptionGroup: outer (2 sub-exceptions)\n"
    "  +-+---------------- 1 ----------------\n"
    "    | KeyError: 'k'\n"
    "    +---------------- 2 ----------------\n"
    "    | ExceptionGroup: inner (2 sub-exceptions)\n"
    "    +-+---------------- 1 ----------------\n"
    "      | RuntimeError: r1\n"
    "      +---------------- 2 ----------------\n"
    "      | ValueError: r2\n"
    "      +------------------------------------\n"
)
assert eg_nested_lines == expected_nested, repr(eg_nested_lines)
print("ExceptionGroup nested ok")

# Raised ExceptionGroup with traceback
try:
    raise ExceptionGroup("raised group", [ValueError("sub")])
except ExceptionGroup as reg:
    reg_out = "".join(traceback.format_exception(reg))
    assert "  + Exception Group Traceback (most recent call last):\n" in reg_out
    assert "  | ExceptionGroup: raised group (1 sub-exception)\n" in reg_out
    assert "    | ValueError: sub\n" in reg_out
    print("ExceptionGroup raised ok")

# max_group_width and max_group_depth truncation
eg_wide = ExceptionGroup("wide", [ValueError(str(i)) for i in range(5)])
te_wide = traceback.TracebackException.from_exception(eg_wide, max_group_width=2)
wide_out = "".join(te_wide.format())
assert "and 3 more exceptions\n" in wide_out
print("ExceptionGroup max_group_width ok")

eg_deep = ExceptionGroup("d1", [ExceptionGroup("d2", [ExceptionGroup("d3", [ValueError("leaf")])])])
te_deep = traceback.TracebackException.from_exception(eg_deep, max_group_depth=1)
deep_out = "".join(te_deep.format())
assert "... (max_group_depth is 1)\n" in deep_out
print("ExceptionGroup max_group_depth ok")


# 11. clear_frames
def clear_inner():
    secret = "clear_me"
    raise RuntimeError(secret)


try:
    clear_inner()
except RuntimeError as ce:
    tb = ce.__traceback__
    assert tb.tb_next is not None
    assert "secret" in tb.tb_next.tb_frame.f_locals
    traceback.clear_frames(tb)
    assert "secret" not in tb.tb_next.tb_frame.f_locals
    print("clear_frames ok")
