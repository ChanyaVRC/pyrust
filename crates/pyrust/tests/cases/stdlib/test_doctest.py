# CPython 3.12 parity tests for `doctest` (issue #3222).
import doctest
import io
import os
import tempfile
import types
import unittest

# ---------------------------------------------------------------------------
# 1. Module surface, option flags, and register_optionflag
# ---------------------------------------------------------------------------
print("__all__:", doctest.__all__)
print(
    "flags:",
    doctest.DONT_ACCEPT_TRUE_FOR_1,
    doctest.DONT_ACCEPT_BLANKLINE,
    doctest.NORMALIZE_WHITESPACE,
    doctest.ELLIPSIS,
    doctest.SKIP,
    doctest.IGNORE_EXCEPTION_DETAIL,
    doctest.COMPARISON_FLAGS,
    doctest.REPORT_UDIFF,
    doctest.REPORT_CDIFF,
    doctest.REPORT_NDIFF,
    doctest.REPORT_ONLY_FIRST_FAILURE,
    doctest.FAIL_FAST,
    doctest.REPORTING_FLAGS,
)
print("markers:", repr(doctest.BLANKLINE_MARKER), repr(doctest.ELLIPSIS_MARKER))
print("TestResults fields:", doctest.TestResults._fields)

# register_optionflag is idempotent for existing names and allocates next bit for new names
print("register existing:", doctest.register_optionflag("ELLIPSIS") == doctest.ELLIPSIS)
custom_flag = doctest.register_optionflag("PYRUST_CUSTOM_FLAG_TEST")
print("register new power of 2:", custom_flag > 0 and (custom_flag & (custom_flag - 1)) == 0)
print("register new idempotent:", doctest.register_optionflag("PYRUST_CUSTOM_FLAG_TEST") == custom_flag)

# ---------------------------------------------------------------------------
# 2. Example, DocTest, and DocTestParser
# ---------------------------------------------------------------------------
parser = doctest.DocTestParser()
sample_doc = """
Intro text before examples.

    >>> x = 1 + 2
    >>> x
    3
    >>> for i in range(2):
    ...     print(i)
    0
    1
    >>> print("a\\n\\nb")
    a
    <BLANKLINE>
    b
    >>> 1 / 0  # doctest: +IGNORE_EXCEPTION_DETAIL
    Traceback (most recent call last):
        ...
    ZeroDivisionError: ignored detail
    >>> skipped_call()  # doctest: +SKIP
    never runs

Trailing prose.
"""

parsed = parser.parse(sample_doc, "sample")
examples = parser.get_examples(sample_doc, "sample")
print("parsed count:", len(parsed), "examples count:", len(examples))
for idx, ex in enumerate(examples):
    print(
        f"ex[{idx}]:",
        repr(ex.source),
        repr(ex.want),
        repr(ex.exc_msg),
        ex.lineno,
        ex.indent,
        sorted(ex.options.items()),
    )

dt = parser.get_doctest(sample_doc, {"extra": 1}, "sample_test", "<string>", 10)
print("DocTest attrs:", dt.name, dt.filename, dt.lineno, len(dt.examples), dt.globs["extra"])

# Invalid directive raises ValueError
try:
    parser.get_examples(">>> 1 # doctest: +NO_SUCH_OPTION\n1\n")
    print("bad option: UNEXPECTED")
except ValueError as e:
    print("bad option ValueError:", "has an invalid option" in str(e))

# ---------------------------------------------------------------------------
# 3. OutputChecker (exact, TRUE_FOR_1, BLANKLINE, NORMALIZE_WHITESPACE, ELLIPSIS,
#    IGNORE_EXCEPTION_DETAIL, and diff reporting)
# ---------------------------------------------------------------------------
checker = doctest.OutputChecker()
print("exact match:", checker.check_output("hello\n", "hello\n", 0))
print("exact mismatch:", checker.check_output("hello\n", "world\n", 0))
print(
    "true for 1:",
    checker.check_output("1\n", "True\n", 0),
    checker.check_output("1\n", "True\n", doctest.DONT_ACCEPT_TRUE_FOR_1),
    checker.check_output("0\n", "False\n", 0),
    checker.check_output("0\n", "False\n", doctest.DONT_ACCEPT_TRUE_FOR_1),
)
print(
    "blankline:",
    checker.check_output("a\n<BLANKLINE>\nb\n", "a\n\nb\n", 0),
    checker.check_output("a\n<BLANKLINE>\nb\n", "a\n\nb\n", doctest.DONT_ACCEPT_BLANKLINE),
)
print(
    "normalize ws:",
    checker.check_output("a   b\nc\n", "a b c\n", 0),
    checker.check_output("a   b\nc\n", "a b c\n", doctest.NORMALIZE_WHITESPACE),
)
print(
    "ellipsis:",
    checker.check_output("hello ... world\n", "hello wonderful world\n", 0),
    checker.check_output("hello ... world\n", "hello wonderful world\n", doctest.ELLIPSIS),
    checker.check_output("...end\n", "start middle end\n", doctest.ELLIPSIS),
    checker.check_output("start...\n", "start middle end\n", doctest.ELLIPSIS),
    checker.check_output("a...b...c\n", "a123b456c\n", doctest.ELLIPSIS),
    checker.check_output("a...b...c\n", "a123x456c\n", doctest.ELLIPSIS),
)
# IGNORE_EXCEPTION_DETAIL tested via DocTestRunner
ied_runner = doctest.DocTestRunner(optionflags=doctest.IGNORE_EXCEPTION_DETAIL)
ied_out = []
ied_pass = ied_runner.run(
    parser.get_doctest(
        ">>> raise ValueError('actual detail')\n"
        "Traceback (most recent call last):\n"
        "    ...\n"
        "ValueError: expected detail\n",
        {},
        "ied_pass",
        "<str>",
        0,
    ),
    out=ied_out.append,
)
ied_fail = ied_runner.run(
    parser.get_doctest(
        ">>> raise TypeError('actual detail')\n"
        "Traceback (most recent call last):\n"
        "    ...\n"
        "ValueError: expected detail\n",
        {},
        "ied_fail",
        "<str>",
        0,
    ),
    out=ied_out.append,
)
print("ignore exc detail runner:", ied_pass, ied_fail)

diff_ex = doctest.Example("foo()\n", "line1\nline2\nline3\n")
for label, flag in [
    ("plain", 0),
    ("udiff", doctest.REPORT_UDIFF),
    ("cdiff", doctest.REPORT_CDIFF),
    ("ndiff", doctest.REPORT_NDIFF),
]:
    diff_out = checker.output_difference(diff_ex, "line1\nLINE2\nline3\n", flag)
    print(f"diff {label}:", repr(diff_out))

# ---------------------------------------------------------------------------
# 4. DocTestRunner (passing, failing, flags, FAIL_FAST, REPORT_ONLY_FIRST_FAILURE,
#    summarize, merge)
# ---------------------------------------------------------------------------
out_buf = []
runner = doctest.DocTestRunner(verbose=False)
res_pass = runner.run(dt, out=out_buf.append)
print("runner pass:", res_pass, len(out_buf), res_pass == doctest.TestResults(0, 6))

fail_doc = """
>>> 1 + 1
99
>>> 2 + 2
88
>>> 1 / 0
0
"""
fail_dt = parser.get_doctest(fail_doc, {}, "fail_test", "fake.py", 0)

# Default runner runs all 3 examples
fail_out = []
r_all = doctest.DocTestRunner()
res_all = r_all.run(fail_dt, out=fail_out.append)
print("runner fail all:", res_all, "Expected:" in "".join(fail_out), "ZeroDivisionError" in "".join(fail_out))

# FAIL_FAST stops after first failure
ff_out = []
r_ff = doctest.DocTestRunner(optionflags=doctest.FAIL_FAST)
res_ff = r_ff.run(fail_dt, out=ff_out.append)
print("runner FAIL_FAST:", res_ff)

# REPORT_ONLY_FIRST_FAILURE runs all 3 but reports only the first
roff_out = []
r_roff = doctest.DocTestRunner(optionflags=doctest.REPORT_ONLY_FIRST_FAILURE)
res_roff = r_roff.run(fail_dt, out=roff_out.append)
roff_text = "".join(roff_out)
print("runner REPORT_ONLY_FIRST_FAILURE:", res_roff, roff_text.count("Failed example:"))

# clear_globs=False preserves namespace mutations on dt.globs
dt_globs = parser.get_doctest(">>> kept = 123\n>>> kept\n123\n", {}, "g", "<str>", 0)
doctest.DocTestRunner().run(dt_globs, clear_globs=False)
print("clear_globs=False:", dt_globs.globs.get("kept"))

# summarize() & merge()
r_a = doctest.DocTestRunner()
r_b = doctest.DocTestRunner()
r_a.run(parser.get_doctest(">>> 1\n1\n", {}, "a", "<str>", 0))
r_b.run(parser.get_doctest(">>> 2\n2\n>>> 3\n3\n", {}, "b", "<str>", 0))
r_a.merge(r_b)
sum_buf = io.StringIO()
import sys
old_stdout = sys.stdout
sys.stdout = sum_buf
try:
    summary = r_a.summarize(verbose=False)
finally:
    sys.stdout = old_stdout
print("summarize merged:", summary, repr(sum_buf.getvalue()))

# ---------------------------------------------------------------------------
# 5. DebugRunner, DocTestFailure, UnexpectedException
# ---------------------------------------------------------------------------
dbg = doctest.DebugRunner()
try:
    dbg.run(parser.get_doctest(">>> 1 + 1\n3\n", {}, "dbg_fail", "<str>", 0))
    print("DebugRunner mismatch: UNEXPECTED")
except doctest.DocTestFailure as err:
    print("DebugRunner DocTestFailure:", err.test.name, repr(err.example.source), repr(err.got))

try:
    dbg.run(parser.get_doctest(">>> 1 / 0\n0\n", {}, "dbg_exc", "<str>", 0))
    print("DebugRunner exc: UNEXPECTED")
except doctest.UnexpectedException as err:
    print("DebugRunner UnexpectedException:", err.test.name, err.exc_info[0].__name__)

# ---------------------------------------------------------------------------
# 6. DocTestFinder & testmod
# ---------------------------------------------------------------------------
synth_mod = types.ModuleType("synth_doctest_mod")
synth_mod.__doc__ = """
Module docstring example:
>>> add(2, 3)
5
"""


exec(
    '''
def add(a, b):
    """Add two numbers.

    >>> add(10, 20)
    30
    """
    return a + b


class Box:
    """Simple box.

    >>> Box(7).value
    7
    """

    def __init__(self, value):
        self.value = value

    def doubled(self):
        """Return doubled value.

        >>> Box(4).doubled()
        8
        """
        return self.value * 2
''',
    synth_mod.__dict__,
)
_add = synth_mod.add
synth_mod.__test__ = {
    "extra_case": ">>> add(1, 1)\n2\n",
    "empty_case": "",
}

finder = doctest.DocTestFinder()
found = finder.find(synth_mod)
print("finder names:", [t.name for t in found])

tm_res = doctest.testmod(synth_mod, report=False)
print("testmod synth:", tm_res)

# ---------------------------------------------------------------------------
# 7. run_docstring_examples, script_from_examples, testsource
# ---------------------------------------------------------------------------
buf_rde = io.StringIO()
old_stdout = sys.stdout
sys.stdout = buf_rde
try:
    doctest.run_docstring_examples(_add, {"add": _add}, verbose=False, name="add_ex")
finally:
    sys.stdout = old_stdout
print("run_docstring_examples pass:", repr(buf_rde.getvalue()))

script = doctest.script_from_examples("""
Demo:
>>> a = 2
>>> a * 3
6
""")
print("script_from_examples:", repr(script))

ts = doctest.testsource(synth_mod, "synth_doctest_mod.add")
print("testsource:", repr(ts))

# ---------------------------------------------------------------------------
# 8. testfile (including CRLF normalization)
# ---------------------------------------------------------------------------
with tempfile.TemporaryDirectory() as tmpdir:
    pass_file = os.path.join(tmpdir, "pass_doc.txt")
    with open(pass_file, "wb") as fp:
        fp.write(
            b"Test file with CRLF.\r\n\r\n"
            b"    >>> x = [1, 2, 3]\r\n"
            b"    >>> sum(x)\r\n"
            b"    6\r\n"
            b"    >>> print('a\\n\\nb')\r\n"
            b"    a\r\n"
            b"    <BLANKLINE>\r\n"
            b"    b\r\n"
        )
    tf_res = doctest.testfile(pass_file, module_relative=False, report=False)
    print("testfile pass:", tf_res)

    fail_file = os.path.join(tmpdir, "fail_doc.txt")
    with open(fail_file, "w", encoding="utf-8") as fp:
        fp.write(">>> 1 + 1\n999\n")
    tf_buf = io.StringIO()
    old_stdout = sys.stdout
    sys.stdout = tf_buf
    try:
        tf_fail = doctest.testfile(fail_file, module_relative=False, report=False)
    finally:
        sys.stdout = old_stdout
    print("testfile fail:", tf_fail, "Expected:\n    999" in tf_buf.getvalue())

    # DocFileSuite via unittest runner
    file_suite = doctest.DocFileSuite(pass_file, module_relative=False)
    file_runner_out = io.StringIO()
    file_unit_res = unittest.TextTestRunner(stream=file_runner_out, verbosity=0).run(file_suite)
    print("DocFileSuite:", file_unit_res.wasSuccessful(), file_unit_res.testsRun)

# ---------------------------------------------------------------------------
# 9. DocTestSuite & set_unittest_reportflags
# ---------------------------------------------------------------------------
prev_flags = doctest.set_unittest_reportflags(doctest.REPORT_UDIFF)
restored_flags = doctest.set_unittest_reportflags(prev_flags)
print("set_unittest_reportflags:", prev_flags, restored_flags == doctest.REPORT_UDIFF)

mod_suite = doctest.DocTestSuite(synth_mod)
suite_out = io.StringIO()
suite_res = unittest.TextTestRunner(stream=suite_out, verbosity=0).run(mod_suite)
print("DocTestSuite:", suite_res.wasSuccessful(), suite_res.testsRun)

print("ALL doctest PARITY TESTS PASSED")
