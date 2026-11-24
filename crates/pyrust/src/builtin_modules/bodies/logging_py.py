"""
Logging package for Python. Based on PEP 282 and CPython 3.12's `logging`.

To use, simply 'import logging' and log away!
"""

import collections.abc
import io
import os
import re
import sys
import time
import traceback
import warnings
from string import Formatter as StrFormatter
from string import Template
from types import GenericAlias

__all__ = [
    'BASIC_FORMAT', 'BufferingFormatter', 'CRITICAL', 'DEBUG', 'ERROR',
    'FATAL', 'FileHandler', 'Filter', 'Formatter', 'Handler', 'INFO',
    'LogRecord', 'Logger', 'LoggerAdapter', 'NOTSET', 'NullHandler',
    'StreamHandler', 'WARN', 'WARNING', 'addLevelName', 'basicConfig',
    'captureWarnings', 'critical', 'debug', 'disable', 'error',
    'exception', 'fatal', 'getLevelName', 'getLogger', 'getLoggerClass',
    'info', 'log', 'makeLogRecord', 'setLoggerClass', 'shutdown',
    'warn', 'warning', 'getLogRecordFactory', 'setLogRecordFactory',
    'lastResort', 'raiseExceptions', 'getLevelNamesMapping',
    'getHandlerByName', 'getHandlerNames',
]

__author__ = "Vinay Sajip <vinay_sajip@red-dove.com>"
__status__ = "production"
__version__ = "0.5.1.2"
__date__ = "07 February 2010"

# ---------------------------------------------------------------------------
# Miscellaneous module data
# ---------------------------------------------------------------------------

_startTime = time.time()

raiseExceptions = True
logThreads = True
logMultiprocessing = True
logProcesses = True
logAsyncioTasks = True


def _mod_attr(name, fallback):
    """Read a potentially user-rebound attribute from the live `logging` module."""
    mod = sys.modules.get("logging")
    if mod is not None:
        return getattr(mod, name, fallback)
    return fallback


def _set_mod_attr(name, value):
    """Mirror an internal global update onto the live `logging` module."""
    mod = sys.modules.get("logging")
    if mod is not None:
        setattr(mod, name, value)


def _text_encoding(encoding, stacklevel=2):
    """Equivalent to CPython 3.12 `io.text_encoding(encoding)`."""
    if encoding is None:
        if getattr(sys.flags, "utf8_mode", 0):
            return "utf-8"
        return "locale"
    return encoding


# ---------------------------------------------------------------------------
# Level related stuff
# ---------------------------------------------------------------------------

CRITICAL = 50
FATAL = CRITICAL
ERROR = 40
WARNING = 30
WARN = WARNING
INFO = 20
DEBUG = 10
NOTSET = 0

_levelToName = {
    CRITICAL: 'CRITICAL',
    ERROR: 'ERROR',
    WARNING: 'WARNING',
    INFO: 'INFO',
    DEBUG: 'DEBUG',
    NOTSET: 'NOTSET',
}
_nameToLevel = {
    'CRITICAL': CRITICAL,
    'FATAL': FATAL,
    'ERROR': ERROR,
    'WARN': WARNING,
    'WARNING': WARNING,
    'INFO': INFO,
    'DEBUG': DEBUG,
    'NOTSET': NOTSET,
}


def getLevelNamesMapping():
    return _nameToLevel.copy()


def getLevelName(level):
    """
    Return the textual or numeric representation of logging level 'level'.
    """
    result = _levelToName.get(level)
    if result is not None:
        return result
    result = _nameToLevel.get(level)
    if result is not None:
        return result
    return "Level %s" % level


def addLevelName(level, levelName):
    """
    Associate 'levelName' with 'level'.
    """
    _acquireLock()
    try:
        _levelToName[level] = levelName
        _nameToLevel[levelName] = level
    finally:
        _releaseLock()


if hasattr(sys, "_getframe"):
    currentframe = lambda: sys._getframe(1)
else:
    def currentframe():
        """Return the frame object for the caller's stack frame."""
        try:
            raise Exception
        except Exception as exc:
            return exc.__traceback__.tb_frame.f_back


_srcfile = os.path.normcase(addLevelName.__code__.co_filename)


def _is_internal_frame(frame):
    """Signal whether the frame is a CPython or logging module internal."""
    filename = os.path.normcase(frame.f_code.co_filename)
    srcfile = _mod_attr("_srcfile", _srcfile)
    return (srcfile is not None and filename == srcfile) or (
        "importlib" in filename and "_bootstrap" in filename
    )


def _checkLevel(level):
    if isinstance(level, int):
        rv = level
    elif str(level) == level:
        if level not in _nameToLevel:
            raise ValueError("Unknown level: %r" % level)
        rv = _nameToLevel[level]
    else:
        raise TypeError("Level not an integer or a valid string: %r"
                        % (level,))
    return rv


# ---------------------------------------------------------------------------
# Thread-related stuff (single-threaded re-entrant lock shim)
# ---------------------------------------------------------------------------

class _RLock:
    def __init__(self):
        self._count = 0

    def acquire(self, blocking=True, timeout=-1):
        self._count += 1
        return True

    def release(self):
        if self._count <= 0:
            raise RuntimeError("cannot release un-acquired lock")
        self._count -= 1

    def __enter__(self):
        self.acquire()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.release()
        return False

    def _at_fork_reinit(self):
        self._count = 0


_lock = _RLock()


def _acquireLock():
    """
    Acquire the module-level lock for serializing access to shared data.
    """
    if _lock:
        _lock.acquire()


def _releaseLock():
    """
    Release the module-level lock acquired by calling _acquireLock().
    """
    if _lock:
        _lock.release()


def _register_at_fork_reinit_lock(instance):
    pass


# ---------------------------------------------------------------------------
# The logging record
# ---------------------------------------------------------------------------

class LogRecord(object):
    """
    A LogRecord instance represents an event being logged.
    """
    def __init__(self, name, level, pathname, lineno,
                 msg, args, exc_info, func=None, sinfo=None, **kwargs):
        ct = time.time()
        self.name = name
        self.msg = msg
        if (args and len(args) == 1 and isinstance(args[0], collections.abc.Mapping)
                and args[0]):
            args = args[0]
        self.args = args
        self.levelname = getLevelName(level)
        self.levelno = level
        self.pathname = pathname
        try:
            self.filename = os.path.basename(pathname)
            self.module = os.path.splitext(self.filename)[0]
        except (TypeError, ValueError, AttributeError):
            self.filename = pathname
            self.module = "Unknown module"
        self.exc_info = exc_info
        self.exc_text = None
        self.stack_info = sinfo
        self.lineno = lineno
        self.funcName = func
        self.created = ct
        self.msecs = int((ct - int(ct)) * 1000) + 0.0
        self.relativeCreated = (self.created - _startTime) * 1000
        if _mod_attr("logThreads", logThreads):
            self.thread = 1
            self.threadName = 'MainThread'
        else:
            self.thread = None
            self.threadName = None
        if not _mod_attr("logMultiprocessing", logMultiprocessing):
            self.processName = None
        else:
            self.processName = 'MainProcess'
            mp = sys.modules.get('multiprocessing')
            if mp is not None:
                try:
                    self.processName = mp.current_process().name
                except Exception:
                    pass
        if _mod_attr("logProcesses", logProcesses) and hasattr(os, 'getpid'):
            self.process = os.getpid()
        else:
            self.process = None

        self.taskName = None
        if _mod_attr("logAsyncioTasks", logAsyncioTasks):
            asyncio_mod = sys.modules.get('asyncio')
            if asyncio_mod:
                try:
                    self.taskName = asyncio_mod.current_task().get_name()
                except Exception:
                    pass

    def __repr__(self):
        return '<LogRecord: %s, %s, %s, %s, "%s">' % (
            self.name, self.levelno, self.pathname, self.lineno, self.msg
        )

    def getMessage(self):
        """
        Return the message for this LogRecord after merging any user-supplied
        arguments with the message.
        """
        msg = str(self.msg)
        if self.args:
            msg = msg % self.args
        return msg


_logRecordFactory = LogRecord


def setLogRecordFactory(factory):
    """
    Set the factory to be used when instantiating a log record.
    """
    global _logRecordFactory
    _logRecordFactory = factory
    _set_mod_attr("_logRecordFactory", factory)


def getLogRecordFactory():
    """
    Return the factory to be used when instantiating a log record.
    """
    return _logRecordFactory


def makeLogRecord(dict):
    """
    Make a LogRecord whose attributes are defined by the specified dictionary.
    """
    rv = _logRecordFactory(None, None, "", 0, "", (), None, None)
    rv.__dict__.update(dict)
    return rv


# ---------------------------------------------------------------------------
# Formatter classes and functions
# ---------------------------------------------------------------------------

_str_formatter = StrFormatter()
del StrFormatter


def _is_ident_start(ch):
    return ch == '_' or ('a' <= ch <= 'z') or ('A' <= ch <= 'Z')


def _is_ident_cont(ch):
    return _is_ident_start(ch) or ('0' <= ch <= '9')


class PercentStyle(object):

    default_format = '%(message)s'
    asctime_format = '%(asctime)s'
    asctime_search = '%(asctime)'
    validation_pattern = re.compile(
        r'%\(\w+\)[#0+ -]*(\*|\d+)?(\.(\*|\d+))?[diouxefgcrsa%]', re.I
    )

    def __init__(self, fmt, *, defaults=None):
        self._fmt = fmt or self.default_format
        self._defaults = defaults

    def usesTime(self):
        return self._fmt.find(self.asctime_search) >= 0

    def validate(self):
        """Validate the input format, ensure it matches the correct style."""
        if not self.validation_pattern.search(self._fmt):
            raise ValueError(
                "Invalid format '%s' for '%s' style"
                % (self._fmt, self.default_format[0])
            )

    def _values(self, record):
        values = dict(record.__dict__.items())
        if defaults := self._defaults:
            merged = dict(defaults.items()) if hasattr(defaults, 'items') else dict(defaults)
            merged.update(values)
            return merged
        return values

    def _format(self, record):
        values = self._values(record)
        if '%' not in self._fmt.replace('%%', ''):
            return self._fmt.replace('%%', '%')
        return self._fmt % values

    def format(self, record):
        try:
            return self._format(record)
        except KeyError as e:
            raise ValueError('Formatting field not found in record: %s' % e)


class StrFormatStyle(PercentStyle):
    default_format = '{message}'
    asctime_format = '{asctime}'
    asctime_search = '{asctime'

    fmt_spec = re.compile(
        r'^(.?[<>=^])?[+ -]?#?0?(\d+|\{\w+\})?[,_]?(\.(\d+|\{\w+\}))?[bcdefgnosx%]?$',
        re.I,
    )
    field_spec = re.compile(r'^(\d+|\w+)(\.\w+|\[[^\]]+\])*$')

    def _format(self, record):
        return self._fmt.format(**self._values(record))

    def validate(self):
        """Validate the input format, ensure it is the correct string formatting style."""
        fields = set()
        try:
            for _, fieldname, spec, conversion in _str_formatter.parse(self._fmt):
                if fieldname:
                    if not self.field_spec.match(fieldname):
                        raise ValueError('invalid field name/expression: %r' % fieldname)
                    fields.add(fieldname)
                if conversion and conversion not in 'rsa':
                    raise ValueError('invalid conversion: %r' % conversion)
                if spec and not self.fmt_spec.match(spec):
                    raise ValueError('bad specifier: %r' % spec)
        except ValueError as e:
            raise ValueError('invalid format: %s' % e)
        if not fields:
            raise ValueError('invalid format: no fields')


class StringTemplateStyle(PercentStyle):
    default_format = '${message}'
    asctime_format = '${asctime}'
    asctime_search = '${asctime}'

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._tpl = Template(self._fmt)

    def usesTime(self):
        fmt = self._fmt
        return fmt.find('$asctime') >= 0 or fmt.find(self.asctime_search) >= 0

    def validate(self):
        fmt = self._fmt
        fields = set()
        i = 0
        n = len(fmt)
        while i < n:
            if fmt[i] != '$':
                i += 1
                continue
            j = i + 1
            if j >= n:
                raise ValueError("invalid format: bare '$' not allowed")
            nxt = fmt[j]
            if nxt == '$':
                i = j + 1
                continue
            if nxt == '{':
                k = j + 1
                if k < n and _is_ident_start(fmt[k]):
                    k += 1
                    while k < n and _is_ident_cont(fmt[k]):
                        k += 1
                    if k < n and fmt[k] == '}':
                        fields.add(fmt[j + 1:k])
                        i = k + 1
                        continue
                raise ValueError("invalid format: bare '$' not allowed")
            if _is_ident_start(nxt):
                k = j + 1
                while k < n and _is_ident_cont(fmt[k]):
                    k += 1
                fields.add(fmt[j:k])
                i = k
                continue
            raise ValueError("invalid format: bare '$' not allowed")
        if not fields:
            raise ValueError('invalid format: no fields')

    def _format(self, record):
        return self._tpl.substitute(**self._values(record))


BASIC_FORMAT = "%(levelname)s:%(name)s:%(message)s"

_STYLES = {
    '%': (PercentStyle, BASIC_FORMAT),
    '{': (StrFormatStyle, '{levelname}:{name}:{message}'),
    '$': (StringTemplateStyle, '${levelname}:${name}:${message}'),
}


class Formatter(object):
    """
    Formatter instances are used to convert a LogRecord to text.
    """

    converter = time.localtime

    def __init__(self, fmt=None, datefmt=None, style='%', validate=True, *,
                 defaults=None):
        if style not in _STYLES:
            raise ValueError('Style must be one of: %s' % ','.join(
                _STYLES.keys()))
        self._style = _STYLES[style][0](fmt, defaults=defaults)
        if validate:
            self._style.validate()

        self._fmt = self._style._fmt
        self.datefmt = datefmt

    default_time_format = '%Y-%m-%d %H:%M:%S'
    default_msec_format = '%s,%03d'

    def formatTime(self, record, datefmt=None):
        """
        Return the creation time of the specified LogRecord as formatted text.
        """
        ct = self.converter(record.created)
        if datefmt:
            s = time.strftime(datefmt, ct)
        else:
            s = time.strftime(self.default_time_format, ct)
            if self.default_msec_format:
                s = self.default_msec_format % (s, record.msecs)
        return s

    def formatException(self, ei):
        """
        Format and return the specified exception information as a string.
        """
        sio = io.StringIO()
        tb = ei[2]
        traceback.print_exception(ei[0], ei[1], tb, None, sio)
        s = sio.getvalue()
        sio.close()
        if s[-1:] == "\n":
            s = s[:-1]
        return s

    def usesTime(self):
        """
        Check if the format uses the creation time of the record.
        """
        return self._style.usesTime()

    def formatMessage(self, record):
        return self._style.format(record)

    def formatStack(self, stack_info):
        """
        Format stack information string.
        """
        return stack_info

    def format(self, record):
        """
        Format the specified record as text.
        """
        record.message = record.getMessage()
        if self.usesTime():
            record.asctime = self.formatTime(record, self.datefmt)
        s = self.formatMessage(record)
        if record.exc_info:
            if not record.exc_text:
                record.exc_text = self.formatException(record.exc_info)
        if record.exc_text:
            if s[-1:] != "\n":
                s = s + "\n"
            s = s + record.exc_text
        if record.stack_info:
            if s[-1:] != "\n":
                s = s + "\n"
            s = s + self.formatStack(record.stack_info)
        return s


_defaultFormatter = Formatter()


class BufferingFormatter(object):
    """
    A formatter suitable for formatting a number of records.
    """
    def __init__(self, linefmt=None):
        if linefmt:
            self.linefmt = linefmt
        else:
            self.linefmt = _defaultFormatter

    def formatHeader(self, records):
        return ""

    def formatFooter(self, records):
        return ""

    def format(self, records):
        rv = ""
        if len(records) > 0:
            rv = rv + self.formatHeader(records)
            for record in records:
                rv = rv + self.linefmt.format(record)
            rv = rv + self.formatFooter(records)
        return rv


# ---------------------------------------------------------------------------
# Filter classes and functions
# ---------------------------------------------------------------------------

class Filter(object):
    """
    Filter instances are used to perform arbitrary filtering of LogRecords.
    """
    def __init__(self, name=''):
        self.name = name
        self.nlen = len(name)

    def filter(self, record):
        if self.nlen == 0:
            return True
        elif self.name == record.name:
            return True
        elif record.name.find(self.name, 0, self.nlen) != 0:
            return False
        return record.name[self.nlen] == "."


class Filterer(object):
    """
    A base class for loggers and handlers which allows them to share
    common code.
    """
    def __init__(self):
        self.filters = []

    def addFilter(self, filter):
        if not (filter in self.filters):
            self.filters.append(filter)

    def removeFilter(self, filter):
        if filter in self.filters:
            self.filters.remove(filter)

    def filter(self, record):
        for f in self.filters:
            if hasattr(f, 'filter'):
                result = f.filter(record)
            else:
                result = f(record)
            if not result:
                return False
            if isinstance(result, LogRecord):
                record = result
        return record


# ---------------------------------------------------------------------------
# Handler classes and functions
# ---------------------------------------------------------------------------

_handlers = {}
_handlerList = []


class _HandlerRef:
    """Callable handler reference compatible with CPython's shutdown() contract."""
    def __init__(self, handler):
        self._handler = handler

    def __call__(self):
        return self._handler


def _addHandlerRef(handler):
    _acquireLock()
    try:
        _handlerList.append(_HandlerRef(handler))
    finally:
        _releaseLock()


def getHandlerByName(name):
    """
    Get a handler with the specified *name*, or None if there isn't one with
    that name.
    """
    return _handlers.get(name)


def getHandlerNames():
    """
    Return all known handler names as an immutable set.
    """
    result = set(_handlers.keys())
    return frozenset(result)


class Handler(Filterer):
    """
    Handler instances dispatch logging events to specific destinations.
    """
    def __init__(self, level=NOTSET):
        Filterer.__init__(self)
        self._name = None
        self.level = _checkLevel(level)
        self.formatter = None
        self._closed = False
        _addHandlerRef(self)
        self.createLock()

    def get_name(self):
        return self._name

    def set_name(self, name):
        _acquireLock()
        try:
            if self._name in _handlers:
                del _handlers[self._name]
            self._name = name
            if name:
                _handlers[name] = self
        finally:
            _releaseLock()

    name = property(get_name, set_name)

    def createLock(self):
        self.lock = _RLock()
        _register_at_fork_reinit_lock(self)

    def _at_fork_reinit(self):
        if self.lock:
            self.lock._at_fork_reinit()

    def acquire(self):
        if self.lock:
            self.lock.acquire()

    def release(self):
        if self.lock:
            self.lock.release()

    def setLevel(self, level):
        self.level = _checkLevel(level)

    def format(self, record):
        if self.formatter:
            fmt = self.formatter
        else:
            fmt = _defaultFormatter
        return fmt.format(record)

    def emit(self, record):
        raise NotImplementedError('emit must be implemented '
                                  'by Handler subclasses')

    def handle(self, record):
        rv = self.filter(record)
        if isinstance(rv, LogRecord):
            record = rv
        if rv:
            self.acquire()
            try:
                self.emit(record)
            finally:
                self.release()
        return rv

    def setFormatter(self, fmt):
        self.formatter = fmt

    def flush(self):
        pass

    def close(self):
        _acquireLock()
        try:
            self._closed = True
            if self._name and self._name in _handlers:
                del _handlers[self._name]
        finally:
            _releaseLock()

    def handleError(self, record):
        if _mod_attr("raiseExceptions", raiseExceptions) and sys.stderr:
            t, v, tb = sys.exc_info()
            try:
                sys.stderr.write('--- Logging error ---\n')
                traceback.print_exception(t, v, tb, None, sys.stderr)
                sys.stderr.write('Call stack:\n')
                frame = tb.tb_frame if tb is not None else None
                while frame and _is_internal_frame(frame):
                    frame = frame.f_back
                if frame:
                    traceback.print_stack(frame, file=sys.stderr)
                else:
                    sys.stderr.write('Logged from file %s, line %s\n' % (
                        record.filename, record.lineno))
                try:
                    sys.stderr.write('Message: %r\n'
                                     'Arguments: %s\n' % (record.msg,
                                                          record.args))
                except RecursionError:
                    raise
                except Exception:
                    sys.stderr.write(
                        'Unable to print the message and arguments'
                        ' - possible formatting error.\nUse the'
                        ' traceback above to help find the error.\n'
                    )
            except OSError:
                pass
            finally:
                del t, v, tb

    def __repr__(self):
        level = getLevelName(self.level)
        return '<%s (%s)>' % (self.__class__.__name__, level)


class StreamHandler(Handler):
    """
    A handler class which writes logging records, appropriately formatted,
    to a stream.
    """

    terminator = '\n'

    def __init__(self, stream=None):
        Handler.__init__(self)
        if stream is None:
            stream = sys.stderr
        self.stream = stream

    def flush(self):
        self.acquire()
        try:
            if self.stream and hasattr(self.stream, "flush"):
                self.stream.flush()
        finally:
            self.release()

    def emit(self, record):
        try:
            msg = self.format(record)
            stream = self.stream
            stream.write(msg + self.terminator)
            self.flush()
        except RecursionError:
            raise
        except Exception:
            self.handleError(record)

    def setStream(self, stream):
        if stream is self.stream:
            result = None
        else:
            result = self.stream
            self.acquire()
            try:
                self.flush()
                self.stream = stream
            finally:
                self.release()
        return result

    def __repr__(self):
        level = getLevelName(self.level)
        name = getattr(self.stream, 'name', '')
        name = str(name)
        if name:
            name += ' '
        return '<%s %s(%s)>' % (self.__class__.__name__, name, level)

    __class_getitem__ = classmethod(GenericAlias)


class FileHandler(StreamHandler):
    """
    A handler class which writes formatted logging records to disk files.
    """
    def __init__(self, filename, mode='a', encoding=None, delay=False, errors=None):
        filename = os.fspath(filename)
        self.baseFilename = os.path.abspath(filename)
        self.mode = mode
        self.encoding = encoding
        if "b" not in mode:
            self.encoding = _text_encoding(encoding)
        self.errors = errors
        self.delay = delay
        self._builtin_open = _open_log_file
        if delay:
            Handler.__init__(self)
            self.stream = None
        else:
            StreamHandler.__init__(self, self._open())

    def close(self):
        self.acquire()
        try:
            try:
                if self.stream:
                    try:
                        self.flush()
                    finally:
                        stream = self.stream
                        self.stream = None
                        if hasattr(stream, "close"):
                            stream.close()
            finally:
                StreamHandler.close(self)
        finally:
            self.release()

    def _open(self):
        open_func = self._builtin_open
        return open_func(self.baseFilename, self.mode,
                         encoding=self.encoding, errors=self.errors)

    def emit(self, record):
        if self.stream is None:
            if self.mode != 'w' or not self._closed:
                self.stream = self._open()
        if self.stream:
            StreamHandler.emit(self, record)

    def __repr__(self):
        level = getLevelName(self.level)
        return '<%s %s (%s)>' % (self.__class__.__name__, self.baseFilename, level)


class _StderrHandler(StreamHandler):
    """
    A StreamHandler that always delegates to the current `sys.stderr`.
    """
    def __init__(self, level=NOTSET):
        Handler.__init__(self, level)

    @property
    def stream(self):
        return sys.stderr


_defaultLastResort = _StderrHandler(WARNING)
lastResort = _defaultLastResort


# ---------------------------------------------------------------------------
# Manager classes and functions
# ---------------------------------------------------------------------------

class PlaceHolder(object):
    """
    PlaceHolder instances are used in the Manager logger hierarchy to take
    the place of nodes for which no loggers have been defined.
    """
    def __init__(self, alogger):
        self.loggerMap = {alogger: None}

    def append(self, alogger):
        if alogger not in self.loggerMap:
            self.loggerMap[alogger] = None


def setLoggerClass(klass):
    """
    Set the class to be used when instantiating a logger.
    """
    if klass != Logger:
        if not issubclass(klass, Logger):
            raise TypeError("logger not derived from logging.Logger: "
                            + klass.__name__)
    global _loggerClass
    _loggerClass = klass
    _set_mod_attr("_loggerClass", klass)


def getLoggerClass():
    """
    Return the class to be used when instantiating a logger.
    """
    return _loggerClass


class Manager(object):
    """
    Holds the hierarchy of loggers.
    """
    def __init__(self, rootnode):
        self.root = rootnode
        self.disable = 0
        self.emittedNoHandlerWarning = False
        self.loggerDict = {}
        self.loggerClass = None
        self.logRecordFactory = None

    @property
    def disable(self):
        return self._disable

    @disable.setter
    def disable(self, value):
        self._disable = _checkLevel(value)

    def getLogger(self, name):
        rv = None
        if not isinstance(name, str):
            raise TypeError('A logger name must be a string')
        _acquireLock()
        try:
            if name in self.loggerDict:
                rv = self.loggerDict[name]
                if isinstance(rv, PlaceHolder):
                    ph = rv
                    rv = (self.loggerClass or _loggerClass)(name)
                    rv.manager = self
                    self.loggerDict[name] = rv
                    self._fixupChildren(ph, rv)
                    self._fixupParents(rv)
            else:
                rv = (self.loggerClass or _loggerClass)(name)
                rv.manager = self
                self.loggerDict[name] = rv
                self._fixupParents(rv)
        finally:
            _releaseLock()
        return rv

    def setLoggerClass(self, klass):
        if klass != Logger:
            if not issubclass(klass, Logger):
                raise TypeError("logger not derived from logging.Logger: "
                                + klass.__name__)
        self.loggerClass = klass

    def setLogRecordFactory(self, factory):
        self.logRecordFactory = factory

    def _fixupParents(self, alogger):
        name = alogger.name
        i = name.rfind(".")
        rv = None
        while (i > 0) and not rv:
            substr = name[:i]
            if substr not in self.loggerDict:
                self.loggerDict[substr] = PlaceHolder(alogger)
            else:
                obj = self.loggerDict[substr]
                if isinstance(obj, Logger):
                    rv = obj
                else:
                    assert isinstance(obj, PlaceHolder)
                    obj.append(alogger)
            i = name.rfind(".", 0, i - 1)
        if not rv:
            rv = self.root
        alogger.parent = rv

    def _fixupChildren(self, ph, alogger):
        name = alogger.name
        namelen = len(name)
        for c in ph.loggerMap.keys():
            if c.parent.name[:namelen] != name:
                alogger.parent = c.parent
                c.parent = alogger

    def _clear_cache(self):
        _acquireLock()
        try:
            for logger in self.loggerDict.values():
                if isinstance(logger, Logger):
                    logger._cache.clear()
            self.root._cache.clear()
        finally:
            _releaseLock()


# ---------------------------------------------------------------------------
# Logger classes and functions
# ---------------------------------------------------------------------------

class Logger(Filterer):
    """
    Instances of the Logger class represent a single logging channel.
    """
    def __init__(self, name, level=NOTSET):
        Filterer.__init__(self)
        self.name = name
        self.level = _checkLevel(level)
        self.parent = None
        self.propagate = True
        self.handlers = []
        self.disabled = False
        self._cache = {}

    def setLevel(self, level):
        self.level = _checkLevel(level)
        self.manager._clear_cache()

    def debug(self, msg, *args, **kwargs):
        if self.isEnabledFor(DEBUG):
            self._log(DEBUG, msg, args, **kwargs)

    def info(self, msg, *args, **kwargs):
        if self.isEnabledFor(INFO):
            self._log(INFO, msg, args, **kwargs)

    def warning(self, msg, *args, **kwargs):
        if self.isEnabledFor(WARNING):
            self._log(WARNING, msg, args, **kwargs)

    def warn(self, msg, *args, **kwargs):
        warnings.warn("The 'warn' method is deprecated, "
                      "use 'warning' instead", DeprecationWarning, 2)
        self.warning(msg, *args, **kwargs)

    def error(self, msg, *args, **kwargs):
        if self.isEnabledFor(ERROR):
            self._log(ERROR, msg, args, **kwargs)

    def exception(self, msg, *args, exc_info=True, **kwargs):
        self.error(msg, *args, exc_info=exc_info, **kwargs)

    def critical(self, msg, *args, **kwargs):
        if self.isEnabledFor(CRITICAL):
            self._log(CRITICAL, msg, args, **kwargs)

    def fatal(self, msg, *args, **kwargs):
        self.critical(msg, *args, **kwargs)

    def log(self, level, msg, *args, **kwargs):
        if not isinstance(level, int):
            if _mod_attr("raiseExceptions", raiseExceptions):
                raise TypeError("level must be an integer")
            else:
                return
        if self.isEnabledFor(level):
            self._log(level, msg, args, **kwargs)

    def findCaller(self, stack_info=False, stacklevel=1):
        f = currentframe()
        if f is None:
            return "(unknown file)", 0, "(unknown function)", None
        depth = 0
        while stacklevel > 0:
            next_f = f.f_back
            if next_f is None:
                break
            f = next_f
            depth += 1
            if not _is_internal_frame(f):
                stacklevel -= 1
        if f.f_lineno == 0 and depth > 0 and hasattr(sys, "_getframe"):
            try:
                f2 = sys._getframe(depth)
                if (f2.f_code.co_filename == f.f_code.co_filename
                        and f2.f_code.co_name == f.f_code.co_name
                        and f2.f_lineno):
                    f = f2
            except ValueError:
                pass
        co = f.f_code
        sinfo = None
        if stack_info:
            with io.StringIO() as sio:
                sio.write("Stack (most recent call last):\n")
                traceback.print_stack(f, file=sio)
                sinfo = sio.getvalue()
                if sinfo[-1:] == '\n':
                    sinfo = sinfo[:-1]
        return co.co_filename, f.f_lineno, co.co_name, sinfo

    def makeRecord(self, name, level, fn, lno, msg, args, exc_info,
                   func=None, extra=None, sinfo=None):
        rv = _logRecordFactory(name, level, fn, lno, msg, args, exc_info, func,
                               sinfo)
        if extra is not None:
            for key in extra:
                if (key in ["message", "asctime"]) or (key in rv.__dict__):
                    raise KeyError("Attempt to overwrite %r in LogRecord" % key)
                rv.__dict__[key] = extra[key]
        return rv

    def _log(self, level, msg, args, exc_info=None, extra=None, stack_info=False,
             stacklevel=1):
        sinfo = None
        if _mod_attr("_srcfile", _srcfile):
            try:
                fn, lno, func, sinfo = self.findCaller(stack_info, stacklevel)
            except ValueError:
                fn, lno, func = "(unknown file)", 0, "(unknown function)"
        else:
            fn, lno, func = "(unknown file)", 0, "(unknown function)"
        if exc_info:
            if isinstance(exc_info, BaseException):
                exc_info = (type(exc_info), exc_info, exc_info.__traceback__)
            elif not isinstance(exc_info, tuple):
                exc_info = sys.exc_info()
        record = self.makeRecord(self.name, level, fn, lno, msg, args,
                                 exc_info, func, extra, sinfo)
        self.handle(record)

    def handle(self, record):
        if self.disabled:
            return
        maybe_record = self.filter(record)
        if not maybe_record:
            return
        if isinstance(maybe_record, LogRecord):
            record = maybe_record
        self.callHandlers(record)

    def addHandler(self, hdlr):
        _acquireLock()
        try:
            if not (hdlr in self.handlers):
                self.handlers.append(hdlr)
        finally:
            _releaseLock()

    def removeHandler(self, hdlr):
        _acquireLock()
        try:
            if hdlr in self.handlers:
                self.handlers.remove(hdlr)
        finally:
            _releaseLock()

    def hasHandlers(self):
        c = self
        rv = False
        while c:
            if c.handlers:
                rv = True
                break
            if not c.propagate:
                break
            else:
                c = c.parent
        return rv

    def callHandlers(self, record):
        c = self
        found = 0
        while c:
            for hdlr in c.handlers:
                found = found + 1
                if record.levelno >= hdlr.level:
                    hdlr.handle(record)
            if not c.propagate:
                c = None
            else:
                c = c.parent
        if found == 0:
            lr = _mod_attr("lastResort", lastResort)
            if lr:
                if record.levelno >= lr.level:
                    lr.handle(record)
            elif (_mod_attr("raiseExceptions", raiseExceptions)
                  and not self.manager.emittedNoHandlerWarning):
                sys.stderr.write('No handlers could be found for logger'
                                 ' "%s"\n' % self.name)
                self.manager.emittedNoHandlerWarning = True

    def getEffectiveLevel(self):
        logger = self
        while logger:
            if logger.level:
                return logger.level
            logger = logger.parent
        return NOTSET

    def isEnabledFor(self, level):
        if self.disabled:
            return False

        try:
            return self._cache[level]
        except KeyError:
            _acquireLock()
            try:
                if self.manager.disable >= level:
                    is_enabled = self._cache[level] = False
                else:
                    is_enabled = self._cache[level] = (
                        level >= self.getEffectiveLevel()
                    )
            finally:
                _releaseLock()
            return is_enabled

    def getChild(self, suffix):
        if self.root is not self:
            suffix = '.'.join((self.name, suffix))
        return self.manager.getLogger(suffix)

    def getChildren(self):
        def _hierlevel(logger):
            if logger is logger.manager.root:
                return 0
            return 1 + logger.name.count('.')

        d = self.manager.loggerDict
        _acquireLock()
        try:
            return set(
                item for item in d.values()
                if isinstance(item, Logger) and item.parent is self
                and _hierlevel(item) == 1 + _hierlevel(item.parent)
            )
        finally:
            _releaseLock()

    def __repr__(self):
        level = getLevelName(self.getEffectiveLevel())
        return '<%s %s (%s)>' % (self.__class__.__name__, self.name, level)


class RootLogger(Logger):
    """
    The root logger of the logging hierarchy.
    """
    def __init__(self, level):
        Logger.__init__(self, "root", level)


_loggerClass = Logger


class LoggerAdapter(object):
    """
    An adapter for loggers which makes it easier to specify contextual
    information in logging output.
    """

    def __init__(self, logger, extra=None):
        self.logger = logger
        self.extra = extra

    def process(self, msg, kwargs):
        kwargs["extra"] = self.extra
        return msg, kwargs

    def debug(self, msg, *args, **kwargs):
        self.log(DEBUG, msg, *args, **kwargs)

    def info(self, msg, *args, **kwargs):
        self.log(INFO, msg, *args, **kwargs)

    def warning(self, msg, *args, **kwargs):
        self.log(WARNING, msg, *args, **kwargs)

    def warn(self, msg, *args, **kwargs):
        warnings.warn("The 'warn' method is deprecated, "
                      "use 'warning' instead", DeprecationWarning, 2)
        self.warning(msg, *args, **kwargs)

    def error(self, msg, *args, **kwargs):
        self.log(ERROR, msg, *args, **kwargs)

    def exception(self, msg, *args, exc_info=True, **kwargs):
        self.log(ERROR, msg, *args, exc_info=exc_info, **kwargs)

    def critical(self, msg, *args, **kwargs):
        self.log(CRITICAL, msg, *args, **kwargs)

    def log(self, level, msg, *args, **kwargs):
        if self.isEnabledFor(level):
            msg, kwargs = self.process(msg, kwargs)
            self.logger.log(level, msg, *args, **kwargs)

    def isEnabledFor(self, level):
        return self.logger.isEnabledFor(level)

    def setLevel(self, level):
        self.logger.setLevel(level)

    def getEffectiveLevel(self):
        return self.logger.getEffectiveLevel()

    def hasHandlers(self):
        return self.logger.hasHandlers()

    def _log(self, level, msg, args, **kwargs):
        return self.logger._log(level, msg, args, **kwargs)

    @property
    def manager(self):
        return self.logger.manager

    @manager.setter
    def manager(self, value):
        self.logger.manager = value

    @property
    def name(self):
        return self.logger.name

    def __repr__(self):
        logger = self.logger
        level = getLevelName(logger.getEffectiveLevel())
        return '<%s %s (%s)>' % (self.__class__.__name__, logger.name, level)

    __class_getitem__ = classmethod(GenericAlias)


root = RootLogger(WARNING)
Logger.root = root
Logger.manager = Manager(Logger.root)


# ---------------------------------------------------------------------------
# Configuration functions
# ---------------------------------------------------------------------------

def basicConfig(**kwargs):
    """
    Do basic configuration for the logging system.
    """
    _acquireLock()
    try:
        r = _mod_attr("root", root)
        force = kwargs.pop('force', False)
        encoding = kwargs.pop('encoding', None)
        errors = kwargs.pop('errors', 'backslashreplace')
        if force:
            for h in r.handlers[:]:
                r.removeHandler(h)
                h.close()
        if len(r.handlers) == 0:
            handlers = kwargs.pop("handlers", None)
            if handlers is None:
                if "stream" in kwargs and "filename" in kwargs:
                    raise ValueError("'stream' and 'filename' should not be "
                                     "specified together")
            else:
                if "stream" in kwargs or "filename" in kwargs:
                    raise ValueError("'stream' or 'filename' should not be "
                                     "specified together with 'handlers'")
            if handlers is None:
                filename = kwargs.pop("filename", None)
                mode = kwargs.pop("filemode", 'a')
                if filename:
                    if 'b' in mode:
                        errors = None
                    else:
                        encoding = _text_encoding(encoding)
                    h = FileHandler(filename, mode,
                                    encoding=encoding, errors=errors)
                else:
                    stream = kwargs.pop("stream", None)
                    h = StreamHandler(stream)
                handlers = [h]
            dfs = kwargs.pop("datefmt", None)
            style = kwargs.pop("style", '%')
            if style not in _STYLES:
                raise ValueError('Style must be one of: %s' % ','.join(
                    _STYLES.keys()))
            fs = kwargs.pop("format", _STYLES[style][1])
            fmt = Formatter(fs, dfs, style)
            for h in handlers:
                if h.formatter is None:
                    h.setFormatter(fmt)
                r.addHandler(h)
            level = kwargs.pop("level", None)
            if level is not None:
                r.setLevel(level)
            if kwargs:
                keys = ', '.join(kwargs.keys())
                raise ValueError('Unrecognised argument(s): %s' % keys)
    finally:
        _releaseLock()


# ---------------------------------------------------------------------------
# Utility functions at module level
# ---------------------------------------------------------------------------

def getLogger(name=None):
    """
    Return a logger with the specified name, creating it if necessary.
    """
    r = _mod_attr("root", root)
    if not name or (isinstance(name, str) and name == r.name):
        return r
    return Logger.manager.getLogger(name)


def critical(msg, *args, **kwargs):
    r = _mod_attr("root", root)
    if len(r.handlers) == 0:
        basicConfig()
    r.critical(msg, *args, **kwargs)


def fatal(msg, *args, **kwargs):
    critical(msg, *args, **kwargs)


def error(msg, *args, **kwargs):
    r = _mod_attr("root", root)
    if len(r.handlers) == 0:
        basicConfig()
    r.error(msg, *args, **kwargs)


def exception(msg, *args, exc_info=True, **kwargs):
    error(msg, *args, exc_info=exc_info, **kwargs)


def warning(msg, *args, **kwargs):
    r = _mod_attr("root", root)
    if len(r.handlers) == 0:
        basicConfig()
    r.warning(msg, *args, **kwargs)


def warn(msg, *args, **kwargs):
    warnings.warn("The 'warn' function is deprecated, "
                  "use 'warning' instead", DeprecationWarning, 2)
    warning(msg, *args, **kwargs)


def info(msg, *args, **kwargs):
    r = _mod_attr("root", root)
    if len(r.handlers) == 0:
        basicConfig()
    r.info(msg, *args, **kwargs)


def debug(msg, *args, **kwargs):
    r = _mod_attr("root", root)
    if len(r.handlers) == 0:
        basicConfig()
    r.debug(msg, *args, **kwargs)


def log(level, msg, *args, **kwargs):
    r = _mod_attr("root", root)
    if len(r.handlers) == 0:
        basicConfig()
    r.log(level, msg, *args, **kwargs)


def disable(level=CRITICAL):
    r = _mod_attr("root", root)
    r.manager.disable = level
    r.manager._clear_cache()


def shutdown(handlerList=_handlerList):
    for wr in reversed(handlerList[:]):
        try:
            h = wr()
            if h:
                try:
                    h.acquire()
                    if getattr(h, 'flushOnClose', True):
                        h.flush()
                    h.close()
                except (OSError, ValueError):
                    pass
                finally:
                    h.release()
        except Exception:
            if _mod_attr("raiseExceptions", raiseExceptions):
                raise


# ---------------------------------------------------------------------------
# Null handler
# ---------------------------------------------------------------------------

class NullHandler(Handler):
    """
    This handler does nothing.
    """
    def handle(self, record):
        pass

    def emit(self, record):
        pass

    def createLock(self):
        self.lock = None

    def _at_fork_reinit(self):
        pass


# ---------------------------------------------------------------------------
# Warnings integration
# ---------------------------------------------------------------------------

_warnings_showwarning = None


def _showwarning(message, category, filename, lineno, file=None, line=None):
    if file is not None:
        if _warnings_showwarning is not None:
            _warnings_showwarning(message, category, filename, lineno, file, line)
    else:
        s = warnings.formatwarning(message, category, filename, lineno, line)
        logger = getLogger("py.warnings")
        if not logger.handlers:
            logger.addHandler(NullHandler())
        logger.warning(str(s))


def captureWarnings(capture):
    global _warnings_showwarning
    if capture:
        if _warnings_showwarning is None:
            _warnings_showwarning = warnings.showwarning
            warnings.showwarning = _showwarning
    else:
        if _warnings_showwarning is not None:
            warnings.showwarning = _warnings_showwarning
            _warnings_showwarning = None
