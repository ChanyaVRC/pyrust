"""CPython 3.12 parity tests for the `logging` standard library module (issue #3151)."""

import io
import logging
import os
import pathlib
import sys
import tempfile
import time
import warnings


# ===========================================================================
# 1. Constants, exports, and level helpers
# ===========================================================================

print("=== 1. levels & exports ===")
print(
    logging.CRITICAL,
    logging.FATAL,
    logging.ERROR,
    logging.WARNING,
    logging.WARN,
    logging.INFO,
    logging.DEBUG,
    logging.NOTSET,
)
print(logging.BASIC_FORMAT)
print(
    sorted(logging.__all__)
    == sorted(
        [
            "BASIC_FORMAT",
            "BufferingFormatter",
            "CRITICAL",
            "DEBUG",
            "ERROR",
            "FATAL",
            "FileHandler",
            "Filter",
            "Formatter",
            "Handler",
            "INFO",
            "LogRecord",
            "Logger",
            "LoggerAdapter",
            "NOTSET",
            "NullHandler",
            "StreamHandler",
            "WARN",
            "WARNING",
            "addLevelName",
            "basicConfig",
            "captureWarnings",
            "critical",
            "debug",
            "disable",
            "error",
            "exception",
            "fatal",
            "getLevelName",
            "getLogger",
            "getLoggerClass",
            "info",
            "log",
            "makeLogRecord",
            "setLoggerClass",
            "shutdown",
            "warn",
            "warning",
            "getLogRecordFactory",
            "setLogRecordFactory",
            "lastResort",
            "raiseExceptions",
            "getLevelNamesMapping",
            "getHandlerByName",
            "getHandlerNames",
        ]
    )
)
for cls in (
    logging.LogRecord,
    logging.Formatter,
    logging.BufferingFormatter,
    logging.Filter,
    logging.Handler,
    logging.StreamHandler,
    logging.FileHandler,
    logging.NullHandler,
    logging.Logger,
    logging.LoggerAdapter,
):
    assert cls.__module__ == "logging", f"bad __module__ for {cls}: {cls.__module__}"
print("class __module__ ok")

print(
    logging.getLevelName(logging.DEBUG),
    logging.getLevelName(logging.INFO),
    logging.getLevelName(logging.WARNING),
    logging.getLevelName(logging.ERROR),
    logging.getLevelName(logging.CRITICAL),
    logging.getLevelName(logging.NOTSET),
)
print(
    logging.getLevelName("DEBUG"),
    logging.getLevelName("WARN"),
    logging.getLevelName("FATAL"),
    logging.getLevelName(42),
    logging.getLevelName("NO_SUCH"),
)

mapping = logging.getLevelNamesMapping()
print(sorted(mapping.items()))
mapping["MUTATED"] = 99
print("MUTATED" in logging.getLevelNamesMapping())

logging.addLevelName(25, "NOTICE")
print(logging.getLevelName(25), logging.getLevelName("NOTICE"))
print(logging.getLevelNamesMapping().get("NOTICE"))

# Invalid levels
tmp_logger = logging.getLogger("test_invalid_levels")
try:
    tmp_logger.setLevel("BOGUS")
except ValueError as e:
    print("bad str level:", str(e))
try:
    tmp_logger.setLevel(1.5)
except TypeError as e:
    print("bad type level:", str(e))
tmp_logger.setLevel("NOTICE")
print("custom level set:", tmp_logger.level)


# ===========================================================================
# 2. Logger hierarchy, Manager, PlaceHolder, getChild, getChildren
# ===========================================================================

print("=== 2. hierarchy & PlaceHolder ===")
print(
    logging.getLogger() is logging.root,
    logging.getLogger(None) is logging.root,
    logging.getLogger("") is logging.root,
    logging.getLogger("root") is logging.root,
)
try:
    logging.getLogger(123)
except TypeError as e:
    print("non-str name:", str(e))

# Out-of-order creation: create deep child before ancestors
deep = logging.getLogger("hier.a.b.c")
print("deep parent before ancestors:", deep.parent.name)
print(
    "placeholders:",
    isinstance(logging.root.manager.loggerDict["hier"], logging.PlaceHolder),
    isinstance(logging.root.manager.loggerDict["hier.a"], logging.PlaceHolder),
    isinstance(logging.root.manager.loggerDict["hier.a.b"], logging.PlaceHolder),
)

mid_top = logging.getLogger("hier")
print("after hier:", mid_top.parent.name, deep.parent.name)

mid_b = logging.getLogger("hier.a.b")
print("after hier.a.b:", mid_b.parent.name, deep.parent.name)

mid_a = logging.getLogger("hier.a")
print("after hier.a:", mid_a.parent.name, mid_b.parent.name, deep.parent.name)

sib = mid_b.getChild("d")
print("getChild:", sib.name, sib.parent is mid_b)
print("root getChild:", logging.root.getChild("hier") is mid_top)

# Grandchild via placeholder should not appear in getChildren()
only_deep = logging.getLogger("gc_root.missing.leaf")
gc_root = logging.getLogger("gc_root")
gc_direct = logging.getLogger("gc_root.direct")
print("gc_root children:", sorted(c.name for c in gc_root.getChildren()))
print("mid_b children:", sorted(c.name for c in mid_b.getChildren()))
print("reprs:", repr(mid_b), repr(logging.root))


# ===========================================================================
# 3. Levels, getEffectiveLevel, isEnabledFor, disabled, logging.disable
# ===========================================================================

print("=== 3. effective levels & disable ===")
p = logging.getLogger("lvl_parent")
c = logging.getLogger("lvl_parent.child")
g = logging.getLogger("lvl_parent.child.grand")

p.setLevel(logging.INFO)
print("effective:", p.getEffectiveLevel(), c.getEffectiveLevel(), g.getEffectiveLevel())
print("enabled:", g.isEnabledFor(logging.DEBUG), g.isEnabledFor(logging.INFO))

# Updating ancestor invalidates cached child isEnabledFor results
p.setLevel(logging.DEBUG)
print("after parent DEBUG:", g.isEnabledFor(logging.DEBUG), g.getEffectiveLevel())

c.setLevel("ERROR")
print("after child ERROR:", g.isEnabledFor(logging.WARNING), g.isEnabledFor(logging.ERROR))

g.disabled = True
print("disabled child:", g.isEnabledFor(logging.CRITICAL))
g.disabled = False

logging.disable(logging.ERROR)
print(
    "global disable ERROR:",
    p.isEnabledFor(logging.ERROR),
    p.isEnabledFor(logging.CRITICAL),
)
logging.disable(logging.NOTSET)
print("after global re-enable:", p.isEnabledFor(logging.DEBUG))


# ===========================================================================
# 4. Propagation, StreamHandler, NullHandler, lastResort, handler names
# ===========================================================================

print("=== 4. handlers & propagation ===")
parent_buf = io.StringIO()
child_buf = io.StringIO()

parent_h = logging.StreamHandler(parent_buf)
parent_h.setFormatter(logging.Formatter("%(name)s|%(levelname)s|%(message)s"))
parent_h.setLevel(logging.WARNING)
parent_h.name = "my_parent_handler"

child_h = logging.StreamHandler(child_buf)
child_h.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))
child_h.name = "my_child_handler"

print(
    "handler lookup:",
    logging.getHandlerByName("my_parent_handler") is parent_h,
    logging.getHandlerByName("nope") is None,
    "my_parent_handler" in logging.getHandlerNames(),
    "my_child_handler" in logging.getHandlerNames(),
)

prop_p = logging.getLogger("prop_p")
prop_p.setLevel(logging.DEBUG)
prop_p.propagate = False
prop_p.addHandler(parent_h)
prop_p.addHandler(parent_h)  # idempotent
print("handler count:", len(prop_p.handlers))

prop_c = logging.getLogger("prop_p.child")
prop_c.addHandler(child_h)
print("hasHandlers:", prop_c.hasHandlers())

prop_c.info("info msg %d", 1)
prop_c.warning("warn msg %s", "hi")

# Disable propagation
prop_c.propagate = False
prop_c.error("only child")

print("child_buf:", repr(child_buf.getvalue()))
print("parent_buf:", repr(parent_buf.getvalue()))

# setStream + terminator
buf2 = io.StringIO()
old = child_h.setStream(buf2)
print("setStream old is child_buf:", old is child_buf)
print("setStream same returns None:", child_h.setStream(buf2) is None)
child_h.terminator = "!"
prop_c.info("custom_term")
print("buf2:", repr(buf2.getvalue()))

# removeHandler & close unregisters handler name
prop_p.removeHandler(parent_h)
parent_h.close()
print("after close:", logging.getHandlerByName("my_parent_handler") is None)

# NullHandler suppresses lastResort
nh_logger = logging.getLogger("null_test")
nh_logger.propagate = False
nh_logger.addHandler(logging.NullHandler())
print("null hasHandlers:", nh_logger.hasHandlers())

# lastResort test with redirected sys.stderr
old_stderr = sys.stderr
err_buf = io.StringIO()
sys.stderr = err_buf
try:
    nh_logger.error("suppressed by null")
    lr_logger = logging.getLogger("lr_test")
    lr_logger.propagate = False
    lr_logger.setLevel(logging.DEBUG)
    lr_logger.info("below lastResort warning")
    lr_logger.warning("hits lastResort")
    saved_lr = logging.lastResort
    logging.lastResort = None
    try:
        lr_logger.error("no handler one-off")
        lr_logger.error("no handler second time")
    finally:
        logging.lastResort = saved_lr
finally:
    sys.stderr = old_stderr
print("stderr capture:", repr(err_buf.getvalue()))


# ===========================================================================
# 5. FileHandler (delay=False, delay=True, mode, encoding, errors, Path)
# ===========================================================================

print("=== 5. FileHandler ===")
with tempfile.TemporaryDirectory() as tmpdir:
    log_path = pathlib.Path(tmpdir) / "app.log"
    fh = logging.FileHandler(log_path, mode="w", encoding="utf-8")
    fh.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))
    print("exists immediately:", log_path.exists())

    flogger = logging.getLogger("file_logger")
    flogger.propagate = False
    flogger.setLevel(logging.DEBUG)
    flogger.addHandler(fh)
    flogger.info("line one")
    # Readable immediately after emit() because StreamHandler flushes
    with open(str(log_path), "r", encoding="utf-8") as fp:
        print("before close:", repr(fp.read()))
    fh.close()
    # Closed 'w' FileHandler does not reopen/truncate on further emit
    flogger.info("after close ignored")
    with open(str(log_path), "r", encoding="utf-8") as fp:
        print("after close:", repr(fp.read()))
    flogger.removeHandler(fh)

    # Append mode + delay=True
    delayed_path = os.path.join(tmpdir, "delayed.log")
    dfh = logging.FileHandler(
        delayed_path, mode="a", encoding="ascii", delay=True, errors="backslashreplace"
    )
    dfh.setFormatter(logging.Formatter("%(message)s"))
    print("delayed exists before emit:", os.path.exists(delayed_path))
    flogger.addHandler(dfh)
    flogger.info("caf\u00e9")
    print("delayed exists after emit:", os.path.exists(delayed_path))
    dfh.close()
    flogger.removeHandler(dfh)
    with open(delayed_path, "r", encoding="ascii") as fp:
        print("delayed ascii backslashreplace:", repr(fp.read()))


# ===========================================================================
# 6. Formatter (%, {, $ styles, defaults, validate, formatTime, exc_info, BufferingFormatter)
# ===========================================================================

print("=== 6. Formatter & styles ===")
rec = logging.LogRecord("app.mod", logging.INFO, "/tmp/demo.py", 42, "hello %s", ("world",), None, "fn")
rec.created = 1700000000.123
rec.msecs = 123.0

f_pct = logging.Formatter("%(asctime)s|%(name)s|%(levelname)s|%(message)s|%(env)s", defaults={"env": "prod"})
f_pct.converter = time.gmtime
print("pct:", f_pct.format(rec), "usesTime:", f_pct.usesTime())

f_pct_datefmt = logging.Formatter("%(asctime)s %(message)s", datefmt="%Y/%m/%d %H:%M:%S")
f_pct_datefmt.converter = time.gmtime
print("pct datefmt:", f_pct_datefmt.format(rec))

f_brace = logging.Formatter("{levelname}:{name}:{message}:{env}", style="{", defaults={"env": "dev"})
print("brace:", f_brace.format(rec), "usesTime:", f_brace.usesTime())

f_dollar = logging.Formatter("$levelname:${name}:$message:$env", style="$", defaults={"env": "stage"})
print("dollar:", f_dollar.format(rec), "usesTime:", f_dollar.usesTime())

# extra overrides defaults
rec_extra = logging.LogRecord("app", logging.INFO, "/tmp/demo.py", 1, "m", (), None)
rec_extra.env = "override"
print("override defaults:", f_brace.format(rec_extra), f_dollar.format(rec_extra))

# Style validation errors
for kwargs in [
    {"fmt": "no fields", "style": "%"},
    {"fmt": "no fields", "style": "{"},
    {"fmt": "no fields", "style": "$"},
    {"fmt": "$$escaped_only", "style": "$"},
    {"fmt": "bad $ sign", "style": "$"},
    {"fmt": "{message!z}", "style": "{"},
    {"fmt": "{message:???}", "style": "{"},
    {"fmt": "{bad-field}", "style": "{"},
    {"fmt": "%(message)s", "style": "?"},
]:
    try:
        logging.Formatter(**kwargs)
        print("UNEXPECTED OK:", kwargs)
    except ValueError as e:
        print("validate error:", kwargs["style"], str(e))

# Escaped $$ and advanced {} format specs
f_dollar_esc = logging.Formatter("$$tag=$levelname:${message}", style="$")
print("dollar esc:", f_dollar_esc.format(rec))
f_brace_spec = logging.Formatter("{levelname:>8s}|{levelno:04d}|{message!r}", style="{")
print("brace spec:", f_brace_spec.format(rec))

# validate=False allows literal-only format (including %% escapes)
f_noval = logging.Formatter("constant 100%% text", style="%", validate=False)
print("noval:", f_noval.format(rec))

# Missing field raises ValueError
try:
    logging.Formatter("%(missing_field)s").format(rec)
except ValueError as e:
    print("missing field:", str(e))

# exc_info formatting
exc_buf = io.StringIO()
exc_h = logging.StreamHandler(exc_buf)
exc_h.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))
exc_logger = logging.getLogger("exc_test")
exc_logger.propagate = False
exc_logger.setLevel(logging.DEBUG)
exc_logger.addHandler(exc_h)

try:
    1 / 0
except ZeroDivisionError as exc:
    exc_logger.exception("boom via exception()")
    exc_logger.error("boom via exc instance", exc_info=exc)

exc_out = exc_buf.getvalue()
print(
    "exc_info output checks:",
    "ERROR:boom via exception()" in exc_out,
    "ERROR:boom via exc instance" in exc_out,
    exc_out.count("ZeroDivisionError: division by zero"),
)

# BufferingFormatter
class CustomBufFmt(logging.BufferingFormatter):
    def formatHeader(self, records):
        return f"HEADER({len(records)})\n"

    def formatFooter(self, records):
        return "FOOTER\n"

bf = CustomBufFmt(logging.Formatter("%(levelname)s:%(message)s\n"))
print("buffering empty:", repr(bf.format([])))
print("buffering 2:", repr(bf.format([rec, rec_extra])))


# ===========================================================================
# 7. LogRecord, makeLogRecord, factory, extra, stack_info, stacklevel
# ===========================================================================

print("=== 7. LogRecord, factory, extra, caller ===")
r_dict = logging.LogRecord("n", logging.INFO, "a.py", 1, "%(k)s=%(v)d", ({"k": "x", "v": 7},), None)
r_zero = logging.LogRecord("n", logging.INFO, "a.py", 1, "val=%d", (0,), None)
r_non_str = logging.LogRecord("n", logging.INFO, "a.py", 1, [1, 2], (), None)
print("getMessage:", r_dict.getMessage(), r_zero.getMessage(), r_non_str.getMessage())
print(
    "record attrs types:",
    isinstance(r_dict.created, float),
    isinstance(r_dict.msecs, float),
    isinstance(r_dict.relativeCreated, float),
    isinstance(r_dict.process, int),
    r_dict.processName,
    isinstance(r_dict.thread, int),
    r_dict.threadName,
    r_dict.taskName,
)

made = logging.makeLogRecord({"name": "sock", "msg": "hi %s", "args": ("there",), "levelno": logging.ERROR})
print("makeLogRecord:", made.name, made.levelno, made.getMessage())

# Custom factory
orig_factory = logging.getLogRecordFactory()
def custom_factory(*args, **kwargs):
    record = orig_factory(*args, **kwargs)
    record.custom_tag = "TAGGED"
    return record

logging.setLogRecordFactory(custom_factory)
try:
    fac_buf = io.StringIO()
    fac_h = logging.StreamHandler(fac_buf)
    fac_h.setFormatter(logging.Formatter("%(custom_tag)s:%(user)s:%(message)s"))
    fac_l = logging.getLogger("fac_test")
    fac_l.propagate = False
    fac_l.setLevel(logging.DEBUG)
    fac_l.addHandler(fac_h)
    fac_l.info("hello", extra={"user": "alice"})
    print("factory + extra:", repr(fac_buf.getvalue()))
    try:
        fac_l.info("bad", extra={"message": "clash"})
    except KeyError as e:
        print("extra clash:", str(e))
finally:
    logging.setLogRecordFactory(orig_factory)

# stacklevel & stack_info
caller_records = []
class RecordCollector(logging.Handler):
    def emit(self, record):
        caller_records.append(record)

caller_logger = logging.getLogger("caller_test")
caller_logger.propagate = False
caller_logger.setLevel(logging.DEBUG)
caller_logger.addHandler(RecordCollector())

def inner_helper(stacklevel):
    caller_logger.info("from helper", stacklevel=stacklevel, stack_info=(stacklevel == 1))

def outer_caller():
    inner_helper(1)
    inner_helper(2)

outer_caller()
print(
    "stacklevel funcs:",
    caller_records[0].funcName,
    caller_records[1].funcName,
    caller_records[0].stack_info is not None and "Stack (most recent call last):" in caller_records[0].stack_info,
    caller_records[1].stack_info is None,
)


# ===========================================================================
# 8. Filter & Filterer (prefix matching, callable filters, record replacement)
# ===========================================================================

print("=== 8. Filter & Filterer ===")
flt_sub = logging.Filter("pkg.sub")
flt_all = logging.Filter("")
for rname in ("pkg.sub", "pkg.sub.mod", "pkg.sub2", "other.pkg.sub", "pkg"):
    r = logging.LogRecord(rname, logging.INFO, "f.py", 1, "m", (), None)
    print("filter", rname, flt_sub.filter(r), flt_all.filter(r))

# Callable filter + Python 3.12 LogRecord replacement filter
flt_buf = io.StringIO()
flt_h = logging.StreamHandler(flt_buf)
flt_h.setFormatter(logging.Formatter("%(name)s:%(message)s"))

def drop_secret(record):
    return "secret" not in record.getMessage()

def rewrite_record(record):
    if record.msg == "old":
        return logging.LogRecord(record.name, record.levelno, record.pathname, record.lineno, "NEW", (), None)
    return True

flt_l = logging.getLogger("flt_logger")
flt_l.propagate = False
flt_l.setLevel(logging.DEBUG)
flt_l.addHandler(flt_h)
flt_l.addFilter(drop_secret)
flt_h.addFilter(rewrite_record)

flt_l.info("public")
flt_l.info("top secret")
flt_l.info("old")
flt_l.removeFilter(drop_secret)
flt_l.info("secret allowed now")
print("filter output:", repr(flt_buf.getvalue()))


# ===========================================================================
# 9. LoggerAdapter & GenericAlias
# ===========================================================================

print("=== 9. LoggerAdapter & GenericAlias ===")
ad_buf = io.StringIO()
ad_h = logging.StreamHandler(ad_buf)
ad_h.setFormatter(logging.Formatter("[%(req)s] %(levelname)s %(message)s"))
ad_logger = logging.getLogger("ad_logger")
ad_logger.propagate = False
ad_logger.setLevel(logging.INFO)
ad_logger.addHandler(ad_h)

adapter = logging.LoggerAdapter(ad_logger, {"req": "r-42"})
print("adapter meta:", adapter.name, adapter.getEffectiveLevel(), adapter.hasHandlers(), adapter.manager is ad_logger.manager)
print("adapter repr:", repr(adapter))
adapter.debug("ignored")
adapter.info("hello %s", "adapter")
adapter.setLevel(logging.DEBUG)
adapter.debug("now visible")

class PrefixAdapter(logging.LoggerAdapter):
    def process(self, msg, kwargs):
        kwargs["extra"] = self.extra
        return f"PREFIX:{msg}", kwargs

pad = PrefixAdapter(ad_logger, {"req": "r-99"})
pad.warning("warned")
print("adapter output:", repr(ad_buf.getvalue()))

print(
    "GenericAlias:",
    logging.StreamHandler[io.StringIO].__origin__ is logging.StreamHandler,
    logging.LoggerAdapter[logging.Logger].__origin__ is logging.LoggerAdapter,
)


# ===========================================================================
# 10. basicConfig, module-level functions, setLoggerClass/getLoggerClass
# ===========================================================================

print("=== 10. basicConfig & top-level API ===")
bc_buf1 = io.StringIO()
logging.basicConfig(stream=bc_buf1, format="{levelname}-{message}", style="{", level=logging.INFO, force=True)
logging.debug("skip debug")
logging.info("top info %d", 1)
logging.warning("top warn")
logging.error("top err")
logging.critical("top crit")
logging.fatal("top fatal")
logging.log(logging.INFO, "top log")

# Without force=True, basicConfig is a no-op
bc_buf2 = io.StringIO()
logging.basicConfig(stream=bc_buf2, level=logging.DEBUG)
logging.debug("still skipped")
print("bc_buf1:", repr(bc_buf1.getvalue()))
print("bc_buf2 empty:", repr(bc_buf2.getvalue()))

# force=True replaces handlers
h_pre = logging.StreamHandler(bc_buf2)
h_custom = logging.StreamHandler(bc_buf2)
h_custom.setFormatter(logging.Formatter("CUSTOM:%(message)s"))
logging.basicConfig(handlers=[h_pre, h_custom], format="DEF:%(message)s", level=logging.DEBUG, force=True)
logging.debug("both handlers")
print("bc_buf2 force:", repr(bc_buf2.getvalue()))

# Conflicting basicConfig args
for bad_kw in [
    {"stream": io.StringIO(), "filename": "x.log", "force": True},
    {"handlers": [logging.NullHandler()], "stream": io.StringIO(), "force": True},
    {"handlers": [logging.NullHandler()], "filename": "x.log", "force": True},
    {"stream": io.StringIO(), "bogus_kwarg": 1, "force": True},
]:
    try:
        logging.basicConfig(**bad_kw)
    except ValueError as e:
        print("basicConfig ValueError:", str(e))

# Custom Logger class
orig_cls = logging.getLoggerClass()
print("default logger class:", orig_cls is logging.Logger)
try:
    logging.setLoggerClass(object)
except TypeError as e:
    print("bad logger class:", str(e))

class CustomLogger(logging.Logger):
    custom_flag = True

logging.setLoggerClass(CustomLogger)
try:
    cl = logging.getLogger("custom_cls_logger")
    print("custom logger:", isinstance(cl, CustomLogger), cl.custom_flag)
finally:
    logging.setLoggerClass(orig_cls)


# ===========================================================================
# 11. captureWarnings
# ===========================================================================

print("=== 11. captureWarnings ===")
warn_buf = io.StringIO()
warn_h = logging.StreamHandler(warn_buf)
warn_h.setFormatter(logging.Formatter("%(name)s|%(levelname)s|%(message)s"))
py_warn_logger = logging.getLogger("py.warnings")
py_warn_logger.propagate = False
py_warn_logger.setLevel(logging.DEBUG)
py_warn_logger.addHandler(warn_h)

logging.captureWarnings(True)
try:
    with warnings.catch_warnings():
        warnings.simplefilter("always")
        warnings.warn("captured warning message", UserWarning)
finally:
    logging.captureWarnings(False)

warn_text = warn_buf.getvalue()
print(
    "captured warning:",
    "py.warnings|WARNING|" in warn_text,
    "UserWarning: captured warning message" in warn_text,
)

# After captureWarnings(False), warnings no longer go to py.warnings
before_len = len(warn_buf.getvalue())
old_stderr2 = sys.stderr
uncaptured_stderr = io.StringIO()
sys.stderr = uncaptured_stderr
try:
    with warnings.catch_warnings():
        warnings.simplefilter("always")
        warnings.warn("not captured", UserWarning)
finally:
    sys.stderr = old_stderr2
print(
    "after captureWarnings(False):",
    len(warn_buf.getvalue()) == before_len,
    "not captured" in uncaptured_stderr.getvalue(),
)


# ===========================================================================
# 12. shutdown
# ===========================================================================

print("=== 12. shutdown ===")
events = []

class TrackedHandler(logging.Handler):
    def __init__(self, label):
        super().__init__()
        self.label = label

    def flush(self):
        events.append(("flush", self.label))

    def close(self):
        events.append(("close", self.label))
        super().close()

t1 = TrackedHandler("first")
t2 = TrackedHandler("second")
logging.shutdown([lambda: t1, lambda: t2])
print("shutdown order:", events)


# ===========================================================================
# 13. Extra edge cases: raiseExceptions, log* flags, basicConfig(filename),
#     Handler.emit NotImplementedError, caller metadata, deprecated warn()
# ===========================================================================

print("=== 13. extra edge cases ===")

# Base Handler.emit raises NotImplementedError
try:
    logging.Handler().emit(rec)
except NotImplementedError as e:
    print("base emit:", str(e))

# Non-int level in logger.log with raiseExceptions True vs False
el = logging.getLogger("edge_log")
el.propagate = False
el_buf = io.StringIO()
el.addHandler(logging.StreamHandler(el_buf))
try:
    el.log("INFO", "bad level")
except TypeError as e:
    print("non-int level raise:", str(e))

saved_re = logging.raiseExceptions
logging.raiseExceptions = False
old_stderr3 = sys.stderr
err_buf3 = io.StringIO()
sys.stderr = err_buf3
try:
    el.log("INFO", "silently ignored")
    # Bad format args swallowed when raiseExceptions is False
    el.error("bad %d", "not_a_number")
finally:
    sys.stderr = old_stderr3
    logging.raiseExceptions = saved_re
print("raiseExceptions=False stderr empty:", err_buf3.getvalue() == "")

# logThreads / logProcesses / logMultiprocessing toggles
logging.logThreads = False
logging.logProcesses = False
logging.logMultiprocessing = False
try:
    r_off = logging.LogRecord("n", logging.INFO, "a.py", 1, "m", (), None)
    print("flags off:", r_off.thread, r_off.threadName, r_off.process, r_off.processName)
finally:
    logging.logThreads = True
    logging.logProcesses = True
    logging.logMultiprocessing = True

# Caller metadata (filename, module, lineno)
print(
    "caller metadata:",
    caller_records[0].filename,
    caller_records[0].module,
    caller_records[0].lineno > 0,
)

# basicConfig(filename=..., filemode='w', force=True)
with tempfile.TemporaryDirectory() as tmpdir2:
    bc_file = os.path.join(tmpdir2, "bc.log")
    logging.basicConfig(
        filename=bc_file,
        filemode="w",
        format="%(levelname)s:%(message)s",
        level=logging.INFO,
        encoding="utf-8",
        force=True,
    )
    logging.info("via basicConfig file")
    # Force-reconfiguring closes the previous FileHandler before tmpdir cleanup
    logging.basicConfig(stream=io.StringIO(), force=True)
    with open(bc_file, "r", encoding="utf-8") as fp:
        print("basicConfig file:", repr(fp.read()))

# Deprecated warn() emits DeprecationWarning and logs at WARNING level
w_buf = io.StringIO()
w_logger = logging.getLogger("warn_dep")
w_logger.propagate = False
w_logger.setLevel(logging.DEBUG)
w_logger.addHandler(logging.StreamHandler(w_buf))
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    w_logger.warn("legacy warn")
print(
    "deprecated warn:",
    len(caught) == 1 and issubclass(caught[0].category, DeprecationWarning),
    repr(w_buf.getvalue()),
)

