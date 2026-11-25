"""Set handlers for asynchronous events.

CPython 3.12-compatible implementation of Python's ``signal`` module for
PyRust (issue #3209), backed by native platform signal tables and
``strsignal`` in ``signal.rs``.
"""

import errno
import operator
from enum import IntEnum


class Handlers(IntEnum):
    SIG_DFL = 0
    SIG_IGN = 1


SIG_DFL = Handlers.SIG_DFL
SIG_IGN = Handlers.SIG_IGN

NSIG = _platform_nsig()
_IS_POSIX = _platform_is_posix()
_SIGNAL_TABLE = _platform_signals()
_VALID_SIGNAL_INTS = _platform_valid_signals()
_VALID_SIGNAL_SET = frozenset(_VALID_SIGNAL_INTS)
_UNCATCHABLE_SET = frozenset(_platform_uncatchable_signals())


def _build_signals_enum(table):
    lines = ["class Signals(IntEnum):"]
    for name, num in table.items():
        lines.append(f"    {name} = {num}")
    ns = {"IntEnum": IntEnum, "__name__": "signal"}
    exec("\n".join(lines), ns)
    cls = ns["Signals"]
    cls.__module__ = "signal"
    return cls


Signals = _build_signals_enum(_SIGNAL_TABLE)

for _sig_name in _SIGNAL_TABLE:
    globals()[_sig_name] = Signals[_sig_name]


def default_int_handler(*args):
    """The default handler for SIGINT installed by Python."""
    if len(args) != 2:
        raise TypeError(f"default_int_handler expected 2 arguments, got {len(args)}")
    raise KeyboardInterrupt()


def _int_to_enum(value, enum_klass):
    """Convert a possible numeric value to an IntEnum member.
    If it's not a known member, return the value itself.
    """
    if not isinstance(value, int):
        return value
    try:
        return enum_klass(value)
    except ValueError:
        return value


def _require_signal_int(signalnum):
    if isinstance(signalnum, int):
        return int(signalnum)
    try:
        return operator.index(signalnum)
    except TypeError:
        raise TypeError(
            f"'{type(signalnum).__name__}' object cannot be interpreted as an integer"
        ) from None


_handlers = {}
for _s in _VALID_SIGNAL_INTS:
    _handlers[_s] = SIG_DFL
if int(Signals.SIGINT) in _handlers:
    _handlers[int(Signals.SIGINT)] = default_int_handler
if _IS_POSIX:
    if "SIGPIPE" in _SIGNAL_TABLE and _SIGNAL_TABLE["SIGPIPE"] in _handlers:
        _handlers[_SIGNAL_TABLE["SIGPIPE"]] = SIG_IGN
    if "SIGXFSZ" in _SIGNAL_TABLE and _SIGNAL_TABLE["SIGXFSZ"] in _handlers:
        _handlers[_SIGNAL_TABLE["SIGXFSZ"]] = SIG_IGN


def getsignal(signalnum):
    """Return the current action for the given signal."""
    sig = _require_signal_int(signalnum)
    if sig < 1 or sig >= NSIG:
        raise ValueError("signal number out of range")
    handler = _handlers.get(sig, None)
    return _int_to_enum(handler, Handlers)


def signal(signalnum, handler):
    """Set the action for the given signal and return the previous handler."""
    sig = _require_signal_int(signalnum)
    if not _IS_POSIX:
        if sig not in _VALID_SIGNAL_SET:
            raise ValueError("invalid signal value")
    else:
        if sig < 1 or sig >= NSIG:
            raise ValueError("signal number out of range")
        if sig not in _VALID_SIGNAL_SET:
            raise ValueError("invalid signal value")

    if isinstance(handler, int):
        h_int = int(handler)
        if h_int == int(SIG_DFL):
            new_handler = SIG_DFL
        elif h_int == int(SIG_IGN):
            new_handler = SIG_IGN
        else:
            raise TypeError(
                "signal handler must be signal.SIG_IGN, signal.SIG_DFL, or a callable object"
            )
    elif callable(handler):
        new_handler = handler
    else:
        raise TypeError(
            "signal handler must be signal.SIG_IGN, signal.SIG_DFL, or a callable object"
        )

    if sig in _UNCATCHABLE_SET:
        raise OSError(errno.EINVAL, "Invalid argument")

    old = _handlers.get(sig, None)
    _handlers[sig] = new_handler
    return _int_to_enum(old, Handlers)


def valid_signals():
    """Return a set of valid signal numbers on this platform."""
    return {_int_to_enum(x, Signals) for x in _VALID_SIGNAL_INTS}


def strsignal(signalnum):
    """Return the system description of the given signal."""
    sig = _require_signal_int(signalnum)
    if sig < 1 or sig >= NSIG:
        raise ValueError("signal number out of range")
    return _platform_strsignal(sig)
