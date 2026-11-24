"""Python 3.12 unit testing framework (`unittest` package and submodules)."""

import collections
import difflib
import functools
import io
import os
import pprint
import re
import sys
import time
import traceback
import types
import warnings

__unittest = True
__package__ = "unittest"
__path__ = []

__all__ = [
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
    "getTestCaseNames",
    "makeSuite",
    "findTestCases",
]


# =============================================================================
# Pure-Python fnmatch helper (PyRust does not ship standalone `fnmatch` yet)
# =============================================================================


def _fnmatch_translate(pat):
    i, n = 0, len(pat)
    res = []
    while i < n:
        c = pat[i]
        i += 1
        if c == "*":
            # Collapse consecutive '*' to avoid catastrophic backtracking.
            while i < n and pat[i] == "*":
                i += 1
            res.append(".*")
        elif c == "?":
            res.append(".")
        elif c == "[":
            j = i
            if j < n and pat[j] == "!":
                j += 1
            if j < n and pat[j] == "]":
                j += 1
            while j < n and pat[j] != "]":
                j += 1
            if j >= n:
                res.append("\\[")
            else:
                stuff = pat[i:j]
                if "--" not in stuff:
                    stuff = stuff.replace("\\", r"\\")
                else:
                    chunks = []
                    k = i + 2 if pat[i] == "!" else i + 1
                    while True:
                        k = pat.find("-", k, j)
                        if k < 0:
                            break
                        chunks.append(pat[i:k])
                        i = k + 1
                        k = k + 3
                    chunks.append(pat[i:j])
                    stuff = "-".join(
                        s.replace("\\", r"\\").replace("-", r"\-")
                        for s in chunks
                    )
                if stuff.startswith("!"):
                    stuff = "^" + stuff[1:]
                elif stuff.startswith(("^", "[")):
                    stuff = "\\" + stuff
                res.append(f"[{stuff}]")
                i = j + 1
        else:
            res.append(re.escape(c))
    return r"(?s:" + "".join(res) + r")\Z"


_fnmatch_cache = {}


def _fnmatchcase(name, pat):
    regex = _fnmatch_cache.get(pat)
    if regex is None:
        if len(_fnmatch_cache) >= 256:
            _fnmatch_cache.clear()
        regex = re.compile(_fnmatch_translate(pat))
        _fnmatch_cache[pat] = regex
    return regex.match(name) is not None


def _normcase(path):
    if os.name == "nt":
        return path.replace("/", "\\").lower()
    return path


def _fnmatch(name, pat):
    return _fnmatchcase(_normcase(name), _normcase(pat))


# =============================================================================
# unittest.util
# =============================================================================

_MAX_LENGTH = 80
_PLACEHOLDER_LEN = 12
_MIN_BEGIN_LEN = 5
_MIN_END_LEN = 5
_MIN_COMMON_LEN = 5
_MIN_DIFF_LEN = _MAX_LENGTH - (
    _MIN_BEGIN_LEN
    + _PLACEHOLDER_LEN
    + _MIN_COMMON_LEN
    + _PLACEHOLDER_LEN
    + _MIN_END_LEN
)
assert _MIN_DIFF_LEN >= 0


def _shorten(s, prefixlen, suffixlen):
    skip = len(s) - prefixlen - suffixlen
    if skip > _PLACEHOLDER_LEN:
        s = "%s[%d chars]%s" % (s[:prefixlen], skip, s[len(s) - suffixlen :])
    return s


def _common_shorten_repr(*args):
    args = tuple(map(safe_repr, args))
    maxlen = max(map(len, args))
    if maxlen <= _MAX_LENGTH:
        return args

    prefix = os.path.commonprefix(args)
    prefixlen = len(prefix)

    common_len = _MAX_LENGTH - (
        maxlen - prefixlen + _MIN_BEGIN_LEN + _PLACEHOLDER_LEN
    )
    if common_len > _MIN_COMMON_LEN:
        prefix = _shorten(prefix, _MIN_BEGIN_LEN, common_len)
        return tuple(prefix + s[prefixlen:] for s in args)

    prefix = _shorten(prefix, _MIN_BEGIN_LEN, _MIN_COMMON_LEN)
    return tuple(
        prefix + _shorten(s[prefixlen:], _MIN_DIFF_LEN, _MIN_END_LEN)
        for s in args
    )


def safe_repr(obj, short=False):
    try:
        result = repr(obj)
    except Exception:
        result = object.__repr__(obj)
    if not short or len(result) < _MAX_LENGTH:
        return result
    return result[:_MAX_LENGTH] + " [truncated]..."


def strclass(cls):
    return "%s.%s" % (cls.__module__, cls.__qualname__)


def sorted_list_difference(expected, actual):
    """Finds elements in only one or the other of two, sorted input lists."""
    i = j = 0
    missing = []
    unexpected = []
    while True:
        try:
            e = expected[i]
            a = actual[j]
            if e < a:
                missing.append(e)
                i += 1
                while expected[i] == e:
                    i += 1
            elif e > a:
                unexpected.append(a)
                j += 1
                while actual[j] == a:
                    j += 1
            else:
                i += 1
                try:
                    while expected[i] == e:
                        i += 1
                finally:
                    j += 1
                    while actual[j] == a:
                        j += 1
        except IndexError:
            missing.extend(expected[i:])
            unexpected.extend(actual[j:])
            break
    return missing, unexpected


def unorderable_list_difference(expected, actual):
    """Same behavior as sorted_list_difference for lists of unorderable items."""
    missing = []
    while expected:
        item = expected.pop()
        try:
            actual.remove(item)
        except ValueError:
            missing.append(item)
    return missing, actual


def three_way_cmp(x, y):
    """Return -1 if x < y, 0 if x == y and 1 if x > y"""
    return (x > y) - (x < y)


_Mismatch = collections.namedtuple("Mismatch", "actual expected value")


def _count_diff_all_purpose(actual, expected):
    """Returns list of (cnt_act, cnt_exp, elem) triples where counts differ."""
    s, t = list(actual), list(expected)
    m, n = len(s), len(t)
    NULL = object()
    result = []
    for i, elem in enumerate(s):
        if elem is NULL:
            continue
        cnt_s = cnt_t = 0
        for j in range(i, m):
            if s[j] == elem:
                cnt_s += 1
                s[j] = NULL
        for j, other_elem in enumerate(t):
            if other_elem == elem:
                cnt_t += 1
                t[j] = NULL
        if cnt_s != cnt_t:
            diff = _Mismatch(cnt_s, cnt_t, elem)
            result.append(diff)

    for i, elem in enumerate(t):
        if elem is NULL:
            continue
        cnt_t = 0
        for j in range(i, n):
            if t[j] == elem:
                cnt_t += 1
                t[j] = NULL
        diff = _Mismatch(0, cnt_t, elem)
        result.append(diff)
    return result


def _count_diff_hashable(actual, expected):
    """Returns list of (cnt_act, cnt_exp, elem) triples where counts differ."""
    s, t = collections.Counter(actual), collections.Counter(expected)
    result = []
    for elem, cnt_s in s.items():
        cnt_t = t.get(elem, 0)
        if cnt_s != cnt_t:
            diff = _Mismatch(cnt_s, cnt_t, elem)
            result.append(diff)
    for elem, cnt_t in t.items():
        if elem not in s:
            diff = _Mismatch(0, cnt_t, elem)
            result.append(diff)
    return result


# =============================================================================
# unittest.signals
# =============================================================================

_registered_results = {}
_interrupt_handler_installed = False


def registerResult(result):
    _registered_results[id(result)] = result


def removeResult(result):
    return _registered_results.pop(id(result), None) is not None


def installHandler():
    global _interrupt_handler_installed
    _interrupt_handler_installed = True


def removeHandler(method=None):
    global _interrupt_handler_installed
    if method is not None:

        @functools.wraps(method)
        def inner(*args, **kwargs):
            prev = _interrupt_handler_installed
            removeHandler()
            try:
                return method(*args, **kwargs)
            finally:
                _set_handler_flag(prev)

        return inner
    _interrupt_handler_installed = False


def _set_handler_flag(val):
    global _interrupt_handler_installed
    _interrupt_handler_installed = val


# =============================================================================
# unittest.result
# =============================================================================


def failfast(method):
    @functools.wraps(method)
    def inner(self, *args, **kw):
        if getattr(self, "failfast", False):
            self.stop()
        return method(self, *args, **kw)

    return inner


STDOUT_LINE = "\nStdout:\n%s"
STDERR_LINE = "\nStderr:\n%s"


class TestResult(object):
    """Holder for test result information."""

    __module__ = "unittest.result"

    _previousTestClass = None
    _testRunEntered = False
    _moduleSetUpFailed = False

    def __init__(self, stream=None, descriptions=None, verbosity=None):
        self.failfast = False
        self.failures = []
        self.errors = []
        self.testsRun = 0
        self.skipped = []
        self.expectedFailures = []
        self.unexpectedSuccesses = []
        self.collectedDurations = []
        self.shouldStop = False
        self.buffer = False
        self.tb_locals = False
        self._stdout_buffer = None
        self._stderr_buffer = None
        self._original_stdout = sys.stdout
        self._original_stderr = sys.stderr
        self._mirrorOutput = False

    def printErrors(self):
        """Called by TestRunner after test run"""

    def startTest(self, test):
        """Called when the given test is about to be run"""
        self.testsRun += 1
        self._mirrorOutput = False
        self._setupStdout()

    def _setupStdout(self):
        if self.buffer:
            if self._stderr_buffer is None:
                self._stderr_buffer = io.StringIO()
                self._stdout_buffer = io.StringIO()
            sys.stdout = self._stdout_buffer
            sys.stderr = self._stderr_buffer

    def startTestRun(self):
        """Called once before any tests are executed."""

    def stopTest(self, test):
        """Called when the given test has been run"""
        self._restoreStdout()
        self._mirrorOutput = False

    def _restoreStdout(self):
        if self.buffer:
            if self._mirrorOutput:
                output = sys.stdout.getvalue()
                error = sys.stderr.getvalue()
                if output:
                    if not output.endswith("\n"):
                        output += "\n"
                    self._original_stdout.write(STDOUT_LINE % output)
                if error:
                    if not error.endswith("\n"):
                        error += "\n"
                    self._original_stderr.write(STDERR_LINE % error)

            sys.stdout = self._original_stdout
            sys.stderr = self._original_stderr
            self._stdout_buffer.seek(0)
            self._stdout_buffer.truncate()
            self._stderr_buffer.seek(0)
            self._stderr_buffer.truncate()

    def stopTestRun(self):
        """Called once after all tests are executed."""

    @failfast
    def addError(self, test, err):
        """Called when an error has occurred."""
        self.errors.append((test, self._exc_info_to_string(err, test)))
        self._mirrorOutput = True

    @failfast
    def addFailure(self, test, err):
        """Called when a failure has occurred."""
        self.failures.append((test, self._exc_info_to_string(err, test)))
        self._mirrorOutput = True

    def addSubTest(self, test, subtest, err):
        """Called at the end of a subtest."""
        if err is not None:
            if getattr(self, "failfast", False):
                self.stop()
            if issubclass(err[0], test.failureException):
                errors = self.failures
            else:
                errors = self.errors
            errors.append((subtest, self._exc_info_to_string(err, test)))
            self._mirrorOutput = True

    def addSuccess(self, test):
        """Called when a test has completed successfully"""
        pass

    def addSkip(self, test, reason):
        """Called when a test is skipped."""
        self.skipped.append((test, reason))

    def addExpectedFailure(self, test, err):
        """Called when an expected failure/error occurred."""
        self.expectedFailures.append(
            (test, self._exc_info_to_string(err, test))
        )

    @failfast
    def addUnexpectedSuccess(self, test):
        """Called when a test was expected to fail, but succeeded."""
        self.unexpectedSuccesses.append(test)

    def addDuration(self, test, elapsed):
        """Called when a test finished to run, regardless of its outcome."""
        if hasattr(self, "collectedDurations"):
            self.collectedDurations.append((str(test), elapsed))

    def wasSuccessful(self):
        """Tells whether or not this result was a success."""
        return len(self.failures) == len(self.errors) == 0 and (
            not hasattr(self, "unexpectedSuccesses")
            or len(self.unexpectedSuccesses) == 0
        )

    def stop(self):
        """Indicates that the tests should be aborted."""
        self.shouldStop = True

    def _exc_info_to_string(self, err, test):
        """Converts a sys.exc_info()-style tuple of values into a string."""
        exctype, value, tb = err
        tb, limit = self._clean_tracebacks(exctype, value, tb, test)
        tb_e = traceback.TracebackException(
            exctype,
            value,
            tb,
            limit=limit,
            capture_locals=self.tb_locals,
            compact=True,
        )
        msgLines = list(tb_e.format())

        if self.buffer:
            output = sys.stdout.getvalue()
            error = sys.stderr.getvalue()
            if output:
                if not output.endswith("\n"):
                    output += "\n"
                msgLines.append(STDOUT_LINE % output)
            if error:
                if not error.endswith("\n"):
                    error += "\n"
                msgLines.append(STDERR_LINE % error)
        return "".join(msgLines)

    def _clean_tracebacks(self, exctype, value, tb, test):
        ret = None
        ret_limit = None
        first = True
        excs = [(exctype, value, tb)]
        seen = {id(value)}
        while excs:
            exctype, value, tb = excs.pop()
            # Skip test runner traceback levels
            while tb and self._is_relevant_tb_level(tb):
                tb = tb.tb_next

            # Skip assert*() traceback levels
            limit = None
            if exctype is test.failureException:
                limit = self._remove_unittest_tb_frames(tb)

            if first:
                ret = tb
                ret_limit = limit
                first = False
            elif value is not None:
                try:
                    value.__traceback__ = tb
                except (AttributeError, TypeError):
                    pass

            if value is not None:
                for c in (
                    getattr(value, "__cause__", None),
                    getattr(value, "__context__", None),
                ):
                    if c is not None and id(c) not in seen:
                        excs.append(
                            (type(c), c, getattr(c, "__traceback__", None))
                        )
                        seen.add(id(c))
        return ret, ret_limit

    def _is_relevant_tb_level(self, tb):
        return "__unittest" in tb.tb_frame.f_globals

    def _remove_unittest_tb_frames(self, tb):
        """Truncates usercode tb at the first unittest frame.

        Returns a frame limit if `tb.tb_next` is read-only so callers can still
        truncate the rendered stack cleanly.
        """
        prev = None
        length = 0
        while tb and not self._is_relevant_tb_level(tb):
            prev = tb
            length += 1
            tb = tb.tb_next
        if prev is not None and tb is not None:
            try:
                prev.tb_next = None
            except (AttributeError, TypeError):
                return length
        return None

    def __repr__(self):
        return "<%s run=%i errors=%i failures=%i>" % (
            strclass(self.__class__),
            self.testsRun,
            len(self.errors),
            len(self.failures),
        )


# =============================================================================
# unittest._log (lazy logging support for assertLogs / assertNoLogs)
# =============================================================================

_LoggingWatcher = collections.namedtuple("_LoggingWatcher", ["records", "output"])


class _AssertLogsContext:
    """A context manager used to implement TestCase.assertLogs / assertNoLogs."""

    __module__ = "unittest._log"

    LOGGING_FORMAT = "%(levelname)s:%(name)s:%(message)s"

    def __init__(self, test_case, logger_name, level, no_logs):
        self.test_case = test_case
        self.logger_name = logger_name
        self.level = level
        self.no_logs = no_logs
        self.msg = None

    def _raiseFailure(self, standardMsg):
        msg = self.test_case._formatMessage(self.msg, standardMsg)
        raise self.test_case.failureException(msg)

    def __enter__(self):
        import logging

        if self.level is not None:
            level = logging._nameToLevel.get(self.level, self.level)
        else:
            level = logging.INFO
        self._resolved_level = level
        if isinstance(self.logger_name, logging.Logger):
            logger = self.logger = self.logger_name
        else:
            logger = self.logger = logging.getLogger(self.logger_name)

        formatter = logging.Formatter(self.LOGGING_FORMAT)
        watcher = _LoggingWatcher([], [])

        class _CapturingHandler(logging.Handler):
            def __init__(self_h):
                super().__init__()
                self_h.watcher = watcher

            def flush(self_h):
                pass

            def emit(self_h, record):
                self_h.watcher.records.append(record)
                msg = self_h.format(record)
                self_h.watcher.output.append(msg)

        handler = _CapturingHandler()
        handler.setLevel(level)
        handler.setFormatter(formatter)
        self.watcher = watcher
        self.old_handlers = logger.handlers[:]
        self.old_level = logger.level
        self.old_propagate = logger.propagate
        logger.handlers = [handler]
        logger.setLevel(level)
        logger.propagate = False
        if self.no_logs:
            return
        return watcher

    def __exit__(self, exc_type, exc_value, tb):
        import logging

        self.logger.handlers = self.old_handlers
        self.logger.propagate = self.old_propagate
        self.logger.setLevel(self.old_level)
        if exc_type is not None:
            return False
        if self.no_logs:
            if len(self.watcher.records) > 0:
                self._raiseFailure(
                    "Unexpected logs found: {!r}".format(self.watcher.output)
                )
        else:
            if len(self.watcher.records) == 0:
                self._raiseFailure(
                    "no logs of level {} or higher triggered on {}".format(
                        logging.getLevelName(self._resolved_level),
                        self.logger.name,
                    )
                )


# =============================================================================
# unittest.case
# =============================================================================

_subtest_msg_sentinel = object()

DIFF_OMITTED = (
    "\nDiff is %s characters long. Set self.maxDiff to None to see it."
)


class SkipTest(Exception):
    """Raise this exception in a test to skip it."""

    __module__ = "unittest.case"


class _ShouldStop(Exception):
    """The test should stop."""

    __module__ = "unittest.case"


class _UnexpectedSuccess(Exception):
    """The test was supposed to fail, but it didn't!"""

    __module__ = "unittest.case"


class _TestPartExecutor(object):
    __module__ = "unittest.case"

    def __init__(self, outcome, test_case, subTest=False):
        self.outcome = outcome
        self.test_case = test_case
        self.subTest = subTest
        self.old_success = True

    def __enter__(self):
        self.old_success = self.outcome.success
        self.outcome.success = True
        return None

    def __exit__(self, exc_type, exc_val, exc_tb):
        outcome = self.outcome
        try:
            if exc_type is None:
                if self.subTest and outcome.success:
                    outcome.result.addSubTest(
                        self.test_case.test_case, self.test_case, None
                    )
                return False
            if issubclass(exc_type, KeyboardInterrupt):
                return False
            if issubclass(exc_type, SkipTest):
                outcome.success = False
                _addSkip(outcome.result, self.test_case, str(exc_val))
                return True
            if issubclass(exc_type, _ShouldStop):
                return True
            exc_info = (exc_type, exc_val, exc_tb)
            if outcome.expecting_failure:
                outcome.expectedFailure = exc_info
            else:
                outcome.success = False
                if self.subTest:
                    outcome.result.addSubTest(
                        self.test_case.test_case, self.test_case, exc_info
                    )
                else:
                    _addError(outcome.result, self.test_case, exc_info)
            exc_info = None
            return True
        finally:
            outcome.success = outcome.success and self.old_success


class _Outcome(object):
    __module__ = "unittest.case"

    def __init__(self, result=None):
        self.expecting_failure = False
        self.result = result
        self.result_supports_subtests = hasattr(result, "addSubTest")
        self.success = True
        self.expectedFailure = None

    def testPartExecutor(self, test_case, subTest=False):
        return _TestPartExecutor(self, test_case, subTest=subTest)


def _addSkip(result, test_case, reason):
    addSkip = getattr(result, "addSkip", None)
    if addSkip is not None:
        addSkip(test_case, reason)
    else:
        warnings.warn(
            "TestResult has no addSkip method, skips not reported",
            RuntimeWarning,
            2,
        )
        result.addSuccess(test_case)


def _addError(result, test, exc_info):
    if result is not None and exc_info is not None:
        if issubclass(exc_info[0], test.failureException):
            result.addFailure(test, exc_info)
        else:
            result.addError(test, exc_info)


def _id(obj):
    return obj


def _enter_context(cm, addcleanup):
    cls = type(cm)
    try:
        enter = cls.__enter__
        exit = cls.__exit__
    except AttributeError:
        raise TypeError(
            f"'{cls.__module__}.{cls.__qualname__}' object does "
            f"not support the context manager protocol"
        ) from None
    result = enter(cm)
    addcleanup(exit, cm, None, None, None)
    return result


_module_cleanups = []


def addModuleCleanup(function, /, *args, **kwargs):
    """Same as addCleanup, except the cleanup items are called even if
    setUpModule fails (unlike tearDownModule)."""
    _module_cleanups.append((function, args, kwargs))


def enterModuleContext(cm):
    """Same as enterContext, but module-wide."""
    return _enter_context(cm, addModuleCleanup)


def doModuleCleanups():
    """Execute all module cleanup functions."""
    exceptions = []
    while _module_cleanups:
        function, args, kwargs = _module_cleanups.pop()
        try:
            function(*args, **kwargs)
        except Exception as exc:
            exceptions.append(exc)
    if exceptions:
        raise exceptions[0]


def skip(reason):
    """Unconditionally skip a test."""

    def decorator(test_item):
        if not isinstance(test_item, type):

            @functools.wraps(test_item)
            def skip_wrapper(*args, **kwargs):
                raise SkipTest(reason)

            test_item = skip_wrapper

        test_item.__unittest_skip__ = True
        test_item.__unittest_skip_why__ = reason
        return test_item

    if isinstance(reason, types.FunctionType):
        test_item = reason
        reason = ""
        return decorator(test_item)
    return decorator


def skipIf(condition, reason):
    """Skip a test if the condition is true."""
    if condition:
        return skip(reason)
    return _id


def skipUnless(condition, reason):
    """Skip a test unless the condition is true."""
    if not condition:
        return skip(reason)
    return _id


def expectedFailure(test_item):
    test_item.__unittest_expecting_failure__ = True
    return test_item


def _is_subtype(expected, basetype):
    if isinstance(expected, tuple):
        return all(_is_subtype(e, basetype) for e in expected)
    return isinstance(expected, type) and issubclass(expected, basetype)


def _iter_warning_classes(expected):
    if isinstance(expected, tuple):
        for item in expected:
            yield from _iter_warning_classes(item)
    else:
        yield expected


class _BaseTestCaseContext:
    __module__ = "unittest.case"

    def __init__(self, test_case):
        self.test_case = test_case

    def _raiseFailure(self, standardMsg):
        msg = self.test_case._formatMessage(self.msg, standardMsg)
        raise self.test_case.failureException(msg)


class _AssertRaisesBaseContext(_BaseTestCaseContext):
    __module__ = "unittest.case"

    def __init__(self, expected, test_case, expected_regex=None):
        _BaseTestCaseContext.__init__(self, test_case)
        self.expected = expected
        self.test_case = test_case
        if expected_regex is not None:
            expected_regex = re.compile(expected_regex)
        self.expected_regex = expected_regex
        self.obj_name = None
        self.msg = None

    def handle(self, name, args, kwargs):
        try:
            if not _is_subtype(self.expected, self._base_type):
                raise TypeError(
                    "%s() arg 1 must be %s" % (name, self._base_type_str)
                )
            if not args:
                self.msg = kwargs.pop("msg", None)
                if kwargs:
                    raise TypeError(
                        "%r is an invalid keyword argument for this function"
                        % (next(iter(kwargs)),)
                    )
                return self

            callable_obj, *args = args
            try:
                self.obj_name = callable_obj.__name__
            except AttributeError:
                self.obj_name = str(callable_obj)
            with self:
                callable_obj(*args, **kwargs)
        finally:
            self = None


class _AssertRaisesContext(_AssertRaisesBaseContext):
    """A context manager used to implement TestCase.assertRaises* methods."""

    __module__ = "unittest.case"

    _base_type = BaseException
    _base_type_str = "an exception type or tuple of exception types"

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, tb):
        if exc_type is None:
            try:
                exc_name = self.expected.__name__
            except AttributeError:
                exc_name = str(self.expected)
            if self.obj_name:
                self._raiseFailure(
                    "{} not raised by {}".format(exc_name, self.obj_name)
                )
            else:
                self._raiseFailure("{} not raised".format(exc_name))
        else:
            traceback.clear_frames(tb)
        if not issubclass(exc_type, self.expected):
            return False
        self.exception = exc_value.with_traceback(None)
        if self.expected_regex is None:
            return True

        expected_regex = self.expected_regex
        if not expected_regex.search(str(exc_value)):
            self._raiseFailure(
                '"{}" does not match "{}"'.format(
                    expected_regex.pattern, str(exc_value)
                )
            )
        return True

    __class_getitem__ = classmethod(types.GenericAlias)


class _AssertWarnsContext(_AssertRaisesBaseContext):
    """A context manager used to implement TestCase.assertWarns* methods."""

    __module__ = "unittest.case"

    _base_type = Warning
    _base_type_str = "a warning type or tuple of warning types"

    def __enter__(self):
        for v in list(sys.modules.values()):
            if getattr(v, "__warningregistry__", None):
                v.__warningregistry__ = {}
        self.warnings_manager = warnings.catch_warnings(record=True)
        self.warnings = self.warnings_manager.__enter__()
        for cat in _iter_warning_classes(self.expected):
            warnings.simplefilter("always", cat)
        return self

    def __exit__(self, exc_type, exc_value, tb):
        self.warnings_manager.__exit__(exc_type, exc_value, tb)
        if exc_type is not None:
            return
        try:
            exc_name = self.expected.__name__
        except AttributeError:
            exc_name = str(self.expected)
        first_matching = None
        for m in self.warnings:
            w = m.message
            if not isinstance(w, self.expected):
                continue
            if first_matching is None:
                first_matching = w
            if self.expected_regex is not None and not self.expected_regex.search(
                str(w)
            ):
                continue
            self.warning = w
            self.filename = m.filename
            self.lineno = m.lineno
            return
        if first_matching is not None:
            self._raiseFailure(
                '"{}" does not match "{}"'.format(
                    self.expected_regex.pattern, str(first_matching)
                )
            )
        if self.obj_name:
            self._raiseFailure(
                "{} not triggered by {}".format(exc_name, self.obj_name)
            )
        else:
            self._raiseFailure("{} not triggered".format(exc_name))


class _OrderedChainMap(collections.ChainMap):
    __module__ = "unittest.case"

    def __iter__(self):
        seen = set()
        for mapping in self.maps:
            for k in mapping:
                if k not in seen:
                    seen.add(k)
                    yield k


class TestCase(object):
    """A class whose instances are single test cases."""

    __module__ = "unittest.case"

    failureException = AssertionError
    longMessage = True
    maxDiff = 80 * 8
    _diffThreshold = 2**16

    def __init_subclass__(cls, *args, **kwargs):
        cls._classSetupFailed = False
        cls._class_cleanups = []
        super().__init_subclass__(*args, **kwargs)

    def __init__(self, methodName="runTest"):
        self._testMethodName = methodName
        self._outcome = None
        self._testMethodDoc = "No test"
        try:
            testMethod = getattr(self, methodName)
        except AttributeError:
            if methodName != "runTest":
                raise ValueError(
                    "no such test method in %s: %s"
                    % (self.__class__, methodName)
                )
        else:
            self._testMethodDoc = testMethod.__doc__
        self._cleanups = []
        self._subtest = None

        self._type_equality_funcs = {}
        self.addTypeEqualityFunc(dict, "assertDictEqual")
        self.addTypeEqualityFunc(list, "assertListEqual")
        self.addTypeEqualityFunc(tuple, "assertTupleEqual")
        self.addTypeEqualityFunc(set, "assertSetEqual")
        self.addTypeEqualityFunc(frozenset, "assertSetEqual")
        self.addTypeEqualityFunc(str, "assertMultiLineEqual")

    def addTypeEqualityFunc(self, typeobj, function):
        """Add a type specific assertEqual style function to compare a type."""
        self._type_equality_funcs[typeobj] = function

    def addCleanup(self, function, /, *args, **kwargs):
        """Add a function, with arguments, to be called when the test is
        completed. Functions added are called on a LIFO basis and are
        called after tearDown on test failure or success."""
        self._cleanups.append((function, args, kwargs))

    def enterContext(self, cm):
        """Enters the supplied context manager and schedules __exit__ cleanup."""
        return _enter_context(cm, self.addCleanup)

    @classmethod
    def addClassCleanup(cls, function, /, *args, **kwargs):
        """Same as addCleanup, except class-wide."""
        cls._class_cleanups.append((function, args, kwargs))

    @classmethod
    def enterClassContext(cls, cm):
        """Same as enterContext, but class-wide."""
        return _enter_context(cm, cls.addClassCleanup)

    def setUp(self):
        """Hook method for setting up the test fixture before exercising it."""
        pass

    def tearDown(self):
        """Hook method for deconstructing the test fixture after testing it."""
        pass

    @classmethod
    def setUpClass(cls):
        """Hook method for setting up class fixture before running tests in the class."""

    @classmethod
    def tearDownClass(cls):
        """Hook method for deconstructing the class fixture after running all tests in the class."""

    def countTestCases(self):
        return 1

    def defaultTestResult(self):
        return TestResult()

    def shortDescription(self):
        """Returns a one-line description of the test, or None."""
        doc = self._testMethodDoc
        return doc.strip().split("\n")[0].strip() if doc else None

    def id(self):
        return "%s.%s" % (strclass(self.__class__), self._testMethodName)

    def __eq__(self, other):
        if type(self) is not type(other):
            return NotImplemented
        return self._testMethodName == other._testMethodName

    def __hash__(self):
        return hash((type(self), self._testMethodName))

    def __str__(self):
        return "%s (%s.%s)" % (
            self._testMethodName,
            strclass(self.__class__),
            self._testMethodName,
        )

    def __repr__(self):
        return "<%s testMethod=%s>" % (
            strclass(self.__class__),
            self._testMethodName,
        )

    def subTest(self, msg=_subtest_msg_sentinel, **params):
        """Return a context manager that runs the enclosed block as a subtest."""
        return _SubTestContext(self, msg, params)

    def _addExpectedFailure(self, result, exc_info):
        try:
            addExpectedFailure = result.addExpectedFailure
        except AttributeError:
            warnings.warn(
                "TestResult has no addExpectedFailure method, reporting as passes",
                RuntimeWarning,
            )
            result.addSuccess(self)
        else:
            addExpectedFailure(self, exc_info)

    def _addUnexpectedSuccess(self, result):
        try:
            addUnexpectedSuccess = result.addUnexpectedSuccess
        except AttributeError:
            warnings.warn(
                "TestResult has no addUnexpectedSuccess method, reporting as failure",
                RuntimeWarning,
            )
            try:
                raise _UnexpectedSuccess from None
            except _UnexpectedSuccess:
                result.addFailure(self, sys.exc_info())
        else:
            addUnexpectedSuccess(self)

    def _addDuration(self, result, elapsed):
        try:
            addDuration = result.addDuration
        except AttributeError:
            warnings.warn(
                "TestResult has no addDuration method", RuntimeWarning
            )
        else:
            addDuration(self, elapsed)

    def _callSetUp(self):
        self.setUp()

    def _callTestMethod(self, method):
        if method() is not None:
            warnings.warn(
                f"It is deprecated to return a value that is not None from a "
                f"test case ({method})",
                DeprecationWarning,
                stacklevel=3,
            )

    def _callTearDown(self):
        self.tearDown()

    def _callCleanup(self, function, /, *args, **kwargs):
        function(*args, **kwargs)

    def run(self, result=None):
        if result is None:
            result = self.defaultTestResult()
            startTestRun = getattr(result, "startTestRun", None)
            stopTestRun = getattr(result, "stopTestRun", None)
            if startTestRun is not None:
                startTestRun()
        else:
            stopTestRun = None

        result.startTest(self)
        try:
            testMethod = getattr(self, self._testMethodName)
            if getattr(self.__class__, "__unittest_skip__", False) or getattr(
                testMethod, "__unittest_skip__", False
            ):
                skip_why = getattr(
                    self.__class__, "__unittest_skip_why__", ""
                ) or getattr(testMethod, "__unittest_skip_why__", "")
                _addSkip(result, self, skip_why)
            else:
                expecting_failure = getattr(
                    self, "__unittest_expecting_failure__", False
                ) or getattr(testMethod, "__unittest_expecting_failure__", False)
                outcome = _Outcome(result)
                start_time = time.perf_counter()
                try:
                    self._outcome = outcome

                    with outcome.testPartExecutor(self):
                        self._callSetUp()
                    if outcome.success:
                        outcome.expecting_failure = expecting_failure
                        with outcome.testPartExecutor(self):
                            self._callTestMethod(testMethod)
                        outcome.expecting_failure = False
                        with outcome.testPartExecutor(self):
                            self._callTearDown()
                    self.doCleanups()
                    self._addDuration(
                        result, (time.perf_counter() - start_time)
                    )

                    if outcome.success:
                        if expecting_failure:
                            if outcome.expectedFailure:
                                self._addExpectedFailure(
                                    result, outcome.expectedFailure
                                )
                            else:
                                self._addUnexpectedSuccess(result)
                        else:
                            result.addSuccess(self)
                finally:
                    outcome.expectedFailure = None
                    outcome = None
                    self._outcome = None
        finally:
            result.stopTest(self)
            if stopTestRun is not None:
                stopTestRun()
        return result

    def doCleanups(self):
        """Execute all cleanup functions."""
        outcome = self._outcome or _Outcome()
        while self._cleanups:
            function, args, kwargs = self._cleanups.pop()
            with outcome.testPartExecutor(self):
                self._callCleanup(function, *args, **kwargs)
        return outcome.success

    @classmethod
    def doClassCleanups(cls):
        """Execute all class cleanup functions."""
        cls.tearDown_exceptions = []
        while cls._class_cleanups:
            function, args, kwargs = cls._class_cleanups.pop()
            try:
                function(*args, **kwargs)
            except Exception:
                cls.tearDown_exceptions.append(sys.exc_info())

    def __call__(self, *args, **kwds):
        return self.run(*args, **kwds)

    def debug(self):
        """Run the test without collecting errors in a TestResult"""
        testMethod = getattr(self, self._testMethodName)
        if getattr(self.__class__, "__unittest_skip__", False) or getattr(
            testMethod, "__unittest_skip__", False
        ):
            skip_why = getattr(
                self.__class__, "__unittest_skip_why__", ""
            ) or getattr(testMethod, "__unittest_skip_why__", "")
            raise SkipTest(skip_why)

        self._callSetUp()
        self._callTestMethod(testMethod)
        self._callTearDown()
        while self._cleanups:
            function, args, kwargs = self._cleanups.pop()
            self._callCleanup(function, *args, **kwargs)

    def skipTest(self, reason):
        """Skip this test."""
        raise SkipTest(reason)

    def fail(self, msg=None):
        """Fail immediately, with the given message."""
        raise self.failureException(msg)

    def assertFalse(self, expr, msg=None):
        """Check that the expression is false."""
        if expr:
            msg = self._formatMessage(msg, "%s is not false" % safe_repr(expr))
            raise self.failureException(msg)

    def assertTrue(self, expr, msg=None):
        """Check that the expression is true."""
        if not expr:
            msg = self._formatMessage(msg, "%s is not true" % safe_repr(expr))
            raise self.failureException(msg)

    def _formatMessage(self, msg, standardMsg):
        if not self.longMessage:
            return msg or standardMsg
        if msg is None:
            return standardMsg
        try:
            return "%s : %s" % (standardMsg, msg)
        except UnicodeDecodeError:
            return "%s : %s" % (safe_repr(standardMsg), safe_repr(msg))

    def assertRaises(self, expected_exception, *args, **kwargs):
        context = _AssertRaisesContext(expected_exception, self)
        try:
            return context.handle("assertRaises", args, kwargs)
        finally:
            context = None

    def assertWarns(self, expected_warning, *args, **kwargs):
        context = _AssertWarnsContext(expected_warning, self)
        return context.handle("assertWarns", args, kwargs)

    def assertLogs(self, logger=None, level=None):
        return _AssertLogsContext(self, logger, level, no_logs=False)

    def assertNoLogs(self, logger=None, level=None):
        return _AssertLogsContext(self, logger, level, no_logs=True)

    def _getAssertEqualityFunc(self, first, second):
        if type(first) is type(second):
            asserter = self._type_equality_funcs.get(type(first))
            if asserter is not None:
                if isinstance(asserter, str):
                    asserter = getattr(self, asserter)
                return asserter
        return self._baseAssertEqual

    def _baseAssertEqual(self, first, second, msg=None):
        if not first == second:
            standardMsg = "%s != %s" % _common_shorten_repr(first, second)
            msg = self._formatMessage(msg, standardMsg)
            raise self.failureException(msg)

    def assertEqual(self, first, second, msg=None):
        assertion_func = self._getAssertEqualityFunc(first, second)
        assertion_func(first, second, msg=msg)

    def assertNotEqual(self, first, second, msg=None):
        if not first != second:
            msg = self._formatMessage(
                msg, "%s == %s" % (safe_repr(first), safe_repr(second))
            )
            raise self.failureException(msg)

    def assertAlmostEqual(
        self, first, second, places=None, msg=None, delta=None
    ):
        if first == second:
            return
        if delta is not None and places is not None:
            raise TypeError("specify delta or places not both")

        diff = abs(first - second)
        if delta is not None:
            if diff <= delta:
                return
            standardMsg = "%s != %s within %s delta (%s difference)" % (
                safe_repr(first),
                safe_repr(second),
                safe_repr(delta),
                safe_repr(diff),
            )
        else:
            if places is None:
                places = 7
            if round(diff, places) == 0:
                return
            standardMsg = "%s != %s within %r places (%s difference)" % (
                safe_repr(first),
                safe_repr(second),
                places,
                safe_repr(diff),
            )
        msg = self._formatMessage(msg, standardMsg)
        raise self.failureException(msg)

    def assertNotAlmostEqual(
        self, first, second, places=None, msg=None, delta=None
    ):
        if delta is not None and places is not None:
            raise TypeError("specify delta or places not both")
        diff = abs(first - second)
        if delta is not None:
            if not (first == second) and diff > delta:
                return
            standardMsg = "%s == %s within %s delta (%s difference)" % (
                safe_repr(first),
                safe_repr(second),
                safe_repr(delta),
                safe_repr(diff),
            )
        else:
            if places is None:
                places = 7
            if not (first == second) and round(diff, places) != 0:
                return
            standardMsg = "%s == %s within %r places" % (
                safe_repr(first),
                safe_repr(second),
                places,
            )
        msg = self._formatMessage(msg, standardMsg)
        raise self.failureException(msg)

    def assertSequenceEqual(self, seq1, seq2, msg=None, seq_type=None):
        if seq_type is not None:
            seq_type_name = seq_type.__name__
            if not isinstance(seq1, seq_type):
                raise self.failureException(
                    "First sequence is not a %s: %s"
                    % (seq_type_name, safe_repr(seq1))
                )
            if not isinstance(seq2, seq_type):
                raise self.failureException(
                    "Second sequence is not a %s: %s"
                    % (seq_type_name, safe_repr(seq2))
                )
        else:
            seq_type_name = "sequence"

        differing = None
        try:
            len1 = len(seq1)
        except (TypeError, NotImplementedError):
            differing = "First %s has no length.    Non-sequence?" % (
                seq_type_name
            )

        if differing is None:
            try:
                len2 = len(seq2)
            except (TypeError, NotImplementedError):
                differing = "Second %s has no length.    Non-sequence?" % (
                    seq_type_name
                )

        if differing is None:
            if seq1 == seq2:
                return

            differing = "%ss differ: %s != %s\n" % (
                (seq_type_name.capitalize(),)
                + _common_shorten_repr(seq1, seq2)
            )

            for i in range(min(len1, len2)):
                try:
                    item1 = seq1[i]
                except (TypeError, IndexError, NotImplementedError):
                    differing += (
                        "\nUnable to index element %d of first %s\n"
                        % (i, seq_type_name)
                    )
                    break

                try:
                    item2 = seq2[i]
                except (TypeError, IndexError, NotImplementedError):
                    differing += (
                        "\nUnable to index element %d of second %s\n"
                        % (i, seq_type_name)
                    )
                    break

                if item1 != item2:
                    differing += (
                        "\nFirst differing element %d:\n%s\n%s\n"
                        % ((i,) + _common_shorten_repr(item1, item2))
                    )
                    break
            else:
                if (
                    len1 == len2
                    and seq_type is None
                    and type(seq1) != type(seq2)
                ):
                    return

            if len1 > len2:
                differing += (
                    "\nFirst %s contains %d additional elements.\n"
                    % (seq_type_name, len1 - len2)
                )
                try:
                    differing += "First extra element %d:\n%s\n" % (
                        len2,
                        safe_repr(seq1[len2]),
                    )
                except (TypeError, IndexError, NotImplementedError):
                    differing += (
                        "Unable to index element %d of first %s\n"
                        % (len2, seq_type_name)
                    )
            elif len1 < len2:
                differing += (
                    "\nSecond %s contains %d additional elements.\n"
                    % (seq_type_name, len2 - len1)
                )
                try:
                    differing += "First extra element %d:\n%s\n" % (
                        len1,
                        safe_repr(seq2[len1]),
                    )
                except (TypeError, IndexError, NotImplementedError):
                    differing += (
                        "Unable to index element %d of second %s\n"
                        % (len1, seq_type_name)
                    )
        standardMsg = differing
        diffMsg = "\n" + "\n".join(
            difflib.ndiff(
                pprint.pformat(seq1).splitlines(),
                pprint.pformat(seq2).splitlines(),
            )
        )

        standardMsg = self._truncateMessage(standardMsg, diffMsg)
        msg = self._formatMessage(msg, standardMsg)
        self.fail(msg)

    def _truncateMessage(self, message, diff):
        max_diff = self.maxDiff
        if max_diff is None or len(diff) <= max_diff:
            return message + diff
        return message + (DIFF_OMITTED % len(diff))

    def assertListEqual(self, list1, list2, msg=None):
        self.assertSequenceEqual(list1, list2, msg, seq_type=list)

    def assertTupleEqual(self, tuple1, tuple2, msg=None):
        self.assertSequenceEqual(tuple1, tuple2, msg, seq_type=tuple)

    def assertSetEqual(self, set1, set2, msg=None):
        try:
            difference1 = set1.difference(set2)
        except TypeError as e:
            self.fail("invalid type when attempting set difference: %s" % e)
        except AttributeError as e:
            self.fail(
                "first argument does not support set difference: %s" % e
            )

        try:
            difference2 = set2.difference(set1)
        except TypeError as e:
            self.fail("invalid type when attempting set difference: %s" % e)
        except AttributeError as e:
            self.fail(
                "second argument does not support set difference: %s" % e
            )

        if not (difference1 or difference2):
            return

        lines = []
        if difference1:
            lines.append("Items in the first set but not the second:")
            for item in difference1:
                lines.append(repr(item))
        if difference2:
            lines.append("Items in the second set but not the first:")
            for item in difference2:
                lines.append(repr(item))

        standardMsg = "\n".join(lines)
        self.fail(self._formatMessage(msg, standardMsg))

    def assertIn(self, member, container, msg=None):
        if member not in container:
            standardMsg = "%s not found in %s" % (
                safe_repr(member),
                safe_repr(container),
            )
            self.fail(self._formatMessage(msg, standardMsg))

    def assertNotIn(self, member, container, msg=None):
        if member in container:
            standardMsg = "%s unexpectedly found in %s" % (
                safe_repr(member),
                safe_repr(container),
            )
            self.fail(self._formatMessage(msg, standardMsg))

    def assertIs(self, expr1, expr2, msg=None):
        if expr1 is not expr2:
            standardMsg = "%s is not %s" % (safe_repr(expr1), safe_repr(expr2))
            self.fail(self._formatMessage(msg, standardMsg))

    def assertIsNot(self, expr1, expr2, msg=None):
        if expr1 is expr2:
            standardMsg = "unexpectedly identical: %s" % (safe_repr(expr1),)
            self.fail(self._formatMessage(msg, standardMsg))

    def assertDictEqual(self, d1, d2, msg=None):
        self.assertIsInstance(d1, dict, "First argument is not a dictionary")
        self.assertIsInstance(d2, dict, "Second argument is not a dictionary")

        if d1 != d2:
            standardMsg = "%s != %s" % _common_shorten_repr(d1, d2)
            diff = "\n" + "\n".join(
                difflib.ndiff(
                    pprint.pformat(d1).splitlines(),
                    pprint.pformat(d2).splitlines(),
                )
            )
            standardMsg = self._truncateMessage(standardMsg, diff)
            self.fail(self._formatMessage(msg, standardMsg))

    def assertCountEqual(self, first, second, msg=None):
        first_seq, second_seq = list(first), list(second)
        try:
            first_c = collections.Counter(first_seq)
            second_c = collections.Counter(second_seq)
        except TypeError:
            differences = _count_diff_all_purpose(first_seq, second_seq)
        else:
            if first_c == second_c:
                return
            differences = _count_diff_hashable(first_seq, second_seq)

        if differences:
            standardMsg = "Element counts were not equal:\n"
            lines = [
                "First has %d, Second has %d:  %r" % tuple(diff)
                for diff in differences
            ]
            diffMsg = "\n".join(lines)
            standardMsg = self._truncateMessage(standardMsg, diffMsg)
            msg = self._formatMessage(msg, standardMsg)
            self.fail(msg)

    def assertMultiLineEqual(self, first, second, msg=None):
        self.assertIsInstance(first, str, "First argument is not a string")
        self.assertIsInstance(second, str, "Second argument is not a string")

        if first != second:
            if (
                len(first) > self._diffThreshold
                or len(second) > self._diffThreshold
            ):
                self._baseAssertEqual(first, second, msg)

            first_presplit = first
            second_presplit = second
            if first and second:
                if first[-1] != "\n" or second[-1] != "\n":
                    first_presplit += "\n"
                    second_presplit += "\n"
            elif second and second[-1] != "\n":
                second_presplit += "\n"
            elif first and first[-1] != "\n":
                first_presplit += "\n"

            firstlines = first_presplit.splitlines(keepends=True)
            secondlines = second_presplit.splitlines(keepends=True)

            standardMsg = "%s != %s" % _common_shorten_repr(first, second)
            diff = "\n" + "".join(difflib.ndiff(firstlines, secondlines))
            standardMsg = self._truncateMessage(standardMsg, diff)
            self.fail(self._formatMessage(msg, standardMsg))

    def assertLess(self, a, b, msg=None):
        if not a < b:
            standardMsg = "%s not less than %s" % (safe_repr(a), safe_repr(b))
            self.fail(self._formatMessage(msg, standardMsg))

    def assertLessEqual(self, a, b, msg=None):
        if not a <= b:
            standardMsg = "%s not less than or equal to %s" % (
                safe_repr(a),
                safe_repr(b),
            )
            self.fail(self._formatMessage(msg, standardMsg))

    def assertGreater(self, a, b, msg=None):
        if not a > b:
            standardMsg = "%s not greater than %s" % (
                safe_repr(a),
                safe_repr(b),
            )
            self.fail(self._formatMessage(msg, standardMsg))

    def assertGreaterEqual(self, a, b, msg=None):
        if not a >= b:
            standardMsg = "%s not greater than or equal to %s" % (
                safe_repr(a),
                safe_repr(b),
            )
            self.fail(self._formatMessage(msg, standardMsg))

    def assertIsNone(self, obj, msg=None):
        if obj is not None:
            standardMsg = "%s is not None" % (safe_repr(obj),)
            self.fail(self._formatMessage(msg, standardMsg))

    def assertIsNotNone(self, obj, msg=None):
        if obj is None:
            standardMsg = "unexpectedly None"
            self.fail(self._formatMessage(msg, standardMsg))

    def assertIsInstance(self, obj, cls, msg=None):
        if not isinstance(obj, cls):
            standardMsg = "%s is not an instance of %r" % (safe_repr(obj), cls)
            self.fail(self._formatMessage(msg, standardMsg))

    def assertNotIsInstance(self, obj, cls, msg=None):
        if isinstance(obj, cls):
            standardMsg = "%s is an instance of %r" % (safe_repr(obj), cls)
            self.fail(self._formatMessage(msg, standardMsg))

    def assertRaisesRegex(
        self, expected_exception, expected_regex, *args, **kwargs
    ):
        context = _AssertRaisesContext(expected_exception, self, expected_regex)
        return context.handle("assertRaisesRegex", args, kwargs)

    def assertWarnsRegex(
        self, expected_warning, expected_regex, *args, **kwargs
    ):
        context = _AssertWarnsContext(expected_warning, self, expected_regex)
        return context.handle("assertWarnsRegex", args, kwargs)

    def assertRegex(self, text, expected_regex, msg=None):
        if isinstance(expected_regex, (str, bytes)):
            assert expected_regex, "expected_regex must not be empty."
            expected_regex = re.compile(expected_regex)
        if not expected_regex.search(text):
            standardMsg = "Regex didn't match: %r not found in %r" % (
                expected_regex.pattern,
                text,
            )
            msg = self._formatMessage(msg, standardMsg)
            raise self.failureException(msg)

    def assertNotRegex(self, text, unexpected_regex, msg=None):
        if isinstance(unexpected_regex, (str, bytes)):
            unexpected_regex = re.compile(unexpected_regex)
        match = unexpected_regex.search(text)
        if match:
            standardMsg = "Regex matched: %r matches %r in %r" % (
                text[match.start() : match.end()],
                unexpected_regex.pattern,
                text,
            )
            msg = self._formatMessage(msg, standardMsg)
            raise self.failureException(msg)


class FunctionTestCase(TestCase):
    """A test case that wraps a test function."""

    __module__ = "unittest.case"

    def __init__(self, testFunc, setUp=None, tearDown=None, description=None):
        super(FunctionTestCase, self).__init__()
        self._setUpFunc = setUp
        self._tearDownFunc = tearDown
        self._testFunc = testFunc
        self._description = description

    def setUp(self):
        if self._setUpFunc is not None:
            self._setUpFunc()

    def tearDown(self):
        if self._tearDownFunc is not None:
            self._tearDownFunc()

    def runTest(self):
        self._testFunc()

    def id(self):
        return self._testFunc.__name__

    def __eq__(self, other):
        if not isinstance(other, self.__class__):
            return NotImplemented
        return (
            self._setUpFunc == other._setUpFunc
            and self._tearDownFunc == other._tearDownFunc
            and self._testFunc == other._testFunc
            and self._description == other._description
        )

    def __hash__(self):
        return hash(
            (
                type(self),
                self._setUpFunc,
                self._tearDownFunc,
                self._testFunc,
                self._description,
            )
        )

    def __str__(self):
        return "%s (%s)" % (strclass(self.__class__), self._testFunc.__name__)

    def __repr__(self):
        return "<%s tec=%s>" % (strclass(self.__class__), self._testFunc)

    def shortDescription(self):
        if self._description is not None:
            return self._description
        doc = self._testFunc.__doc__
        return doc and doc.split("\n")[0].strip() or None


class _SubTestContext(object):
    __module__ = "unittest.case"

    def __init__(self, test_case, msg, params):
        self.test_case = test_case
        self.msg = msg
        self.params = params
        self._active = False
        self._parent = None
        self._executor = None

    def __enter__(self):
        tc = self.test_case
        if tc._outcome is None or not tc._outcome.result_supports_subtests:
            return None
        self._active = True
        parent = tc._subtest
        self._parent = parent
        if parent is None:
            params_map = _OrderedChainMap(self.params)
        else:
            params_map = parent.params.new_child(self.params)
        tc._subtest = _SubTest(tc, self.msg, params_map)
        self._executor = tc._outcome.testPartExecutor(
            tc._subtest, subTest=True
        )
        self._executor.__enter__()
        return None

    def __exit__(self, exc_type, exc_val, exc_tb):
        if not self._active:
            return False
        tc = self.test_case
        try:
            suppress = self._executor.__exit__(exc_type, exc_val, exc_tb)
            if not suppress and exc_type is not None:
                return False
            if not tc._outcome.success:
                result = tc._outcome.result
                if result is not None and result.failfast:
                    raise _ShouldStop
            elif tc._outcome.expectedFailure:
                raise _ShouldStop
            return exc_type is not None
        finally:
            tc._subtest = self._parent


class _SubTest(TestCase):
    __module__ = "unittest.case"

    def __init__(self, test_case, message, params):
        super().__init__()
        self._message = message
        self.test_case = test_case
        self.params = params
        self.failureException = test_case.failureException

    def runTest(self):
        raise NotImplementedError("subtests cannot be run directly")

    def _subDescription(self):
        parts = []
        if self._message is not _subtest_msg_sentinel:
            parts.append("[{}]".format(self._message))
        if self.params:
            params_desc = ", ".join(
                "{}={!r}".format(k, v) for (k, v) in self.params.items()
            )
            parts.append("({})".format(params_desc))
        return " ".join(parts) or "(<subtest>)"

    def id(self):
        return "{} {}".format(self.test_case.id(), self._subDescription())

    def shortDescription(self):
        return self.test_case.shortDescription()

    def __str__(self):
        return "{} {}".format(self.test_case, self._subDescription())


# =============================================================================
# unittest.async_case
# =============================================================================


class IsolatedAsyncioTestCase(TestCase):
    """TestCase subclass that runs async test methods on an isolated event loop."""

    __module__ = "unittest.async_case"

    def __init__(self, methodName="runTest"):
        super().__init__(methodName)
        self._asyncio_running = False

    async def asyncSetUp(self):
        pass

    async def asyncTearDown(self):
        pass

    def addAsyncCleanup(self, func, /, *args, **kwargs):
        self.addCleanup(func, *args, **kwargs)

    async def enterAsyncContext(self, cm):
        cls = type(cm)
        try:
            enter = cls.__aenter__
            exit = cls.__aexit__
        except AttributeError:
            raise TypeError(
                f"'{cls.__module__}.{cls.__qualname__}' object does "
                f"not support the asynchronous context manager protocol"
            ) from None
        result = await enter(cm)
        self.addAsyncCleanup(exit, cm, None, None, None)
        return result

    async def _callMaybeAsyncCoro(self, func, *args, **kwargs):
        import inspect

        res = func(*args, **kwargs)
        if inspect.isawaitable(res):
            return await res
        return res

    def run(self, result=None):
        import asyncio

        if result is None:
            result = self.defaultTestResult()
            startTestRun = getattr(result, "startTestRun", None)
            stopTestRun = getattr(result, "stopTestRun", None)
            if startTestRun is not None:
                startTestRun()
        else:
            stopTestRun = None

        result.startTest(self)
        try:
            testMethod = getattr(self, self._testMethodName)
            if getattr(self.__class__, "__unittest_skip__", False) or getattr(
                testMethod, "__unittest_skip__", False
            ):
                skip_why = getattr(
                    self.__class__, "__unittest_skip_why__", ""
                ) or getattr(testMethod, "__unittest_skip_why__", "")
                _addSkip(result, self, skip_why)
            else:
                expecting_failure = getattr(
                    self, "__unittest_expecting_failure__", False
                ) or getattr(testMethod, "__unittest_expecting_failure__", False)
                outcome = _Outcome(result)
                start_time = time.perf_counter()

                async def _run_async_test():
                    with outcome.testPartExecutor(self):
                        self.setUp()
                        await self.asyncSetUp()
                    if outcome.success:
                        outcome.expecting_failure = expecting_failure
                        with outcome.testPartExecutor(self):
                            ret = await self._callMaybeAsyncCoro(testMethod)
                            if ret is not None:
                                warnings.warn(
                                    f"It is deprecated to return a value that is not None from a "
                                    f"test case ({testMethod})",
                                    DeprecationWarning,
                                    stacklevel=4,
                                )
                        outcome.expecting_failure = False
                        with outcome.testPartExecutor(self):
                            await self.asyncTearDown()
                            self.tearDown()
                    while self._cleanups:
                        function, args, kwargs = self._cleanups.pop()
                        with outcome.testPartExecutor(self):
                            await self._callMaybeAsyncCoro(
                                function, *args, **kwargs
                            )

                try:
                    self._outcome = outcome
                    self._asyncio_running = True
                    asyncio.run(_run_async_test())
                    self._addDuration(
                        result, (time.perf_counter() - start_time)
                    )

                    if outcome.success:
                        if expecting_failure:
                            if outcome.expectedFailure:
                                self._addExpectedFailure(
                                    result, outcome.expectedFailure
                                )
                            else:
                                self._addUnexpectedSuccess(result)
                        else:
                            result.addSuccess(self)
                finally:
                    self._asyncio_running = False
                    outcome.expectedFailure = None
                    outcome = None
                    self._outcome = None
        finally:
            result.stopTest(self)
            if stopTestRun is not None:
                stopTestRun()
        return result

    def debug(self):
        import asyncio

        testMethod = getattr(self, self._testMethodName)
        if getattr(self.__class__, "__unittest_skip__", False) or getattr(
            testMethod, "__unittest_skip__", False
        ):
            skip_why = getattr(
                self.__class__, "__unittest_skip_why__", ""
            ) or getattr(testMethod, "__unittest_skip_why__", "")
            raise SkipTest(skip_why)

        async def _debug_async_test():
            self.setUp()
            await self.asyncSetUp()
            ret = await self._callMaybeAsyncCoro(testMethod)
            if ret is not None:
                warnings.warn(
                    f"It is deprecated to return a value that is not None from a "
                    f"test case ({testMethod})",
                    DeprecationWarning,
                    stacklevel=4,
                )
            await self.asyncTearDown()
            self.tearDown()
            while self._cleanups:
                function, args, kwargs = self._cleanups.pop()
                await self._callMaybeAsyncCoro(function, *args, **kwargs)

        asyncio.run(_debug_async_test())


# =============================================================================
# unittest.suite
# =============================================================================


def _call_if_exists(parent, attr):
    func = getattr(parent, attr, lambda: None)
    func()


class BaseTestSuite(object):
    """A simple test suite that doesn't provide class or module shared fixtures."""

    __module__ = "unittest.suite"

    _cleanup = True

    def __init__(self, tests=()):
        self._tests = []
        self._removed_tests = 0
        self.addTests(tests)

    def __repr__(self):
        return "<%s tests=%s>" % (strclass(self.__class__), list(self))

    def __eq__(self, other):
        if not isinstance(other, self.__class__):
            return NotImplemented
        return list(self) == list(other)

    def __iter__(self):
        return iter(self._tests)

    def countTestCases(self):
        cases = self._removed_tests
        for test in self:
            if test:
                cases += test.countTestCases()
        return cases

    def addTest(self, test):
        if not callable(test):
            raise TypeError("{} is not callable".format(repr(test)))
        if isinstance(test, type) and issubclass(test, (TestCase, TestSuite)):
            raise TypeError(
                "TestCases and TestSuites must be instantiated "
                "before passing them to addTest()"
            )
        self._tests.append(test)

    def addTests(self, tests):
        if isinstance(tests, str):
            raise TypeError("tests must be an iterable of tests, not a string")
        for test in tests:
            self.addTest(test)

    def run(self, result):
        for index, test in enumerate(self):
            if result.shouldStop:
                break
            test(result)
            if self._cleanup:
                self._removeTestAtIndex(index)
        return result

    def _removeTestAtIndex(self, index):
        try:
            test = self._tests[index]
        except TypeError:
            pass
        else:
            if hasattr(test, "countTestCases"):
                self._removed_tests += test.countTestCases()
            self._tests[index] = None

    def __call__(self, *args, **kwds):
        return self.run(*args, **kwds)

    def debug(self):
        for test in self:
            test.debug()


class TestSuite(BaseTestSuite):
    """A test suite is a composite test consisting of a number of TestCases."""

    __module__ = "unittest.suite"

    def run(self, result, debug=False):
        topLevel = False
        if getattr(result, "_testRunEntered", False) is False:
            result._testRunEntered = topLevel = True

        for index, test in enumerate(self):
            if result.shouldStop:
                break

            if _isnotsuite(test):
                self._tearDownPreviousClass(test, result)
                self._handleModuleFixture(test, result)
                self._handleClassSetUp(test, result)
                result._previousTestClass = test.__class__

                if getattr(
                    test.__class__, "_classSetupFailed", False
                ) or getattr(result, "_moduleSetUpFailed", False):
                    continue

            if not debug:
                test(result)
            else:
                test.debug()

            if self._cleanup:
                self._removeTestAtIndex(index)

        if topLevel:
            self._tearDownPreviousClass(None, result)
            self._handleModuleTearDown(result)
            result._testRunEntered = False
        return result

    def debug(self):
        debug = _DebugResult()
        self.run(debug, True)

    def _handleClassSetUp(self, test, result):
        previousClass = getattr(result, "_previousTestClass", None)
        currentClass = test.__class__
        if currentClass == previousClass:
            return
        if result._moduleSetUpFailed:
            return
        if getattr(currentClass, "__unittest_skip__", False):
            return

        failed = False
        try:
            currentClass._classSetupFailed = False
        except TypeError:
            pass

        setUpClass = getattr(currentClass, "setUpClass", None)
        doClassCleanups = getattr(currentClass, "doClassCleanups", None)
        if setUpClass is not None:
            _call_if_exists(result, "_setupStdout")
            try:
                try:
                    setUpClass()
                except Exception as e:
                    if isinstance(result, _DebugResult):
                        raise
                    failed = True
                    try:
                        currentClass._classSetupFailed = True
                    except TypeError:
                        pass
                    className = strclass(currentClass)
                    self._createClassOrModuleLevelException(
                        result, e, "setUpClass", className
                    )
                if failed and doClassCleanups is not None:
                    doClassCleanups()
                    for exc_info in currentClass.tearDown_exceptions:
                        self._createClassOrModuleLevelException(
                            result,
                            exc_info[1],
                            "setUpClass",
                            className,
                            info=exc_info,
                        )
            finally:
                _call_if_exists(result, "_restoreStdout")

    def _get_previous_module(self, result):
        previousModule = None
        previousClass = getattr(result, "_previousTestClass", None)
        if previousClass is not None:
            previousModule = previousClass.__module__
        return previousModule

    def _handleModuleFixture(self, test, result):
        previousModule = self._get_previous_module(result)
        currentModule = test.__class__.__module__
        if currentModule == previousModule:
            return

        self._handleModuleTearDown(result)

        result._moduleSetUpFailed = False
        try:
            module = sys.modules[currentModule]
        except KeyError:
            return
        setUpModule = getattr(module, "setUpModule", None)
        if setUpModule is not None:
            _call_if_exists(result, "_setupStdout")
            try:
                try:
                    setUpModule()
                except Exception as e:
                    if isinstance(result, _DebugResult):
                        raise
                    result._moduleSetUpFailed = True
                    self._createClassOrModuleLevelException(
                        result, e, "setUpModule", currentModule
                    )
                if result._moduleSetUpFailed:
                    try:
                        doModuleCleanups()
                    except Exception as e:
                        self._createClassOrModuleLevelException(
                            result, e, "setUpModule", currentModule
                        )
            finally:
                _call_if_exists(result, "_restoreStdout")

    def _createClassOrModuleLevelException(
        self, result, exc, method_name, parent, info=None
    ):
        errorName = f"{method_name} ({parent})"
        self._addClassOrModuleLevelException(result, exc, errorName, info)

    def _addClassOrModuleLevelException(
        self, result, exception, errorName, info=None
    ):
        error = _ErrorHolder(errorName)
        addSkip = getattr(result, "addSkip", None)
        if addSkip is not None and isinstance(exception, SkipTest):
            addSkip(error, str(exception))
        else:
            if not info:
                result.addError(error, sys.exc_info())
            else:
                result.addError(error, info)

    def _handleModuleTearDown(self, result):
        previousModule = self._get_previous_module(result)
        if previousModule is None:
            return
        if result._moduleSetUpFailed:
            return

        try:
            module = sys.modules[previousModule]
        except KeyError:
            return

        _call_if_exists(result, "_setupStdout")
        try:
            tearDownModule = getattr(module, "tearDownModule", None)
            if tearDownModule is not None:
                try:
                    tearDownModule()
                except Exception as e:
                    if isinstance(result, _DebugResult):
                        raise
                    self._createClassOrModuleLevelException(
                        result, e, "tearDownModule", previousModule
                    )
            try:
                doModuleCleanups()
            except Exception as e:
                if isinstance(result, _DebugResult):
                    raise
                self._createClassOrModuleLevelException(
                    result, e, "tearDownModule", previousModule
                )
        finally:
            _call_if_exists(result, "_restoreStdout")

    def _tearDownPreviousClass(self, test, result):
        previousClass = getattr(result, "_previousTestClass", None)
        currentClass = test.__class__
        if currentClass == previousClass or previousClass is None:
            return
        if getattr(previousClass, "_classSetupFailed", False):
            return
        if getattr(result, "_moduleSetUpFailed", False):
            return
        if getattr(previousClass, "__unittest_skip__", False):
            return

        tearDownClass = getattr(previousClass, "tearDownClass", None)
        doClassCleanups = getattr(previousClass, "doClassCleanups", None)
        if tearDownClass is None and doClassCleanups is None:
            return

        _call_if_exists(result, "_setupStdout")
        try:
            if tearDownClass is not None:
                try:
                    tearDownClass()
                except Exception as e:
                    if isinstance(result, _DebugResult):
                        raise
                    className = strclass(previousClass)
                    self._createClassOrModuleLevelException(
                        result, e, "tearDownClass", className
                    )
            if doClassCleanups is not None:
                doClassCleanups()
                for exc_info in previousClass.tearDown_exceptions:
                    if isinstance(result, _DebugResult):
                        raise exc_info[1]
                    className = strclass(previousClass)
                    self._createClassOrModuleLevelException(
                        result,
                        exc_info[1],
                        "tearDownClass",
                        className,
                        info=exc_info,
                    )
        finally:
            _call_if_exists(result, "_restoreStdout")


class _ErrorHolder(object):
    __module__ = "unittest.suite"

    failureException = None

    def __init__(self, description):
        self.description = description

    def id(self):
        return self.description

    def shortDescription(self):
        return None

    def __repr__(self):
        return "<ErrorHolder description=%r>" % (self.description,)

    def __str__(self):
        return self.id()

    def run(self, result):
        pass

    def __call__(self, result):
        return self.run(result)

    def countTestCases(self):
        return 0


def _isnotsuite(test):
    try:
        iter(test)
    except TypeError:
        return True
    return False


class _DebugResult(object):
    __module__ = "unittest.suite"

    _previousTestClass = None
    _moduleSetUpFailed = False
    shouldStop = False


# =============================================================================
# unittest.loader
# =============================================================================

VALID_MODULE_NAME = re.compile(r"[_a-z]\w*\.py$", re.IGNORECASE)


class _FailedTest(TestCase):
    __module__ = "unittest.loader"

    _testMethodName = None

    def __init__(self, method_name, exception):
        self._exception = exception
        super(_FailedTest, self).__init__(method_name)

    def __getattr__(self, name):
        if name != self._testMethodName:
            return super(_FailedTest, self).__getattr__(name)

        def testFailure():
            raise self._exception

        return testFailure


def _make_failed_import_test(name, suiteClass):
    message = "Failed to import test module: %s\n%s" % (
        name,
        traceback.format_exc(),
    )
    return _make_failed_test(name, ImportError(message), suiteClass, message)


def _make_failed_load_tests(name, exception, suiteClass):
    message = "Failed to call load_tests:\n%s" % (traceback.format_exc(),)
    return _make_failed_test(name, exception, suiteClass, message)


def _make_failed_test(methodname, exception, suiteClass, message):
    test = _FailedTest(methodname, exception)
    return suiteClass((test,)), message


def _make_skipped_test(methodname, exception, suiteClass):
    @skip(str(exception))
    def testSkipped(self):
        pass

    attrs = {methodname: testSkipped}
    TestClass = type("ModuleSkipped", (TestCase,), attrs)
    return suiteClass((TestClass(methodname),))


def _splitext(path):
    return os.path.splitext(path)[0]


class TestLoader(object):
    """Responsible for loading tests and returning them wrapped in a TestSuite."""

    __module__ = "unittest.loader"

    testMethodPrefix = "test"
    sortTestMethodsUsing = staticmethod(three_way_cmp)
    testNamePatterns = None
    suiteClass = TestSuite
    _top_level_dir = None

    def __init__(self):
        super(TestLoader, self).__init__()
        self.errors = []
        self._loading_packages = set()

    def loadTestsFromTestCase(self, testCaseClass):
        """Return a suite of all test cases contained in testCaseClass"""
        if issubclass(testCaseClass, TestSuite):
            raise TypeError(
                "Test cases should not be derived from "
                "TestSuite. Maybe you meant to derive from "
                "TestCase?"
            )
        if testCaseClass in (TestCase, FunctionTestCase):
            testCaseNames = []
        else:
            testCaseNames = self.getTestCaseNames(testCaseClass)
            if not testCaseNames and hasattr(testCaseClass, "runTest"):
                testCaseNames = ["runTest"]
        loaded_suite = self.suiteClass(map(testCaseClass, testCaseNames))
        return loaded_suite

    def loadTestsFromModule(self, module, *, pattern=None):
        """Return a suite of all test cases contained in the given module"""
        tests = []
        for name in dir(module):
            obj = getattr(module, name)
            if (
                isinstance(obj, type)
                and issubclass(obj, TestCase)
                and obj not in (TestCase, FunctionTestCase)
            ):
                tests.append(self.loadTestsFromTestCase(obj))

        load_tests = getattr(module, "load_tests", None)
        tests = self.suiteClass(tests)
        if load_tests is not None:
            try:
                return load_tests(self, tests, pattern)
            except Exception as e:
                error_case, error_message = _make_failed_load_tests(
                    module.__name__, e, self.suiteClass
                )
                self.errors.append(error_message)
                return error_case
        return tests

    def loadTestsFromName(self, name, module=None):
        """Return a suite of all test cases given a string specifier."""
        parts = name.split(".")
        error_case, error_message = None, None
        if module is None:
            parts_copy = parts[:]
            while parts_copy:
                try:
                    module_name = ".".join(parts_copy)
                    module = __import__(module_name)
                    break
                except ImportError:
                    next_attribute = parts_copy.pop()
                    error_case, error_message = _make_failed_import_test(
                        next_attribute, self.suiteClass
                    )
                    if not parts_copy:
                        self.errors.append(error_message)
                        return error_case
            parts = parts[1:]
        obj = module
        parent = None
        for part in parts:
            try:
                parent, obj = obj, getattr(obj, part)
            except AttributeError as e:
                if (
                    getattr(obj, "__path__", None) is not None
                    and error_case is not None
                ):
                    self.errors.append(error_message)
                    return error_case
                else:
                    error_case, error_message = _make_failed_test(
                        part,
                        e,
                        self.suiteClass,
                        "Failed to access attribute:\n%s"
                        % (traceback.format_exc(),),
                    )
                    self.errors.append(error_message)
                    return error_case

        if isinstance(obj, types.ModuleType):
            return self.loadTestsFromModule(obj)
        elif (
            isinstance(obj, type)
            and issubclass(obj, TestCase)
            and obj not in (TestCase, FunctionTestCase)
        ):
            return self.loadTestsFromTestCase(obj)
        elif (
            isinstance(obj, types.FunctionType)
            and isinstance(parent, type)
            and issubclass(parent, TestCase)
        ):
            name = parts[-1]
            inst = parent(name)
            if not isinstance(getattr(inst, name), types.FunctionType):
                return self.suiteClass([inst])
        elif isinstance(obj, TestSuite):
            return obj
        if callable(obj):
            test = obj()
            if isinstance(test, TestSuite):
                return test
            elif isinstance(test, TestCase):
                return self.suiteClass([test])
            else:
                raise TypeError(
                    "calling %s returned %s, not a test" % (obj, test)
                )
        else:
            raise TypeError("don't know how to make test from: %s" % obj)

    def loadTestsFromNames(self, names, module=None):
        """Return a suite of all test cases found using the given sequence
        of string specifiers."""
        suites = [self.loadTestsFromName(name, module) for name in names]
        return self.suiteClass(suites)

    def getTestCaseNames(self, testCaseClass):
        """Return a sorted sequence of method names found within testCaseClass."""

        def shouldIncludeMethod(attrname):
            if not attrname.startswith(self.testMethodPrefix):
                return False
            testFunc = getattr(testCaseClass, attrname)
            if not callable(testFunc):
                return False
            fullName = "%s.%s.%s" % (
                testCaseClass.__module__,
                testCaseClass.__qualname__,
                attrname,
            )
            return self.testNamePatterns is None or any(
                _fnmatchcase(fullName, pattern)
                for pattern in self.testNamePatterns
            )

        testFnNames = list(filter(shouldIncludeMethod, dir(testCaseClass)))
        if self.sortTestMethodsUsing:
            testFnNames.sort(
                key=functools.cmp_to_key(self.sortTestMethodsUsing)
            )
        return testFnNames

    def discover(self, start_dir, pattern="test*.py", top_level_dir=None):
        """Find and return all test modules from the specified start directory."""
        original_top_level_dir = self._top_level_dir
        set_implicit_top = False
        if top_level_dir is None and self._top_level_dir is not None:
            top_level_dir = self._top_level_dir
        elif top_level_dir is None:
            set_implicit_top = True
            top_level_dir = start_dir

        top_level_dir = os.path.abspath(top_level_dir)

        if top_level_dir not in sys.path:
            sys.path.insert(0, top_level_dir)
        self._top_level_dir = top_level_dir

        is_not_importable = False
        if os.path.isdir(os.path.abspath(start_dir)):
            start_dir = os.path.abspath(start_dir)
            if start_dir != top_level_dir:
                is_not_importable = not os.path.isfile(
                    os.path.join(start_dir, "__init__.py")
                )
        else:
            try:
                __import__(start_dir)
            except ImportError:
                is_not_importable = True
            else:
                the_module = sys.modules[start_dir]
                top_part = start_dir.split(".")[0]
                try:
                    start_dir = os.path.abspath(
                        os.path.dirname(the_module.__file__)
                    )
                except AttributeError:
                    if the_module.__name__ in sys.builtin_module_names:
                        raise TypeError(
                            "Can not use builtin modules as dotted module names"
                        ) from None
                    else:
                        raise TypeError(
                            f"don't know how to discover from {the_module!r}"
                        ) from None

                if set_implicit_top:
                    self._top_level_dir = self._get_directory_containing_module(
                        top_part
                    )
                    sys.path.remove(top_level_dir)

        if is_not_importable:
            raise ImportError(
                "Start directory is not importable: %r" % start_dir
            )

        tests = list(self._find_tests(start_dir, pattern))
        self._top_level_dir = original_top_level_dir
        return self.suiteClass(tests)

    def _get_directory_containing_module(self, module_name):
        module = sys.modules[module_name]
        full_path = os.path.abspath(module.__file__)
        if os.path.basename(full_path).lower().startswith("__init__.py"):
            return os.path.dirname(os.path.dirname(full_path))
        return os.path.dirname(full_path)

    def _get_name_from_path(self, path):
        if path == self._top_level_dir:
            return "."
        path = _splitext(os.path.normpath(path))
        _relpath = os.path.relpath(path, self._top_level_dir)
        assert not os.path.isabs(_relpath), "Path must be within the project"
        assert not _relpath.startswith(".."), "Path must be within the project"
        return _relpath.replace(os.path.sep, ".")

    def _get_module_from_name(self, name):
        __import__(name)
        return sys.modules[name]

    def _match_path(self, path, full_path, pattern):
        return _fnmatch(path, pattern)

    def _find_tests(self, start_dir, pattern):
        name = self._get_name_from_path(start_dir)
        if name != "." and name not in self._loading_packages:
            tests, should_recurse = self._find_test_path(start_dir, pattern)
            if tests is not None:
                yield tests
            if not should_recurse:
                return
        paths = sorted(os.listdir(start_dir))
        for path in paths:
            full_path = os.path.join(start_dir, path)
            tests, should_recurse = self._find_test_path(full_path, pattern)
            if tests is not None:
                yield tests
            if should_recurse:
                name = self._get_name_from_path(full_path)
                self._loading_packages.add(name)
                try:
                    yield from self._find_tests(full_path, pattern)
                finally:
                    self._loading_packages.discard(name)

    def _find_test_path(self, full_path, pattern):
        basename = os.path.basename(full_path)
        if os.path.isfile(full_path):
            if not VALID_MODULE_NAME.match(basename):
                return None, False
            if not self._match_path(basename, full_path, pattern):
                return None, False
            name = self._get_name_from_path(full_path)
            try:
                module = self._get_module_from_name(name)
            except SkipTest as e:
                return _make_skipped_test(name, e, self.suiteClass), False
            except:
                error_case, error_message = _make_failed_import_test(
                    name, self.suiteClass
                )
                self.errors.append(error_message)
                return error_case, False
            else:
                mod_file = os.path.abspath(
                    getattr(module, "__file__", full_path)
                )
                realpath = _splitext(os.path.realpath(mod_file))
                fullpath_noext = _splitext(os.path.realpath(full_path))
                if realpath.lower() != fullpath_noext.lower():
                    module_dir = os.path.dirname(realpath)
                    mod_name = _splitext(os.path.basename(full_path))
                    expected_dir = os.path.dirname(full_path)
                    msg = (
                        "%r module incorrectly imported from %r. Expected "
                        "%r. Is this module globally installed?"
                    )
                    raise ImportError(
                        msg % (mod_name, module_dir, expected_dir)
                    )
                return self.loadTestsFromModule(module, pattern=pattern), False
        elif os.path.isdir(full_path):
            if not os.path.isfile(os.path.join(full_path, "__init__.py")):
                return None, False

            name = self._get_name_from_path(full_path)
            try:
                package = self._get_module_from_name(name)
            except SkipTest as e:
                return _make_skipped_test(name, e, self.suiteClass), False
            except:
                error_case, error_message = _make_failed_import_test(
                    name, self.suiteClass
                )
                self.errors.append(error_message)
                return error_case, False
            else:
                load_tests = getattr(package, "load_tests", None)
                self._loading_packages.add(name)
                try:
                    tests = self.loadTestsFromModule(package, pattern=pattern)
                    if load_tests is not None:
                        return tests, False
                    return tests, True
                finally:
                    self._loading_packages.discard(name)
        else:
            return None, False


defaultTestLoader = TestLoader()


def _makeLoader(prefix, sortUsing, suiteClass=None, testNamePatterns=None):
    loader = TestLoader()
    loader.sortTestMethodsUsing = sortUsing
    loader.testMethodPrefix = prefix
    loader.testNamePatterns = testNamePatterns
    if suiteClass:
        loader.suiteClass = suiteClass
    return loader


def getTestCaseNames(
    testCaseClass, prefix, sortUsing=three_way_cmp, testNamePatterns=None
):
    warnings.warn(
        "unittest.getTestCaseNames() is deprecated and will be removed in Python 3.13. "
        "Please use unittest.TestLoader.getTestCaseNames() instead.",
        DeprecationWarning,
        stacklevel=2,
    )
    return _makeLoader(
        prefix, sortUsing, testNamePatterns=testNamePatterns
    ).getTestCaseNames(testCaseClass)


def makeSuite(
    testCaseClass, prefix="test", sortUsing=three_way_cmp, suiteClass=TestSuite
):
    warnings.warn(
        "unittest.makeSuite() is deprecated and will be removed in Python 3.13. "
        "Please use unittest.TestLoader.loadTestsFromTestCase() instead.",
        DeprecationWarning,
        stacklevel=2,
    )
    return _makeLoader(prefix, sortUsing, suiteClass).loadTestsFromTestCase(
        testCaseClass
    )


def findTestCases(
    module, prefix="test", sortUsing=three_way_cmp, suiteClass=TestSuite
):
    warnings.warn(
        "unittest.findTestCases() is deprecated and will be removed in Python 3.13. "
        "Please use unittest.TestLoader.loadTestsFromModule() instead.",
        DeprecationWarning,
        stacklevel=2,
    )
    return _makeLoader(prefix, sortUsing, suiteClass).loadTestsFromModule(
        module
    )


# =============================================================================
# unittest.runner
# =============================================================================


class _WritelnDecorator(object):
    """Used to decorate file-like objects with a handy 'writeln' method"""

    __module__ = "unittest.runner"

    def __init__(self, stream):
        self.stream = stream

    def __getattr__(self, attr):
        if attr in ("stream", "__getstate__"):
            raise AttributeError(attr)
        return getattr(self.stream, attr)

    def writeln(self, arg=None):
        if arg:
            self.write(arg)
        self.write("\n")


class TextTestResult(TestResult):
    """A test result class that can print formatted text results to a stream."""

    __module__ = "unittest.runner"

    separator1 = "=" * 70
    separator2 = "-" * 70

    def __init__(self, stream, descriptions, verbosity, *, durations=None):
        super(TextTestResult, self).__init__(stream, descriptions, verbosity)
        self.stream = stream
        self.showAll = verbosity > 1
        self.dots = verbosity == 1
        self.descriptions = descriptions
        self._newline = True
        self.durations = durations

    def getDescription(self, test):
        doc_first_line = test.shortDescription()
        if self.descriptions and doc_first_line:
            return "\n".join((str(test), doc_first_line))
        else:
            return str(test)

    def startTest(self, test):
        super(TextTestResult, self).startTest(test)
        if self.showAll:
            self.stream.write(self.getDescription(test))
            self.stream.write(" ... ")
            self.stream.flush()
            self._newline = False

    def _write_status(self, test, status):
        is_subtest = isinstance(test, _SubTest)
        if is_subtest or self._newline:
            if not self._newline:
                self.stream.writeln()
            if is_subtest:
                self.stream.write("  ")
            self.stream.write(self.getDescription(test))
            self.stream.write(" ... ")
        self.stream.writeln(status)
        self.stream.flush()
        self._newline = True

    def addSubTest(self, test, subtest, err):
        if err is not None:
            if self.showAll:
                if issubclass(err[0], subtest.failureException):
                    self._write_status(subtest, "FAIL")
                else:
                    self._write_status(subtest, "ERROR")
            elif self.dots:
                if issubclass(err[0], subtest.failureException):
                    self.stream.write("F")
                else:
                    self.stream.write("E")
                self.stream.flush()
        super(TextTestResult, self).addSubTest(test, subtest, err)

    def addSuccess(self, test):
        super(TextTestResult, self).addSuccess(test)
        if self.showAll:
            self._write_status(test, "ok")
        elif self.dots:
            self.stream.write(".")
            self.stream.flush()

    def addError(self, test, err):
        super(TextTestResult, self).addError(test, err)
        if self.showAll:
            self._write_status(test, "ERROR")
        elif self.dots:
            self.stream.write("E")
            self.stream.flush()

    def addFailure(self, test, err):
        super(TextTestResult, self).addFailure(test, err)
        if self.showAll:
            self._write_status(test, "FAIL")
        elif self.dots:
            self.stream.write("F")
            self.stream.flush()

    def addSkip(self, test, reason):
        super(TextTestResult, self).addSkip(test, reason)
        if self.showAll:
            self._write_status(test, "skipped {0!r}".format(reason))
        elif self.dots:
            self.stream.write("s")
            self.stream.flush()

    def addExpectedFailure(self, test, err):
        super(TextTestResult, self).addExpectedFailure(test, err)
        if self.showAll:
            self.stream.writeln("expected failure")
            self.stream.flush()
        elif self.dots:
            self.stream.write("x")
            self.stream.flush()

    def addUnexpectedSuccess(self, test):
        super(TextTestResult, self).addUnexpectedSuccess(test)
        if self.showAll:
            self.stream.writeln("unexpected success")
            self.stream.flush()
        elif self.dots:
            self.stream.write("u")
            self.stream.flush()

    def printErrors(self):
        if self.dots or self.showAll:
            self.stream.writeln()
            self.stream.flush()
        self.printErrorList("ERROR", self.errors)
        self.printErrorList("FAIL", self.failures)
        unexpectedSuccesses = getattr(self, "unexpectedSuccesses", ())
        if unexpectedSuccesses:
            self.stream.writeln(self.separator1)
            for test in unexpectedSuccesses:
                self.stream.writeln(
                    f"UNEXPECTED SUCCESS: {self.getDescription(test)}"
                )
            self.stream.flush()

    def printErrorList(self, flavour, errors):
        for test, err in errors:
            self.stream.writeln(self.separator1)
            self.stream.writeln(
                "%s: %s" % (flavour, self.getDescription(test))
            )
            self.stream.writeln(self.separator2)
            self.stream.writeln("%s" % err)
            self.stream.flush()


class TextTestRunner(object):
    """A test runner class that displays results in textual form."""

    __module__ = "unittest.runner"

    resultclass = TextTestResult

    def __init__(
        self,
        stream=None,
        descriptions=True,
        verbosity=1,
        failfast=False,
        buffer=False,
        resultclass=None,
        warnings=None,
        *,
        tb_locals=False,
        durations=None,
    ):
        if stream is None:
            stream = sys.stderr
        self.stream = _WritelnDecorator(stream)
        self.descriptions = descriptions
        self.verbosity = verbosity
        self.failfast = failfast
        self.buffer = buffer
        self.tb_locals = tb_locals
        self.durations = durations
        self.warnings = warnings
        if resultclass is not None:
            self.resultclass = resultclass

    def _makeResult(self):
        try:
            return self.resultclass(
                self.stream,
                self.descriptions,
                self.verbosity,
                durations=self.durations,
            )
        except TypeError:
            return self.resultclass(
                self.stream, self.descriptions, self.verbosity
            )

    def _printDurations(self, result):
        if not result.collectedDurations:
            return
        ls = sorted(
            result.collectedDurations, key=lambda x: x[1], reverse=True
        )
        if self.durations > 0:
            ls = ls[: self.durations]
        self.stream.writeln("Slowest test durations")
        if hasattr(result, "separator2"):
            self.stream.writeln(result.separator2)
        hidden = False
        for test, elapsed in ls:
            if self.verbosity < 2 and elapsed < 0.001:
                hidden = True
                continue
            self.stream.writeln("%-10s %s" % ("%.3fs" % elapsed, test))
        if hidden:
            self.stream.writeln(
                "\n(durations < 0.001s were hidden; "
                "use -v to show these durations)"
            )
        else:
            self.stream.writeln("")

    def run(self, test):
        """Run the given test case or test suite."""
        result = self._makeResult()
        registerResult(result)
        result.failfast = self.failfast
        result.buffer = self.buffer
        result.tb_locals = self.tb_locals
        with warnings.catch_warnings():
            if self.warnings:
                warnings.simplefilter(self.warnings)
            startTime = time.perf_counter()
            startTestRun = getattr(result, "startTestRun", None)
            if startTestRun is not None:
                startTestRun()
            try:
                test(result)
            finally:
                stopTestRun = getattr(result, "stopTestRun", None)
                if stopTestRun is not None:
                    stopTestRun()
            stopTime = time.perf_counter()
        timeTaken = stopTime - startTime
        result.printErrors()
        if self.durations is not None:
            self._printDurations(result)

        if hasattr(result, "separator2"):
            self.stream.writeln(result.separator2)

        run = result.testsRun
        self.stream.writeln(
            "Ran %d test%s in %.3fs"
            % (run, run != 1 and "s" or "", timeTaken)
        )
        self.stream.writeln()

        expectedFails = unexpectedSuccesses = skipped = 0
        try:
            results = map(
                len,
                (
                    result.expectedFailures,
                    result.unexpectedSuccesses,
                    result.skipped,
                ),
            )
        except AttributeError:
            pass
        else:
            expectedFails, unexpectedSuccesses, skipped = results

        infos = []
        if not result.wasSuccessful():
            self.stream.write("FAILED")
            failed, errored = len(result.failures), len(result.errors)
            if failed:
                infos.append("failures=%d" % failed)
            if errored:
                infos.append("errors=%d" % errored)
        elif run == 0 and not skipped:
            self.stream.write("NO TESTS RAN")
        else:
            self.stream.write("OK")
        if skipped:
            infos.append("skipped=%d" % skipped)
        if expectedFails:
            infos.append("expected failures=%d" % expectedFails)
        if unexpectedSuccesses:
            infos.append("unexpected successes=%d" % unexpectedSuccesses)
        if infos:
            self.stream.writeln(" (%s)" % (", ".join(infos),))
        else:
            self.stream.write("\n")
        self.stream.flush()
        return result


# =============================================================================
# unittest.main
# =============================================================================

_NO_TESTS_EXITCODE = 5

MAIN_EXAMPLES = """\
Examples:
  %(prog)s test_module               - run tests from test_module
  %(prog)s module.TestClass          - run tests from module.TestClass
  %(prog)s module.Class.test_method  - run specified test method
  %(prog)s path/to/test_file.py      - run tests from test_file.py
"""

MODULE_EXAMPLES = """\
Examples:
  %(prog)s                           - run default set of tests
  %(prog)s MyTestSuite               - run suite 'MyTestSuite'
  %(prog)s MyTestCase.testSomething  - run MyTestCase.testSomething
  %(prog)s MyTestCase                - run all 'test*' test methods
                                       in MyTestCase
"""


def _convert_name(name):
    if os.path.isfile(name) and name.lower().endswith(".py"):
        if os.path.isabs(name):
            rel_path = os.path.relpath(name, os.getcwd())
            if os.path.isabs(rel_path) or rel_path.startswith(os.pardir):
                return name
            name = rel_path
        return (
            os.path.normpath(name)[:-3].replace("\\", ".").replace("/", ".")
        )
    return name


def _convert_names(names):
    return [_convert_name(name) for name in names]


def _convert_select_pattern(pattern):
    if "*" not in pattern:
        pattern = "*%s*" % pattern
    return pattern


class TestProgram(object):
    """A command-line program that runs a set of tests."""

    __module__ = "unittest.main"

    module = None
    verbosity = 1
    failfast = catchbreak = buffer = progName = warnings = testNamePatterns = None

    def __init__(
        self,
        module="__main__",
        defaultTest=None,
        argv=None,
        testRunner=None,
        testLoader=defaultTestLoader,
        exit=True,
        verbosity=1,
        failfast=None,
        catchbreak=None,
        buffer=None,
        warnings=None,
        *,
        tb_locals=False,
        durations=None,
    ):
        if isinstance(module, str):
            self.module = __import__(module)
            for part in module.split(".")[1:]:
                self.module = getattr(self.module, part)
        else:
            self.module = module
        if argv is None:
            argv = sys.argv

        self.exit = exit
        self.failfast = failfast
        self.catchbreak = catchbreak
        self.verbosity = verbosity
        self.buffer = buffer
        self.tb_locals = tb_locals
        self.durations = durations
        if warnings is None and not sys.warnoptions:
            self.warnings = "default"
        else:
            self.warnings = warnings
        self.defaultTest = defaultTest
        self.testRunner = testRunner
        self.testLoader = testLoader
        self.progName = os.path.basename(argv[0]) if argv else ""
        self.parseArgs(argv)
        self.runTests()

    def usageExit(self, msg=None):
        warnings.warn(
            "TestProgram.usageExit() is deprecated and will be"
            " removed in Python 3.13",
            DeprecationWarning,
        )
        if msg:
            print(msg)
        self._print_help()
        sys.exit(2)

    def _print_help(self, *args, **kwargs):
        print(
            f"usage: {self.progName} [-h] [-v] [-q] [--locals] "
            "[--durations N] [-f] [-c] [-b] [-k TESTNAMEPATTERNS] "
            "[tests ...]"
        )
        if self.module is None:
            print(MAIN_EXAMPLES % {"prog": self.progName})
        else:
            print(MODULE_EXAMPLES % {"prog": self.progName})

    def _parse_cli_args(self, raw_args, discovery=False):
        allow_failfast = self.failfast is None
        if allow_failfast:
            self.failfast = False
        allow_catch = self.catchbreak is None
        if allow_catch:
            self.catchbreak = False
        allow_buffer = self.buffer is None
        if allow_buffer:
            self.buffer = False
        allow_k = self.testNamePatterns is None
        if allow_k:
            self.testNamePatterns = []

        positionals = []
        i = 0
        n = len(raw_args)
        while i < n:
            arg = raw_args[i]
            if arg == "--":
                positionals.extend(raw_args[i + 1 :])
                break
            if arg in ("-h", "--help"):
                self._print_help()
                sys.exit(0)
            elif arg in ("-v", "--verbose"):
                self.verbosity = 2
            elif arg in ("-q", "--quiet"):
                self.verbosity = 0
            elif arg == "--locals":
                self.tb_locals = True
            elif arg == "--durations":
                if i + 1 >= n:
                    sys.stderr.write(
                        f"{self.progName}: error: argument --durations: expected one argument\n"
                    )
                    sys.exit(2)
                i += 1
                try:
                    self.durations = int(raw_args[i])
                except ValueError:
                    sys.stderr.write(
                        f"{self.progName}: error: argument --durations: invalid int value: {raw_args[i]!r}\n"
                    )
                    sys.exit(2)
            elif arg.startswith("--durations="):
                val = arg.split("=", 1)[1]
                try:
                    self.durations = int(val)
                except ValueError:
                    sys.stderr.write(
                        f"{self.progName}: error: argument --durations: invalid int value: {val!r}\n"
                    )
                    sys.exit(2)
            elif allow_failfast and arg in ("-f", "--failfast"):
                self.failfast = True
            elif allow_catch and arg in ("-c", "--catch"):
                self.catchbreak = True
            elif allow_buffer and arg in ("-b", "--buffer"):
                self.buffer = True
            elif allow_k and arg == "-k":
                if i + 1 >= n:
                    sys.stderr.write(
                        f"{self.progName}: error: argument -k: expected one argument\n"
                    )
                    sys.exit(2)
                i += 1
                self.testNamePatterns.append(
                    _convert_select_pattern(raw_args[i])
                )
            elif allow_k and arg.startswith("-k"):
                val = arg[2:]
                if val.startswith("="):
                    val = val[1:]
                self.testNamePatterns.append(_convert_select_pattern(val))
            elif discovery and arg in ("-s", "--start-directory"):
                if i + 1 >= n:
                    sys.stderr.write(
                        f"{self.progName}: error: argument {arg}: expected one argument\n"
                    )
                    sys.exit(2)
                i += 1
                self.start = raw_args[i]
            elif discovery and arg.startswith("--start-directory="):
                self.start = arg.split("=", 1)[1]
            elif discovery and arg in ("-p", "--pattern"):
                if i + 1 >= n:
                    sys.stderr.write(
                        f"{self.progName}: error: argument {arg}: expected one argument\n"
                    )
                    sys.exit(2)
                i += 1
                self.pattern = raw_args[i]
            elif discovery and arg.startswith("--pattern="):
                self.pattern = arg.split("=", 1)[1]
            elif discovery and arg in ("-t", "--top-level-directory"):
                if i + 1 >= n:
                    sys.stderr.write(
                        f"{self.progName}: error: argument {arg}: expected one argument\n"
                    )
                    sys.exit(2)
                i += 1
                self.top = raw_args[i]
            elif discovery and arg.startswith("--top-level-directory="):
                self.top = arg.split("=", 1)[1]
            elif arg.startswith("-") and len(arg) > 2 and not arg.startswith("--"):
                # Combined short options such as -vf, -qb, -vfkpat
                j = 1
                while j < len(arg):
                    ch = arg[j]
                    if ch == "v":
                        self.verbosity = 2
                    elif ch == "q":
                        self.verbosity = 0
                    elif ch == "f" and allow_failfast:
                        self.failfast = True
                    elif ch == "c" and allow_catch:
                        self.catchbreak = True
                    elif ch == "b" and allow_buffer:
                        self.buffer = True
                    elif ch == "k" and allow_k:
                        rest = arg[j + 1 :]
                        if rest:
                            self.testNamePatterns.append(
                                _convert_select_pattern(rest)
                            )
                        else:
                            if i + 1 >= n:
                                sys.stderr.write(
                                    f"{self.progName}: error: argument -k: expected one argument\n"
                                )
                                sys.exit(2)
                            i += 1
                            self.testNamePatterns.append(
                                _convert_select_pattern(raw_args[i])
                            )
                        break
                    else:
                        sys.stderr.write(
                            f"{self.progName}: error: unrecognized arguments: {arg}\n"
                        )
                        sys.exit(2)
                    j += 1
            elif arg.startswith("-"):
                sys.stderr.write(
                    f"{self.progName}: error: unrecognized arguments: {arg}\n"
                )
                sys.exit(2)
            else:
                positionals.append(arg)
            i += 1

        if discovery:
            if len(positionals) >= 1:
                self.start = positionals[0]
            if len(positionals) >= 2:
                self.pattern = positionals[1]
            if len(positionals) >= 3:
                self.top = positionals[2]
        else:
            self.tests = positionals

    def parseArgs(self, argv):
        raw = list(argv[1:]) if argv else []
        if self.module is None:
            if raw and raw[0].lower() == "discover":
                self._do_discovery(raw[1:])
                return
            self._parse_cli_args(raw, discovery=False)
            if not self.tests:
                self._do_discovery([])
                return
        else:
            self._parse_cli_args(raw, discovery=False)

        if self.tests:
            self.testNames = _convert_names(self.tests)
        elif self.defaultTest is None:
            self.testNames = None
        elif isinstance(self.defaultTest, str):
            self.testNames = (self.defaultTest,)
        else:
            self.testNames = list(self.defaultTest)
        self.createTests()

    def createTests(self, from_discovery=False, Loader=None):
        if self.testNamePatterns:
            self.testLoader.testNamePatterns = self.testNamePatterns
        if from_discovery:
            loader = self.testLoader if Loader is None else Loader()
            self.test = loader.discover(self.start, self.pattern, self.top)
        elif self.testNames is None:
            self.test = self.testLoader.loadTestsFromModule(self.module)
        else:
            self.test = self.testLoader.loadTestsFromNames(
                self.testNames, self.module
            )

    def _do_discovery(self, argv, Loader=None):
        self.start = "."
        self.pattern = "test*.py"
        self.top = None
        if argv is not None:
            self._parse_cli_args(list(argv), discovery=True)
        self.createTests(from_discovery=True, Loader=Loader)

    def runTests(self):
        if self.catchbreak:
            installHandler()
        if self.testRunner is None:
            self.testRunner = TextTestRunner
        if isinstance(self.testRunner, type):
            try:
                try:
                    testRunner = self.testRunner(
                        verbosity=self.verbosity,
                        failfast=self.failfast,
                        buffer=self.buffer,
                        warnings=self.warnings,
                        tb_locals=self.tb_locals,
                        durations=self.durations,
                    )
                except TypeError:
                    testRunner = self.testRunner(
                        verbosity=self.verbosity,
                        failfast=self.failfast,
                        buffer=self.buffer,
                        warnings=self.warnings,
                    )
            except TypeError:
                testRunner = self.testRunner()
        else:
            testRunner = self.testRunner
        self.result = testRunner.run(self.test)
        if self.exit:
            if self.result.testsRun == 0 and len(self.result.skipped) == 0:
                sys.exit(_NO_TESTS_EXITCODE)
            elif self.result.wasSuccessful():
                sys.exit(0)
            else:
                sys.exit(1)


main = TestProgram


# =============================================================================
# Submodule construction & sys.modules registration
# =============================================================================


def _make_submodule(short_name, exports):
    full_name = f"unittest.{short_name}"
    mod = types.ModuleType(full_name)
    mod.__package__ = "unittest"
    mod.__unittest = True
    for attr_name, val in exports.items():
        setattr(mod, attr_name, val)
    sys.modules[full_name] = mod
    return mod


util = _make_submodule(
    "util",
    {
        "_MAX_LENGTH": _MAX_LENGTH,
        "_PLACEHOLDER_LEN": _PLACEHOLDER_LEN,
        "_MIN_BEGIN_LEN": _MIN_BEGIN_LEN,
        "_MIN_END_LEN": _MIN_END_LEN,
        "_MIN_COMMON_LEN": _MIN_COMMON_LEN,
        "_MIN_DIFF_LEN": _MIN_DIFF_LEN,
        "_shorten": _shorten,
        "_common_shorten_repr": _common_shorten_repr,
        "safe_repr": safe_repr,
        "strclass": strclass,
        "sorted_list_difference": sorted_list_difference,
        "unorderable_list_difference": unorderable_list_difference,
        "three_way_cmp": three_way_cmp,
        "_Mismatch": _Mismatch,
        "_count_diff_all_purpose": _count_diff_all_purpose,
        "_count_diff_hashable": _count_diff_hashable,
    },
)

signals = _make_submodule(
    "signals",
    {
        "installHandler": installHandler,
        "registerResult": registerResult,
        "removeResult": removeResult,
        "removeHandler": removeHandler,
    },
)

result = _make_submodule(
    "result",
    {
        "failfast": failfast,
        "STDOUT_LINE": STDOUT_LINE,
        "STDERR_LINE": STDERR_LINE,
        "TestResult": TestResult,
        "util": util,
    },
)

_log = _make_submodule(
    "_log",
    {
        "_LoggingWatcher": _LoggingWatcher,
        "_AssertLogsContext": _AssertLogsContext,
    },
)

case = _make_submodule(
    "case",
    {
        "DIFF_OMITTED": DIFF_OMITTED,
        "SkipTest": SkipTest,
        "_ShouldStop": _ShouldStop,
        "_UnexpectedSuccess": _UnexpectedSuccess,
        "_Outcome": _Outcome,
        "addModuleCleanup": addModuleCleanup,
        "enterModuleContext": enterModuleContext,
        "doModuleCleanups": doModuleCleanups,
        "skip": skip,
        "skipIf": skipIf,
        "skipUnless": skipUnless,
        "expectedFailure": expectedFailure,
        "_AssertRaisesBaseContext": _AssertRaisesBaseContext,
        "_AssertRaisesContext": _AssertRaisesContext,
        "_AssertWarnsContext": _AssertWarnsContext,
        "_OrderedChainMap": _OrderedChainMap,
        "TestCase": TestCase,
        "FunctionTestCase": FunctionTestCase,
        "_SubTest": _SubTest,
        "result": result,
    },
)

suite = _make_submodule(
    "suite",
    {
        "BaseTestSuite": BaseTestSuite,
        "TestSuite": TestSuite,
        "_ErrorHolder": _ErrorHolder,
        "_DebugResult": _DebugResult,
        "case": case,
        "util": util,
    },
)

loader = _make_submodule(
    "loader",
    {
        "VALID_MODULE_NAME": VALID_MODULE_NAME,
        "_FailedTest": _FailedTest,
        "TestLoader": TestLoader,
        "defaultTestLoader": defaultTestLoader,
        "getTestCaseNames": getTestCaseNames,
        "makeSuite": makeSuite,
        "findTestCases": findTestCases,
        "case": case,
        "suite": suite,
        "util": util,
    },
)

runner = _make_submodule(
    "runner",
    {
        "_WritelnDecorator": _WritelnDecorator,
        "TextTestResult": TextTestResult,
        "TextTestRunner": TextTestRunner,
        "result": result,
    },
)

_main_mod = _make_submodule(
    "main",
    {
        "_NO_TESTS_EXITCODE": _NO_TESTS_EXITCODE,
        "MAIN_EXAMPLES": MAIN_EXAMPLES,
        "MODULE_EXAMPLES": MODULE_EXAMPLES,
        "TestProgram": TestProgram,
        "main": TestProgram,
        "loader": loader,
        "runner": runner,
    },
)

async_case = _make_submodule(
    "async_case",
    {
        "IsolatedAsyncioTestCase": IsolatedAsyncioTestCase,
        "TestCase": TestCase,
    },
)
