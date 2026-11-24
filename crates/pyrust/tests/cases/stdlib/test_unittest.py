# CPython 3.12 parity tests for `unittest` module (issue #3153).
import asyncio
import io
import re
import sys
import types
import unittest
import warnings


def normalize_runner_output(text):
    # Replace elapsed timing (e.g. "in 0.001s") with a stable placeholder so
    # runner output can be compared byte-for-byte against CPython 3.12.
    return re.sub(r"in \d+\.\d{3}s", "in X.XXXs", text)


# ---------------------------------------------------------------------------
# 1. Module surface & submodules
# ---------------------------------------------------------------------------
CORE_ALL = [
    "TestResult",
    "TestCase",
    "IsolatedAsyncioTestCase",
    "TestSuite",
    "TextTestRunner",
    "TestLoader",
    "FunctionTestCase",
    "main",
    "defaultTestLoader",
    "SkipTest",
    "skip",
    "skipIf",
    "skipUnless",
    "expectedFailure",
    "TextTestResult",
    "installHandler",
    "registerResult",
    "removeResult",
    "removeHandler",
    "addModuleCleanup",
    "doModuleCleanups",
    "enterModuleContext",
]
print("all has core:", all(name in unittest.__all__ for name in CORE_ALL))
print(
    "modules:",
    unittest.TestCase.__module__,
    unittest.TestSuite.__module__,
    unittest.TestLoader.__module__,
    unittest.TestResult.__module__,
    unittest.TextTestResult.__module__,
    unittest.TextTestRunner.__module__,
    unittest.TestProgram.__module__,
    unittest.FunctionTestCase.__module__,
    unittest.SkipTest.__module__,
    unittest.IsolatedAsyncioTestCase.__module__,
)

import unittest.case
import unittest.loader
from unittest.main import TestProgram as _MainTestProgram
import unittest.result
import unittest.runner
import unittest.signals
import unittest.suite
import unittest.util
import unittest.async_case

print(
    "submodule identities:",
    unittest.case.TestCase is unittest.TestCase,
    unittest.suite.TestSuite is unittest.TestSuite,
    unittest.loader.TestLoader is unittest.TestLoader,
    unittest.result.TestResult is unittest.TestResult,
    unittest.runner.TextTestRunner is unittest.TextTestRunner,
    _MainTestProgram is unittest.TestProgram,
    sys.modules["unittest.main"].TestProgram is unittest.TestProgram,
    unittest.async_case.IsolatedAsyncioTestCase is unittest.IsolatedAsyncioTestCase,
    unittest.main is unittest.TestProgram,
)


# ---------------------------------------------------------------------------
# 2. TestCase assertions (passing + exact failure messages)
# ---------------------------------------------------------------------------
tc = unittest.TestCase()

# Passing assertions
tc.assertEqual(1 + 1, 2)
tc.assertNotEqual(1, 2)
tc.assertTrue(True)
tc.assertTrue([1])
tc.assertFalse(False)
tc.assertFalse([])
obj = object()
tc.assertIs(obj, obj)
tc.assertIsNot(object(), object())
tc.assertIsNone(None)
tc.assertIsNotNone(0)
tc.assertIn(2, [1, 2, 3])
tc.assertNotIn(4, [1, 2, 3])
tc.assertIsInstance("abc", str)
tc.assertIsInstance(42, (int, float))
tc.assertNotIsInstance("abc", int)
tc.assertAlmostEqual(1.0, 1.00000001)
tc.assertAlmostEqual(1.0, 1.04, places=1)
tc.assertAlmostEqual(10, 12, delta=2)
tc.assertNotAlmostEqual(1.0, 1.01)
tc.assertNotAlmostEqual(10, 13, delta=2)
tc.assertGreater(5, 3)
tc.assertGreaterEqual(5, 5)
tc.assertGreaterEqual(6, 5)
tc.assertLess(3, 5)
tc.assertLessEqual(5, 5)
tc.assertLessEqual(4, 5)
tc.assertRegex("hello world", r"wor\w+")
tc.assertRegex("hello world", re.compile(r"HELLO", re.IGNORECASE))
tc.assertNotRegex("hello world", r"\d+")
tc.assertCountEqual([1, 2, 2, 3], [3, 2, 1, 2])
tc.assertCountEqual([{"a": 1}, {"b": 2}], [{"b": 2}, {"a": 1}])
tc.assertMultiLineEqual("line1\nline2\n", "line1\nline2\n")
tc.assertSequenceEqual([1, 2, 3], (1, 2, 3))
tc.assertListEqual([1, 2], [1, 2])
tc.assertTupleEqual((1, 2), (1, 2))
tc.assertSetEqual({1, 2}, frozenset([2, 1]))
tc.assertDictEqual({"a": 1, "b": 2}, {"b": 2, "a": 1})
print("passing assertions ok")


def capture_fail(fn):
    try:
        fn()
    except AssertionError as e:
        return str(e)
    return "<NO FAILURE>"


print("eq fail:", capture_fail(lambda: tc.assertEqual(1, 2)))
print("eq fail msg:", capture_fail(lambda: tc.assertEqual(1, 2, "custom")))
tc_short = unittest.TestCase()
tc_short.longMessage = False
print("eq fail short:", capture_fail(lambda: tc_short.assertEqual(1, 2, "only this")))
print("ne fail:", capture_fail(lambda: tc.assertNotEqual(3, 3)))
print("true fail:", capture_fail(lambda: tc.assertTrue(0)))
print("false fail:", capture_fail(lambda: tc.assertFalse(1)))
print("is fail:", capture_fail(lambda: tc.assertIs(1, 2)))
print("isnot fail:", capture_fail(lambda: tc.assertIsNot(None, None)))
print("isnone fail:", capture_fail(lambda: tc.assertIsNone(5)))
print("isnotnone fail:", capture_fail(lambda: tc.assertIsNotNone(None)))
print("in fail:", capture_fail(lambda: tc.assertIn(9, [1, 2])))
print("notin fail:", capture_fail(lambda: tc.assertNotIn(2, [1, 2])))
print("isinstance fail:", capture_fail(lambda: tc.assertIsInstance(1, str)))
print("notisinstance fail:", capture_fail(lambda: tc.assertNotIsInstance("x", str)))
print("almost fail places:", capture_fail(lambda: tc.assertAlmostEqual(1.0, 2.0, places=2)))
print("almost fail delta:", capture_fail(lambda: tc.assertAlmostEqual(1, 5, delta=2)))
print("notalmost fail places:", capture_fail(lambda: tc.assertNotAlmostEqual(1.0, 1.001, places=2)))
print("notalmost fail delta:", capture_fail(lambda: tc.assertNotAlmostEqual(1, 2, delta=2)))
print("gt fail:", capture_fail(lambda: tc.assertGreater(1, 2)))
print("ge fail:", capture_fail(lambda: tc.assertGreaterEqual(1, 2)))
print("lt fail:", capture_fail(lambda: tc.assertLess(2, 1)))
print("le fail:", capture_fail(lambda: tc.assertLessEqual(2, 1)))
print("regex fail:", capture_fail(lambda: tc.assertRegex("abc", r"\d+")))
print("notregex fail:", capture_fail(lambda: tc.assertNotRegex("abc123def", r"\d+")))
print("count fail hashable:", capture_fail(lambda: tc.assertCountEqual([1, 1, 2], [1, 2, 2])))
print("count fail unhashable:", capture_fail(lambda: tc.assertCountEqual([{"a": 1}], [{"a": 2}])))
print("set fail:", capture_fail(lambda: tc.assertSetEqual({1, 2}, {2, 3})))
print("list fail:", capture_fail(lambda: tc.assertListEqual([1, 2, 3], [1, 9, 3, 4])))
print("tuple fail:", capture_fail(lambda: tc.assertTupleEqual((1, 2), (1, 3))))
print("dict fail:", capture_fail(lambda: tc.assertDictEqual({"a": 1, "b": 2}, {"a": 1, "b": 3})))
print("multiline fail:", capture_fail(lambda: tc.assertMultiLineEqual("a\nb\nc\n", "a\nB\nc\n")))

# maxDiff truncation
tc_small_diff = unittest.TestCase()
tc_small_diff.maxDiff = 5
print("maxDiff truncated:", capture_fail(lambda: tc_small_diff.assertMultiLineEqual("alpha\n", "beta\n")))
tc_small_diff.maxDiff = None
print("maxDiff None:", "- alpha\n" in capture_fail(lambda: tc_small_diff.assertMultiLineEqual("alpha\n", "beta\n")))

# both places and delta -> TypeError
for fn in (
    lambda: tc.assertAlmostEqual(1.0, 2.0, places=2, delta=0.5),
    lambda: tc.assertNotAlmostEqual(1.0, 2.0, places=2, delta=0.5),
):
    try:
        fn()
    except TypeError as e:
        print("places+delta TypeError:", str(e))


# addTypeEqualityFunc
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def __eq__(self, other):
        return isinstance(other, Point) and self.x == other.x and self.y == other.y


def assert_point_equal(p1, p2, msg=None):
    if p1 != p2:
        raise AssertionError(f"Point mismatch: ({p1.x},{p1.y}) vs ({p2.x},{p2.y})")


tc_pt = unittest.TestCase()
tc_pt.addTypeEqualityFunc(Point, assert_point_equal)
tc_pt.assertEqual(Point(1, 2), Point(1, 2))
print("custom type eq fail:", capture_fail(lambda: tc_pt.assertEqual(Point(1, 2), Point(3, 4))))


# Custom failureException
class CustomFail(Exception):
    pass


class CustomFailCase(unittest.TestCase):
    failureException = CustomFail

    def test_custom(self):
        self.fail("boom")


res_custom = unittest.TestResult()
CustomFailCase("test_custom").run(res_custom)
print("custom failureException:", len(res_custom.failures), len(res_custom.errors), res_custom.wasSuccessful())


# ---------------------------------------------------------------------------
# 3. assertRaises & assertRaisesRegex
# ---------------------------------------------------------------------------
def boom(msg="bad"):
    raise ValueError(msg)


tc.assertRaises(ValueError, boom, "oops")
tc.assertRaises((TypeError, ValueError), boom, msg="kw")
tc.assertRaisesRegex(ValueError, r"oo\w+", boom, "oops!")

with tc.assertRaises(ValueError) as cm:
    raise ValueError("cm error")
print("assertRaises cm:", type(cm.exception).__name__, str(cm.exception))

with tc.assertRaisesRegex(ValueError, r"code=\d+") as cm2:
    raise ValueError("failed with code=42")
print("assertRaisesRegex cm:", str(cm2.exception))

print("raises not raised cm:", capture_fail(lambda: tc.assertRaises(ValueError).__exit__(None, None, None)))
print("raises not raised fn:", capture_fail(lambda: tc.assertRaises(ValueError, lambda: 42)))


def call_raises_regex_mismatch():
    with tc.assertRaisesRegex(ValueError, r"^\d+$"):
        raise ValueError("abc")


print("raisesRegex mismatch:", capture_fail(call_raises_regex_mismatch))


def call_raises_wrong_exc():
    with tc.assertRaises(KeyError):
        raise ValueError("wrong")


try:
    call_raises_wrong_exc()
except ValueError as e:
    print("raises wrong exc propagated:", str(e))

try:
    tc.assertRaises(int)
except TypeError as e:
    print("raises bad type:", str(e))

try:
    tc.assertRaises(ValueError, unknown=1)
except TypeError as e:
    print("raises bad kwarg:", str(e))


# ---------------------------------------------------------------------------
# 4. assertWarns & assertWarnsRegex
# ---------------------------------------------------------------------------
def emit_warn(msg="careful", cat=UserWarning):
    warnings.warn(msg, cat)


tc.assertWarns(UserWarning, emit_warn, "hello")
tc.assertWarnsRegex(UserWarning, r"hel+o", emit_warn, "helllo")

with tc.assertWarns(DeprecationWarning) as wcm:
    warnings.warn("old api", DeprecationWarning)
print("assertWarns cm:", type(wcm.warning).__name__, str(wcm.warning), len(wcm.warnings))

with tc.assertWarns((UserWarning, DeprecationWarning)) as wcm_tuple:
    warnings.warn("from tuple", DeprecationWarning)
print("assertWarns tuple:", type(wcm_tuple.warning).__name__, str(wcm_tuple.warning))

with tc.assertWarnsRegex(UserWarning, r"val=\d+") as wcm2:
    warnings.warn("ignore me", UserWarning)
    warnings.warn("got val=99", UserWarning)
print("assertWarnsRegex cm:", str(wcm2.warning), len(wcm2.warnings))


def call_warns_not_triggered():
    with tc.assertWarns(UserWarning):
        pass


print("warns not triggered:", capture_fail(call_warns_not_triggered))


def call_warns_regex_mismatch():
    with tc.assertWarnsRegex(UserWarning, r"^\d+$"):
        warnings.warn("not digits", UserWarning)


print("warnsRegex mismatch:", capture_fail(call_warns_regex_mismatch))

try:
    tc.assertWarns(ValueError)
except TypeError as e:
    print("warns bad type:", str(e))


# ---------------------------------------------------------------------------
# 5. subTest (including nested & failfast)
# ---------------------------------------------------------------------------
class SubTestCase(unittest.TestCase):
    def test_even(self):
        for i in range(4):
            with self.subTest(i=i):
                self.assertEqual(i % 2, 0)

    def test_nested(self):
        with self.subTest("outer", a=1):
            with self.subTest("inner", b=2):
                self.assertEqual(1, 2)


res_sub = unittest.TestResult()
SubTestCase("test_even").run(res_sub)
print(
    "subtest even:",
    res_sub.testsRun,
    len(res_sub.failures),
    [str(t) for t, _ in res_sub.failures],
    res_sub.wasSuccessful(),
)

res_nested = unittest.TestResult()
SubTestCase("test_nested").run(res_nested)
print(
    "subtest nested:",
    len(res_nested.failures),
    [str(t) for t, _ in res_nested.failures],
)

res_ff = unittest.TestResult()
res_ff.failfast = True
SubTestCase("test_even").run(res_ff)
print("subtest failfast:", len(res_ff.failures), res_ff.shouldStop)


# ---------------------------------------------------------------------------
# 6. Lifecycle: setUp/tearDown, setUpClass/tearDownClass, addCleanup,
#    addClassCleanup, enterContext, enterClassContext, module fixtures
# ---------------------------------------------------------------------------
events = []


class DummyCM:
    def __init__(self, name):
        self.name = name

    def __enter__(self):
        events.append(f"enter:{self.name}")
        return f"val:{self.name}"

    def __exit__(self, exc_type, exc_val, tb):
        events.append(f"exit:{self.name}")
        return False


class LifecycleTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        events.append("setUpClass")
        cls.addClassCleanup(events.append, "classCleanup1")
        cls.cls_val = cls.enterClassContext(DummyCM("cls"))
        cls.addClassCleanup(events.append, "classCleanup2")

    @classmethod
    def tearDownClass(cls):
        events.append("tearDownClass")

    def setUp(self):
        events.append(f"setUp:{self._testMethodName}")
        self.addCleanup(events.append, f"cleanup1:{self._testMethodName}")
        self.inst_val = self.enterContext(DummyCM(self._testMethodName))
        self.addCleanup(events.append, f"cleanup2:{self._testMethodName}")

    def tearDown(self):
        events.append(f"tearDown:{self._testMethodName}")

    def test_a(self):
        """First test docstring.
        Second line ignored.
        """
        events.append(f"run:test_a:{self.cls_val}:{self.inst_val}")

    def test_b(self):
        events.append("run:test_b")


suite_lc = unittest.defaultTestLoader.loadTestsFromTestCase(LifecycleTest)
res_lc = unittest.TestResult()
suite_lc.run(res_lc)
print("lifecycle events:", events)
print("lifecycle ok:", res_lc.testsRun, res_lc.wasSuccessful())

# Verify id(), shortDescription(), str(), repr(), eq/hash
t_a = LifecycleTest("test_a")
t_a2 = LifecycleTest("test_a")
t_b = LifecycleTest("test_b")
print("id:", t_a.id())
print("shortDescription:", t_a.shortDescription(), t_b.shortDescription())
print("str:", str(t_a))
print("repr:", repr(t_a))
print("eq/hash:", t_a == t_a2, t_a != t_b, hash(t_a) == hash(t_a2))

# setUp failure still runs addCleanup, skips tearDown
setup_fail_events = []


class SetUpFailCase(unittest.TestCase):
    def setUp(self):
        setup_fail_events.append("setUp")
        self.addCleanup(setup_fail_events.append, "cleanup")
        raise RuntimeError("setup boom")

    def tearDown(self):
        setup_fail_events.append("tearDown")

    def test_never(self):
        setup_fail_events.append("test")


res_sf = unittest.TestResult()
SetUpFailCase("test_never").run(res_sf)
print("setUp fail events:", setup_fail_events, len(res_sf.errors), res_sf.wasSuccessful())

# enterContext with non-CM raises TypeError
try:
    tc.enterContext(123)
except TypeError as e:
    print("enterContext non-CM:", "context manager protocol" in str(e))


# Module fixtures (setUpModule / tearDownModule / addModuleCleanup / enterModuleContext)
mod_events = []
fake_mod = types.ModuleType("_fake_unittest_mod")
fake_mod.setUpModule = lambda: (
    mod_events.append("setUpModule"),
    unittest.addModuleCleanup(mod_events.append, "modCleanup1"),
    unittest.enterModuleContext(DummyCM("mod")),
)
fake_mod.tearDownModule = lambda: mod_events.append("tearDownModule")
sys.modules["_fake_unittest_mod"] = fake_mod


class ModFixtureTest(unittest.TestCase):
    def test_one(self):
        mod_events.append("test_one")


ModFixtureTest.__module__ = "_fake_unittest_mod"
events.clear()
res_mod = unittest.TestResult()
unittest.TestSuite([ModFixtureTest("test_one")]).run(res_mod)
del sys.modules["_fake_unittest_mod"]
print("module fixture events:", mod_events, events, res_mod.wasSuccessful())


# ---------------------------------------------------------------------------
# 7. Skipping & expectedFailure
# ---------------------------------------------------------------------------
class SkipAndExpectedTest(unittest.TestCase):
    @unittest.skip("always skipped")
    def test_skip_reason(self):
        self.fail("should not run")

    @unittest.skip
    def test_skip_bare(self):
        self.fail("should not run")

    @unittest.skipIf(True, "cond true")
    def test_skip_if_true(self):
        self.fail("should not run")

    @unittest.skipIf(False, "cond false")
    def test_skip_if_false(self):
        self.assertEqual(1, 1)

    @unittest.skipUnless(False, "unless false")
    def test_skip_unless_false(self):
        self.fail("should not run")

    @unittest.skipUnless(True, "unless true")
    def test_skip_unless_true(self):
        self.assertEqual(2, 2)

    def test_runtime_skip(self):
        self.skipTest("skipped inside test")

    @unittest.expectedFailure
    def test_expected_fail(self):
        self.assertEqual(1, 2)

    @unittest.expectedFailure
    def test_unexpected_success(self):
        self.assertEqual(1, 1)


@unittest.skip("whole class skipped")
class SkippedClassTest(unittest.TestCase):
    def test_a(self):
        self.fail("should not run")

    def test_b(self):
        self.fail("should not run")


res_skip = unittest.TestResult()
suite_skip = unittest.TestSuite(
    [
        unittest.defaultTestLoader.loadTestsFromTestCase(SkipAndExpectedTest),
        unittest.defaultTestLoader.loadTestsFromTestCase(SkippedClassTest),
    ]
)
suite_skip.run(res_skip)
print(
    "skip/expected counts:",
    res_skip.testsRun,
    len(res_skip.skipped),
    [r for _, r in res_skip.skipped],
    len(res_skip.expectedFailures),
    len(res_skip.unexpectedSuccesses),
    res_skip.wasSuccessful(),
)

# Expected failure without unexpected success IS successful
res_ef_only = unittest.TestResult()
SkipAndExpectedTest("test_expected_fail").run(res_ef_only)
print("expectedFailure only ok:", res_ef_only.wasSuccessful(), len(res_ef_only.expectedFailures))


# ---------------------------------------------------------------------------
# 8. TestSuite, TestLoader, FunctionTestCase
# ---------------------------------------------------------------------------
func_events = []


def sample_test_fn():
    """Sample function test."""
    func_events.append("body")


ftc = unittest.FunctionTestCase(
    sample_test_fn,
    setUp=lambda: func_events.append("setup"),
    tearDown=lambda: func_events.append("teardown"),
)
print("FunctionTestCase:", ftc.id(), ftc.shortDescription(), str(ftc))
res_ftc = unittest.TestResult()
ftc.run(res_ftc)
print("FunctionTestCase run:", func_events, res_ftc.wasSuccessful())


class LoaderDemo(unittest.TestCase):
    def test_beta(self):
        pass

    def test_alpha(self):
        pass

    def check_gamma(self):
        pass


loader = unittest.TestLoader()
print("getTestCaseNames default:", loader.getTestCaseNames(LoaderDemo))
loader.testMethodPrefix = "check"
print("getTestCaseNames check:", loader.getTestCaseNames(LoaderDemo))
loader.testMethodPrefix = "test"
loader.testNamePatterns = ["*alpha*"]
print("getTestCaseNames pattern:", loader.getTestCaseNames(LoaderDemo))
loader.testNamePatterns = None

# loadTestsFromName & loadTestsFromNames
s_name = loader.loadTestsFromName("LoaderDemo.test_alpha", sys.modules[__name__])
print("loadTestsFromName count:", s_name.countTestCases())
s_names = loader.loadTestsFromNames(
    ["LoaderDemo.test_alpha", "LoaderDemo.test_beta"], sys.modules[__name__]
)
print("loadTestsFromNames count:", s_names.countTestCases())

# loadTestsFromModule with load_tests protocol
custom_mod = types.ModuleType("_custom_loader_mod")
custom_mod.LoaderDemo = LoaderDemo
s_mod = loader.loadTestsFromModule(custom_mod)
print("loadTestsFromModule count:", s_mod.countTestCases())

custom_mod.load_tests = lambda ldr, tests, pattern: unittest.TestSuite(
    [LoaderDemo("test_alpha")]
)
s_mod_hook = loader.loadTestsFromModule(custom_mod)
print("loadTestsFromModule hook count:", s_mod_hook.countTestCases())

# TestSuite errors & repr
s_empty = unittest.TestSuite()
print("empty suite repr:", repr(s_empty), s_empty.countTestCases())
try:
    s_empty.addTest(LoaderDemo)
except TypeError as e:
    print("addTest uninstantiated:", str(e))
try:
    s_empty.addTests("not a suite")
except TypeError as e:
    print("addTests str:", str(e))


# ---------------------------------------------------------------------------
# 9. TextTestRunner & TextTestResult (using io.StringIO)
# ---------------------------------------------------------------------------
class RunnerDemo(unittest.TestCase):
    def test_pass(self):
        """Pass test."""
        self.assertEqual(2 * 3, 6)

    def test_fail(self):
        self.assertEqual(1, 2)

    def test_error(self):
        raise ZeroDivisionError("oops")

    @unittest.skip("not now")
    def test_skip(self):
        pass


buf_dots = io.StringIO()
runner_dots = unittest.TextTestRunner(stream=buf_dots, verbosity=1)
res_dots = runner_dots.run(unittest.defaultTestLoader.loadTestsFromTestCase(RunnerDemo))
out_dots = normalize_runner_output(buf_dots.getvalue())
print("runner dots summary:", res_dots.testsRun, len(res_dots.failures), len(res_dots.errors), len(res_dots.skipped), res_dots.wasSuccessful())
print("runner dots first line:", out_dots.splitlines()[0])
print("runner dots last line:", out_dots.splitlines()[-1])
# Verify unittest internal frames are stripped from failure traceback
fail_tb = res_dots.failures[0][1]
print("fail tb has AssertionError:", "AssertionError: 1 != 2" in fail_tb)
print("fail tb hides assertEqual:", "def assertEqual" not in fail_tb and "raise self.failureException" not in fail_tb)

buf_verbose = io.StringIO()
runner_verbose = unittest.TextTestRunner(stream=buf_verbose, verbosity=2)
suite_ok = unittest.TestSuite([RunnerDemo("test_pass"), RunnerDemo("test_skip")])
res_verbose = runner_verbose.run(suite_ok)
print("runner verbose:\n" + normalize_runner_output(buf_verbose.getvalue()))

# buffer=True captures stdout on passing tests and prints on failing tests
class BufferDemo(unittest.TestCase):
    def test_quiet_pass(self):
        print("hidden stdout")

    def test_loud_fail(self):
        print("captured stdout")
        self.fail("boom")


buf_b = io.StringIO()
res_b = unittest.TextTestRunner(stream=buf_b, verbosity=1, buffer=True).run(
    unittest.defaultTestLoader.loadTestsFromTestCase(BufferDemo)
)
out_b = buf_b.getvalue()
print("buffer hides pass stdout:", "hidden stdout" not in out_b)
print("buffer shows fail stdout:", "captured stdout" in out_b)


# ---------------------------------------------------------------------------
# 10. unittest.main(..., exit=False)
# ---------------------------------------------------------------------------
main_mod = types.ModuleType("_main_demo_mod")


class MainDemoTest(unittest.TestCase):
    def test_one(self):
        self.assertEqual(10, 10)

    def test_two(self):
        self.assertTrue(True)


main_mod.MainDemoTest = MainDemoTest
buf_main = io.StringIO()
prog = unittest.main(
    module=main_mod,
    argv=["prog", "-k", "one"],
    testRunner=unittest.TextTestRunner(stream=buf_main, verbosity=1),
    exit=False,
)
print(
    "unittest.main -k:",
    prog.result.testsRun,
    prog.result.wasSuccessful(),
    normalize_runner_output(buf_main.getvalue()).splitlines()[0],
    normalize_runner_output(buf_main.getvalue()).splitlines()[-1],
)

# Reset defaultTestLoader.testNamePatterns (as unittest.main sets it on defaultTestLoader)
unittest.defaultTestLoader.testNamePatterns = None

buf_main2 = io.StringIO()
prog2 = unittest.main(
    module=main_mod,
    defaultTest="MainDemoTest.test_two",
    argv=["prog"],
    testRunner=unittest.TextTestRunner(stream=buf_main2, verbosity=1),
    exit=False,
)
print("unittest.main defaultTest:", prog2.result.testsRun, prog2.result.wasSuccessful())


# ---------------------------------------------------------------------------
# 11. IsolatedAsyncioTestCase
# ---------------------------------------------------------------------------
async_events = []


class AsyncCM:
    async def __aenter__(self):
        async_events.append("aenter")
        return "async_resource"

    async def __aexit__(self, exc_type, exc_val, tb):
        async_events.append("aexit")
        return False


class AsyncDemoTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        async_events.append("setUp")

    async def asyncSetUp(self):
        async_events.append("asyncSetUp")
        self.queue = asyncio.Queue()
        await self.queue.put("item")
        self.res = await self.enterAsyncContext(AsyncCM())
        self.addAsyncCleanup(self._async_cleanup, "clean")

    async def _async_cleanup(self, tag):
        await asyncio.sleep(0)
        async_events.append(f"asyncCleanup:{tag}")

    async def asyncTearDown(self):
        async_events.append("asyncTearDown")

    def tearDown(self):
        async_events.append("tearDown")

    async def test_async_flow(self):
        val = await self.queue.get()
        self.assertEqual(val, "item")
        self.assertEqual(self.res, "async_resource")
        async_events.append("test_async_flow")


res_async = unittest.TestResult()
AsyncDemoTest("test_async_flow").run(res_async)
print("async lifecycle:", async_events, res_async.testsRun, res_async.wasSuccessful())

# Async debug() and failure handling
class AsyncDebugTest(unittest.IsolatedAsyncioTestCase):
    async def test_ok(self):
        await asyncio.sleep(0)
        self.assertEqual(2 + 2, 4)

    async def test_bad(self):
        await asyncio.sleep(0)
        self.assertEqual(1, 2)


AsyncDebugTest("test_ok").debug()
try:
    AsyncDebugTest("test_bad").debug()
except AssertionError as e:
    print("async debug fail:", str(e))


# ---------------------------------------------------------------------------
# 12. Extra edge cases: setUpClass fail/skip, tearDownClass error, debug(),
#     expectedFailure+subTest, assertRaises/Warns msg=, signals, util,
#     TestProgram exit codes & CLI flags, TestLoader edge cases
# ---------------------------------------------------------------------------
cls_fail_events = []


class SetUpClassFailTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls_fail_events.append("setUpClass")
        cls.addClassCleanup(cls_fail_events.append, "classCleanup")
        raise RuntimeError("cls setup failed")

    @classmethod
    def tearDownClass(cls):
        cls_fail_events.append("tearDownClass")

    def test_never(self):
        cls_fail_events.append("test_never")


res_csf = unittest.TestResult()
unittest.defaultTestLoader.loadTestsFromTestCase(SetUpClassFailTest).run(res_csf)
print(
    "setUpClass fail:",
    cls_fail_events,
    res_csf.testsRun,
    len(res_csf.errors),
    res_csf.wasSuccessful(),
)


class SetUpClassSkipTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise unittest.SkipTest("skip whole class via setUpClass")

    def test_never(self):
        self.fail("should not run")


res_css = unittest.TestResult()
unittest.defaultTestLoader.loadTestsFromTestCase(SetUpClassSkipTest).run(res_css)
print(
    "setUpClass skip:",
    res_css.testsRun,
    len(res_css.skipped),
    res_css.skipped[0][1],
    res_css.wasSuccessful(),
)


class TearDownClassFailTest(unittest.TestCase):
    @classmethod
    def tearDownClass(cls):
        raise RuntimeError("cls teardown failed")

    def test_ok(self):
        self.assertTrue(True)


res_tdc = unittest.TestResult()
unittest.defaultTestLoader.loadTestsFromTestCase(TearDownClassFailTest).run(res_tdc)
print("tearDownClass fail:", res_tdc.testsRun, len(res_tdc.errors), res_tdc.wasSuccessful())


# TestCase.debug() and TestSuite.debug()
debug_events = []


class DebugDemo(unittest.TestCase):
    def setUp(self):
        debug_events.append("setUp")
        self.addCleanup(debug_events.append, "cleanup")

    def tearDown(self):
        debug_events.append("tearDown")

    def test_ok(self):
        debug_events.append("ok")

    def test_fail(self):
        self.fail("debug failure")

    @unittest.skip("debug skip")
    def test_skip(self):
        pass


unittest.TestSuite([DebugDemo("test_ok")]).debug()
print("debug ok events:", debug_events)
try:
    DebugDemo("test_fail").debug()
except AssertionError as e:
    print("debug fail:", str(e))
try:
    DebugDemo("test_skip").debug()
except unittest.SkipTest as e:
    print("debug skip:", str(e))


# expectedFailure with failing subTest
class ExpectedFailSubTest(unittest.TestCase):
    @unittest.expectedFailure
    def test_sub_ef(self):
        self.ran = []
        for i in range(3):
            self.ran.append(i)
            with self.subTest(i=i):
                self.assertNotEqual(i, 1)


ef_st = ExpectedFailSubTest("test_sub_ef")
res_ef_st = unittest.TestResult()
ef_st.run(res_ef_st)
print(
    "expectedFailure subTest:",
    ef_st.ran,
    len(res_ef_st.expectedFailures),
    len(res_ef_st.failures),
    res_ef_st.wasSuccessful(),
)


# assertRaises / assertWarns with msg= kwarg
def raises_with_msg():
    with tc.assertRaises(ValueError, msg="custom raises msg"):
        pass


print("raises msg:", capture_fail(raises_with_msg))


def warns_with_msg():
    with tc.assertWarns(UserWarning, msg="custom warns msg"):
        pass


print("warns msg:", capture_fail(warns_with_msg))


# TestProgram exit=True codes (0=pass, 1=fail, 5=no tests)
mod_pass = types.ModuleType("_exit_pass_mod")
mod_fail = types.ModuleType("_exit_fail_mod")
mod_empty = types.ModuleType("_exit_empty_mod")


class PassOnly(unittest.TestCase):
    def test_a(self):
        self.assertEqual(1, 1)


class FailOnly(unittest.TestCase):
    def test_a(self):
        self.assertEqual(1, 2)


mod_pass.PassOnly = PassOnly
mod_fail.FailOnly = FailOnly

for label, mod in [("pass", mod_pass), ("fail", mod_fail), ("empty", mod_empty)]:
    buf = io.StringIO()
    try:
        unittest.main(
            module=mod,
            argv=["prog", "-q"],
            testRunner=unittest.TextTestRunner(stream=buf, verbosity=0),
            exit=True,
        )
    except SystemExit as e:
        print(f"main exit {label}:", e.code)


# TextTestRunner failfast=True
class FailFastDemo(unittest.TestCase):
    def test_1_fail(self):
        self.fail("first")

    def test_2_pass(self):
        self.assertTrue(True)


buf_ff = io.StringIO()
res_ff_runner = unittest.TextTestRunner(stream=buf_ff, verbosity=0, failfast=True).run(
    unittest.defaultTestLoader.loadTestsFromTestCase(FailFastDemo)
)
print("runner failfast:", res_ff_runner.testsRun, len(res_ff_runner.failures), repr(res_ff_runner))


# TestLoader edge cases
ldr2 = unittest.TestLoader()
print("load base TestCase:", ldr2.loadTestsFromTestCase(unittest.TestCase).countTestCases())
try:
    ldr2.loadTestsFromTestCase(unittest.TestSuite)
except TypeError as e:
    print("load TestSuite TypeError:", "TestCase" in str(e))

bad_suite = ldr2.loadTestsFromName("NoSuchAttr", mod_empty)
res_bad = unittest.TestResult()
bad_suite.run(res_bad)
print("load missing attr:", len(ldr2.errors), len(res_bad.errors), res_bad.wasSuccessful())


# signals & util helpers
r_sig = unittest.TestResult()
unittest.registerResult(r_sig)
print("signals removeResult:", unittest.removeResult(r_sig), unittest.removeResult(r_sig))


@unittest.removeHandler
def decorated_sig_fn(x):
    return x + 1


print("signals removeHandler decorator:", decorated_sig_fn(41))
print("util.three_way_cmp:", unittest.util.three_way_cmp(1, 2), unittest.util.three_way_cmp(2, 2), unittest.util.three_way_cmp(3, 2))
print("util.sorted_list_difference:", unittest.util.sorted_list_difference([1, 2, 2, 3], [2, 3, 4]))
print("util.unorderable_list_difference:", unittest.util.unorderable_list_difference([{"a": 1}, {"b": 2}], [{"b": 2}, {"c": 3}]))
print("util.safe_repr short:", unittest.util.safe_repr("a" * 100, short=True))

print("unittest ok")
