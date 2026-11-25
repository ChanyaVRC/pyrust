"""CPython 3.12 parity tests for `logging.handlers` and `logging.config` (issue #3251)."""

import io
import logging
import logging.config
import logging.handlers
import os
import pickle
import queue
import struct
import tempfile


def _reset_logging():
    root = logging.getLogger()
    for h in list(root.handlers):
        root.removeHandler(h)
        try:
            h.close()
        except Exception:
            pass
    root.setLevel(logging.WARNING)


# ===========================================================================
# 1. Submodule binding & class hierarchy
# ===========================================================================
print("=== 1. binding & hierarchy ===")
print(hasattr(logging, "handlers"), hasattr(logging, "config"))
print(logging.handlers.DEFAULT_TCP_LOGGING_PORT, logging.handlers.SYSLOG_UDP_PORT)
print(
    issubclass(logging.handlers.BaseRotatingHandler, logging.FileHandler),
    issubclass(logging.handlers.RotatingFileHandler, logging.handlers.BaseRotatingHandler),
    issubclass(logging.handlers.TimedRotatingFileHandler, logging.handlers.BaseRotatingHandler),
    issubclass(logging.handlers.WatchedFileHandler, logging.FileHandler),
    issubclass(logging.handlers.BufferingHandler, logging.Handler),
    issubclass(logging.handlers.MemoryHandler, logging.handlers.BufferingHandler),
    issubclass(logging.handlers.QueueHandler, logging.Handler),
    issubclass(logging.handlers.SocketHandler, logging.Handler),
    issubclass(logging.handlers.DatagramHandler, logging.handlers.SocketHandler),
    issubclass(logging.handlers.SysLogHandler, logging.Handler),
    issubclass(logging.handlers.HTTPHandler, logging.Handler),
    issubclass(logging.handlers.SMTPHandler, logging.Handler),
)


# ===========================================================================
# 2. RotatingFileHandler (real rollover + .1/.2 backups + namer/rotator)
# ===========================================================================
print("=== 2. RotatingFileHandler ===")
with tempfile.TemporaryDirectory() as tmpdir:
    log_path = os.path.join(tmpdir, "app.log")
    rh = logging.handlers.RotatingFileHandler(
        log_path, maxBytes=25, backupCount=2, encoding="utf-8"
    )
    rh.setFormatter(logging.Formatter("%(message)s"))
    logger = logging.getLogger("test.rot")
    logger.handlers.clear()
    logger.propagate = False
    logger.setLevel(logging.INFO)
    logger.addHandler(rh)

    for i in range(6):
        logger.info("message-number-%02d", i)
    rh.close()
    logger.removeHandler(rh)

    files = sorted(os.listdir(tmpdir))
    print("rotated files:", files)
    with open(log_path, "r", encoding="utf-8") as f:
        cur = f.read().strip().splitlines()
    with open(log_path + ".1", "r", encoding="utf-8") as f:
        b1 = f.read().strip().splitlines()
    with open(log_path + ".2", "r", encoding="utf-8") as f:
        b2 = f.read().strip().splitlines()
    print("cur non-empty:", len(cur) > 0, "b1 non-empty:", len(b1) > 0, "b2 non-empty:", len(b2) > 0)
    print("latest msg:", cur[-1])

    # Custom namer
    custom_path = os.path.join(tmpdir, "custom.log")
    rh2 = logging.handlers.RotatingFileHandler(custom_path, maxBytes=10, backupCount=1)
    rh2.namer = lambda name: name + ".bak"
    rh2.setFormatter(logging.Formatter("%(message)s"))
    r1 = logging.LogRecord("x", logging.INFO, "", 0, "0123456789", (), None)
    r2 = logging.LogRecord("x", logging.INFO, "", 0, "abcdefghij", (), None)
    rh2.emit(r1)
    rh2.emit(r2)
    rh2.close()
    print("namer backup exists:", os.path.exists(custom_path + ".1.bak"))


# ===========================================================================
# 3. TimedRotatingFileHandler & WatchedFileHandler
# ===========================================================================
print("=== 3. TimedRotatingFileHandler & WatchedFileHandler ===")
with tempfile.TemporaryDirectory() as tmpdir:
    tpath = os.path.join(tmpdir, "timed.log")
    th = logging.handlers.TimedRotatingFileHandler(
        tpath, when="S", interval=2, backupCount=2, utc=True
    )
    print("when/interval:", th.when, th.interval, th.utc)
    th.setFormatter(logging.Formatter("%(message)s"))
    th.emit(logging.LogRecord("t", logging.INFO, "", 0, "first", (), None))
    th.rolloverAt = 0
    th.emit(logging.LogRecord("t", logging.INFO, "", 0, "second", (), None))
    th.close()
    tfiles = sorted(os.listdir(tmpdir))
    print("timed rotated count >= 2:", len(tfiles) >= 2)

    try:
        logging.handlers.TimedRotatingFileHandler(
            os.path.join(tmpdir, "bad.log"), when="INVALID", delay=True
        )
        print("bad when: NO-ERR")
    except ValueError:
        print("bad when: ValueError")

    wpath = os.path.join(tmpdir, "watched.log")
    wh = logging.handlers.WatchedFileHandler(wpath, encoding="utf-8")
    wh.setFormatter(logging.Formatter("%(message)s"))
    wh.emit(logging.LogRecord("w", logging.INFO, "", 0, "line1", (), None))
    wh.close()
    with open(wpath, "r", encoding="utf-8") as f:
        print("watched content:", f.read().strip())


# ===========================================================================
# 4. BufferingHandler & MemoryHandler
# ===========================================================================
print("=== 4. BufferingHandler & MemoryHandler ===")
bh = logging.handlers.BufferingHandler(2)
bh.emit(logging.LogRecord("b", logging.INFO, "", 0, "a", (), None))
print("bh len after 1:", len(bh.buffer))
bh.emit(logging.LogRecord("b", logging.INFO, "", 0, "b", (), None))
print("bh len after 2 (flushed):", len(bh.buffer))
bh.close()

target_stream = io.StringIO()
target_h = logging.StreamHandler(target_stream)
target_h.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))
mh = logging.handlers.MemoryHandler(capacity=5, flushLevel=logging.ERROR, target=target_h)
mh.handle(logging.LogRecord("m", logging.INFO, "", 0, "buffered-1", (), None))
mh.handle(logging.LogRecord("m", logging.WARNING, "", 0, "buffered-2", (), None))
print("before error:", repr(target_stream.getvalue()), len(mh.buffer))
mh.handle(logging.LogRecord("m", logging.ERROR, "", 0, "trigger", (), None))
print("after error:", target_stream.getvalue().strip().splitlines(), len(mh.buffer))
mh.handle(logging.LogRecord("m", logging.INFO, "", 0, "on-close", (), None))
mh.close()
print("after close:", target_stream.getvalue().strip().splitlines()[-1])


# ===========================================================================
# 5. QueueHandler & QueueListener
# ===========================================================================
print("=== 5. QueueHandler & QueueListener ===")
q = queue.Queue()
qh = logging.handlers.QueueHandler(q)
q_out = io.StringIO()
q_target = logging.StreamHandler(q_out)
q_target.setLevel(logging.WARNING)
q_target.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))

listener = logging.handlers.QueueListener(q, q_target, respect_handler_level=True)
listener.start()
qlogger = logging.getLogger("test.queue")
qlogger.handlers.clear()
qlogger.propagate = False
qlogger.setLevel(logging.DEBUG)
qlogger.addHandler(qh)
qlogger.info("ignored-by-level")
qlogger.warning("kept-warn")
qlogger.error("kept-err %d", 42)
listener.stop()
print("queue listener lines:", q_out.getvalue().strip().splitlines())


# ===========================================================================
# 6. SocketHandler, DatagramHandler, SysLogHandler, HTTPHandler, SMTPHandler
# ===========================================================================
print("=== 6. network handlers ===")
sh = logging.handlers.SocketHandler("localhost", 9020)
rec = logging.LogRecord("net", logging.WARNING, "f.py", 10, "hi %s", ("world",), None)
framed = sh.makePickle(rec)
(plen,) = struct.unpack(">L", framed[:4])
unpickled = pickle.loads(framed[4:])
print("socket pickle:", plen == len(framed) - 4, unpickled["msg"], unpickled["levelname"])
sh.close()

dh = logging.handlers.DatagramHandler("localhost", 9021)
dgram = dh.makePickle(rec)
(dlen,) = struct.unpack(">L", dgram[:4])
print("dgram pickle:", dlen == len(dgram) - 4, pickle.loads(dgram[4:])["msg"])
dh.close()

sl = logging.handlers.SysLogHandler(address=("localhost", 514))
print("syslog encode:", sl.encodePriority("user", "warning"), sl.encodePriority(sl.LOG_USER, sl.LOG_ERR))
print("syslog map:", sl.mapPriority("ERROR"), sl.mapPriority("UNKNOWN"))
sl.close()

hh = logging.handlers.HTTPHandler("localhost:8080", "/log", method="POST")
print("http attrs:", hh.host, hh.url, hh.method, hh.mapLogRecord(rec)["levelname"])
try:
    logging.handlers.HTTPHandler("localhost", "/log", method="PUT")
    print("bad http method: NO-ERR")
except ValueError:
    print("bad http method: ValueError")
hh.close()

sm = logging.handlers.SMTPHandler(("mail.example", 2525), "a@b", "c@d", "Alert", timeout=3.0)
print("smtp attrs:", sm.mailhost, sm.mailport, sm.toaddrs, sm.getSubject(rec), sm.timeout)
sm.close()


# ===========================================================================
# 7. logging.config: valid_ident, Converting*, dictConfig, fileConfig
# ===========================================================================
print("=== 7. logging.config ===")
print("valid_ident ok:", logging.config.valid_ident("valid_name_1"))
for bad in ("123bad", "bad-ident", ""):
    try:
        logging.config.valid_ident(bad)
        print("valid_ident bad:", bad, "NO-ERR")
    except ValueError:
        print("valid_ident bad:", bad, "ValueError")

# ConvertingDict / ConvertingList / ConvertingTuple + ext:// + cfg://
cfg_stream = io.StringIO()


def _make_custom_handler(stream=None, tag="X"):
    h = logging.StreamHandler(stream)
    h._tag = tag
    return h


def _make_custom_filter(prefix=""):
    class _F(logging.Filter):
        def filter(self, record):
            return record.getMessage().startswith(prefix)

    return _F()


# Missing version / bad version
for bad_cfg in ({}, {"version": 2}):
    try:
        logging.config.dictConfig(bad_cfg)
        print("bad version: NO-ERR")
    except ValueError:
        print("bad version: ValueError")

# Full dictConfig with ext://, cfg://, custom () factory, MemoryHandler deferral, disable_existing_loggers
preexisting = logging.getLogger("test.preexisting")
preexisting.disabled = False

buf_a = io.StringIO()
buf_b = io.StringIO()
logging.config.dictConfig(
    {
        "version": 1,
        "disable_existing_loggers": True,
        "formatters": {
            "simple": {
                "format": "[%(levelname)s] %(name)s: %(message)s",
            },
            "braces": {
                "format": "{levelname}|{message}",
                "style": "{",
            },
        },
        "filters": {
            "only_ok": {
                "()": _make_custom_filter,
                "prefix": "OK:",
            },
        },
        "handlers": {
            "mem": {
                "class": "logging.handlers.MemoryHandler",
                "capacity": 10,
                "flushLevel": logging.ERROR,
                "target": "stream_b",
            },
            "stream_a": {
                "()": _make_custom_handler,
                "stream": buf_a,
                "tag": "custom",
                "level": "INFO",
                "formatter": "simple",
                "filters": ["only_ok"],
            },
            "stream_b": {
                "class": "logging.StreamHandler",
                "stream": buf_b,
                "level": "DEBUG",
                "formatter": "braces",
            },
        },
        "loggers": {
            "app.alpha": {
                "level": "DEBUG",
                "handlers": ["stream_a"],
                "propagate": False,
            },
            "app.beta": {
                "level": "DEBUG",
                "handlers": ["mem"],
                "propagate": False,
            },
        },
        "root": {
            "level": "WARNING",
            "handlers": [],
        },
    }
)

print("preexisting disabled:", preexisting.disabled)
la = logging.getLogger("app.alpha")
la.info("SKIP: hidden")
la.info("OK: visible")
print("stream_a:", buf_a.getvalue().strip())

lb = logging.getLogger("app.beta")
lb.info("queued-1")
print("stream_b before error:", repr(buf_b.getvalue()))
lb.error("boom-2")
print("stream_b after error:", buf_b.getvalue().strip().splitlines())

# Incremental dictConfig
logging.config.dictConfig(
    {
        "version": 1,
        "incremental": True,
        "loggers": {
            "app.alpha": {
                "level": "ERROR",
            },
        },
    }
)
print("incremental level:", la.level == logging.ERROR)

# cfg:// and ext:// resolution via DictConfigurator directly
dc = logging.config.DictConfigurator(
    {
        "version": 1,
        "vals": {"nums": [10, 20, 30]},
        "picked": "cfg://vals.nums[1]",
        "stdout_ref": "ext://sys.stdout",
    }
)
print("cfg_convert:", dc.config["picked"])
print("ext_convert is sys.stdout:", dc.config["stdout_ref"] is __import__("sys").stdout)

# fileConfig via StringIO
ini_text = """\
[loggers]
keys=root,sample

[handlers]
keys=nullh

[formatters]
keys=simple

[logger_root]
level=WARNING
handlers=nullh

[logger_sample]
level=DEBUG
handlers=nullh
qualname=filecfg.sample
propagate=0

[handler_nullh]
class=NullHandler
level=DEBUG
formatter=simple
args=()

[formatter_simple]
format=%(levelname)s - %(message)s
"""
logging.config.fileConfig(io.StringIO(ini_text), disable_existing_loggers=False)
fc_logger = logging.getLogger("filecfg.sample")
print(
    "fileConfig:",
    fc_logger.level == logging.DEBUG,
    fc_logger.propagate == 0,
    len(fc_logger.handlers) == 1,
    isinstance(fc_logger.handlers[0], logging.NullHandler),
    fc_logger.handlers[0].formatter._fmt,
)

_reset_logging()
print("DONE")
