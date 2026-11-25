# CPython 3.12 parity tests for the `signal` module (issue #3209).

import enum
import errno
import signal
import sys

# ── IntEnum types and handler constants ──────────────────────────────────────
print("Signals IntEnum:", issubclass(signal.Signals, enum.IntEnum))
print("Handlers IntEnum:", issubclass(signal.Handlers, enum.IntEnum))
print("SIG_DFL:", isinstance(signal.SIG_DFL, signal.Handlers), int(signal.SIG_DFL))
print("SIG_IGN:", isinstance(signal.SIG_IGN, signal.Handlers), int(signal.SIG_IGN))
print("SIG_DFL repr:", repr(signal.SIG_DFL), "str:", str(signal.SIG_DFL))
print("SIG_IGN repr:", repr(signal.SIG_IGN), "str:", str(signal.SIG_IGN))
print("NSIG:", signal.NSIG)

# ── Standard cross-platform signal constants ─────────────────────────────────
for name in ("SIGINT", "SIGTERM", "SIGABRT", "SIGFPE", "SIGILL", "SIGSEGV"):
    val = getattr(signal, name)
    print(
        name,
        isinstance(val, signal.Signals),
        isinstance(val, int),
        val.name,
        int(val),
        repr(val),
    )

# ── Platform-specific signal constants ───────────────────────────────────────
if sys.platform == "win32":
    for name in ("SIGBREAK", "CTRL_C_EVENT", "CTRL_BREAK_EVENT"):
        val = getattr(signal, name)
        print("win", name, isinstance(val, signal.Signals), int(val))
else:
    for name in (
        "SIGHUP",
        "SIGQUIT",
        "SIGKILL",
        "SIGALRM",
        "SIGPIPE",
        "SIGUSR1",
        "SIGUSR2",
        "SIGCHLD",
        "SIGSTOP",
        "SIGCONT",
    ):
        val = getattr(signal, name)
        print("posix", name, isinstance(val, signal.Signals), int(val))

# ── valid_signals() ──────────────────────────────────────────────────────────
vs = signal.valid_signals()
print("valid_signals type:", type(vs).__name__)
print("SIGINT in valid:", signal.SIGINT in vs, "SIGTERM in valid:", signal.SIGTERM in vs)
print("valid_signals ints:", sorted(int(s) for s in vs))

# ── default_int_handler ──────────────────────────────────────────────────────
print("SIGINT default:", signal.getsignal(signal.SIGINT) is signal.default_int_handler)
print("SIGTERM default:", signal.getsignal(signal.SIGTERM) is signal.SIG_DFL)
print(
    "SIGTERM default is Handlers:",
    isinstance(signal.getsignal(signal.SIGTERM), signal.Handlers),
)

try:
    signal.default_int_handler(signal.SIGINT, None)
except KeyboardInterrupt:
    print("default_int_handler raised KeyboardInterrupt")

try:
    signal.default_int_handler()
except TypeError:
    print("default_int_handler() raised TypeError")

# ── signal() / getsignal() state management ──────────────────────────────────
def my_handler(signum, frame):
    pass


prev1 = signal.signal(signal.SIGTERM, my_handler)
print("prev1 is SIG_DFL:", prev1 is signal.SIG_DFL)
print("cur is my_handler:", signal.getsignal(signal.SIGTERM) is my_handler)

prev2 = signal.signal(signal.SIGTERM, signal.SIG_IGN)
print("prev2 is my_handler:", prev2 is my_handler)
print("cur is SIG_IGN:", signal.getsignal(signal.SIGTERM) is signal.SIG_IGN)

prev3 = signal.signal(signal.SIGTERM, 0)
print("prev3 is SIG_IGN:", prev3 is signal.SIG_IGN)
print("cur is SIG_DFL:", signal.getsignal(signal.SIGTERM) is signal.SIG_DFL)

prev4 = signal.signal(int(signal.SIGTERM), 1)
print("prev4 is SIG_DFL:", prev4 is signal.SIG_DFL)
print("cur after int 1:", signal.getsignal(int(signal.SIGTERM)) is signal.SIG_IGN)

# Restore default
signal.signal(signal.SIGTERM, signal.SIG_DFL)

# ── Validation errors ────────────────────────────────────────────────────────
for bad_sig in (0, -1, signal.NSIG, 99999):
    for label, fn in (
        ("getsignal", lambda s=bad_sig: signal.getsignal(s)),
        ("signal", lambda s=bad_sig: signal.signal(s, signal.SIG_DFL)),
        ("strsignal", lambda s=bad_sig: signal.strsignal(s)),
    ):
        try:
            fn()
        except ValueError:
            print(label, bad_sig, "ValueError")

for bad_handler in (42, "not-callable", None, []):
    try:
        signal.signal(signal.SIGTERM, bad_handler)
    except TypeError:
        print("bad_handler", type(bad_handler).__name__, "TypeError")

if sys.platform != "win32":
    for uncatchable in (signal.SIGKILL, signal.SIGSTOP):
        try:
            signal.signal(uncatchable, signal.SIG_IGN)
        except OSError as exc:
            print("uncatchable", uncatchable.name, exc.errno == errno.EINVAL)

# ── strsignal() ──────────────────────────────────────────────────────────────
for name in ("SIGINT", "SIGTERM", "SIGABRT", "SIGFPE", "SIGILL", "SIGSEGV"):
    sig = getattr(signal, name)
    desc = signal.strsignal(sig)
    # CPython 3.12.x switched Windows SIGFPE between "Floating point exception"
    # (3.12.3) and "Floating-point exception" (3.12.13); normalise the hyphen so
    # the test is exact across 3.12 patch releases.
    if name == "SIGFPE" and isinstance(desc, str):
        desc = desc.replace("-", " ")
    print("strsignal", name, repr(desc))
if sys.platform == "win32":
    print("strsignal SIGBREAK", repr(signal.strsignal(signal.SIGBREAK)))
    print("getsignal(3)", repr(signal.getsignal(3)))
    print("strsignal(3)", repr(signal.strsignal(3)))
