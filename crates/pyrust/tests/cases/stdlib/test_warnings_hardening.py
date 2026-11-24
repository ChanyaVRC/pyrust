"""CPython 3.12 parity tests for warnings hardening (issue #3155)."""

import io
import re
import sys
import warnings


def current_line():
    return sys._getframe(1).f_lineno


# ── 1. Live warnings.filters list ─────────────────────────────────────────────
warnings.resetwarnings()
filters_ref = warnings.filters
print("filters is list:", isinstance(filters_ref, list))
print("filters empty after reset:", len(filters_ref) == 0)

# filterwarnings/simplefilter mutate the live list in place.
warnings.simplefilter("ignore", UserWarning)
print("in-place simplefilter:", warnings.filters is filters_ref, len(filters_ref))
warnings.resetwarnings()
print("in-place reset:", warnings.filters is filters_ref, len(filters_ref))

# Direct mutation of warnings.filters takes effect immediately.
warnings.filters.insert(0, ("error", None, UserWarning, None, 0))
try:
    warnings.warn("direct-insert", UserWarning)
    print("direct-insert FAIL")
except UserWarning as e:
    print("direct-insert caught:", str(e))

# Clearing directly removes the error filter.
warnings.filters.clear()
with warnings.catch_warnings(record=True) as w:
    warnings.warn("after-direct-clear", UserWarning)
print("after-direct-clear:", len(w), str(w[0].message))

# Rebinding warnings.filters = [...] takes effect immediately.
orig_filters = warnings.filters
warnings.filters = [("ignore", None, UserWarning, None, 0)]
with warnings.catch_warnings(record=True) as w:
    warnings.warn("rebound-ignored", UserWarning)
print("rebound-ignored:", len(w))
warnings.filters = orig_filters
warnings.resetwarnings()

# catch_warnings copies filters on enter and restores original list on exit.
warnings.simplefilter("ignore", DeprecationWarning)
before_list = warnings.filters
with warnings.catch_warnings():
    print("inside catch copy:", warnings.filters is not before_list, len(warnings.filters))
    warnings.simplefilter("error", UserWarning)
    print("inside catch len:", len(warnings.filters))
print("after catch restored:", warnings.filters is before_list, len(warnings.filters))
warnings.resetwarnings()

# catch_warnings(action=...) (Python 3.11+)
with warnings.catch_warnings(action="error", category=RuntimeWarning):
    try:
        warnings.warn("cw-action", RuntimeWarning)
        print("cw-action FAIL")
    except RuntimeWarning as e:
        print("cw-action caught:", str(e))


# ── 2. Caller frame resolution with stacklevel ────────────────────────────────
def emit_level(msg, level):
    warnings.warn(msg, UserWarning, stacklevel=level)


def outer_emit(msg, level):
    return emit_level(msg, level)


def emit_kw(*, msg, level):
    warnings.warn(msg, UserWarning, stacklevel=level)


with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter("always")
    l1 = current_line() + 1
    warnings.warn("direct-1")
    l2 = current_line() + 1
    emit_level("helper-2", 2)
    l3 = current_line() + 1
    outer_emit("nested-3", 3)
    l4 = current_line() + 1
    emit_kw(msg="kw-2", level=2)
    warnings.warn("huge-stack", stacklevel=999)

print("stacklevel=1 line match:", w[0].lineno == l1, w[0].filename.endswith("test_warnings_hardening.py"))
print("stacklevel=2 line match:", w[1].lineno == l2, w[1].filename.endswith("test_warnings_hardening.py"))
print("stacklevel=3 line match:", w[2].lineno == l3, w[2].filename.endswith("test_warnings_hardening.py"))
print("stacklevel=2 kw line match:", w[3].lineno == l4, w[3].filename.endswith("test_warnings_hardening.py"))
print("stacklevel=999 fallback:", w[4].filename, w[4].lineno)


# ── 3. Full filterwarnings (message regex, module regex, lineno, append) ──────
warnings.resetwarnings()
warnings.filterwarnings("error", message=r"hello\s+\d+", category=UserWarning)
print("filter tuple len:", len(warnings.filters[0]), warnings.filters[0][0])

try:
    warnings.warn("HELLO   42 world", UserWarning)
    print("regex-msg FAIL")
except UserWarning as e:
    print("regex-msg matched:", str(e))

with warnings.catch_warnings(record=True) as w:
    # Does not match regex -> falls through to default
    warnings.warn("goodbye 42", UserWarning)
print("regex-msg non-match recorded:", len(w), str(w[0].message))

# module regex filter
warnings.resetwarnings()
warnings.filterwarnings("error", module=r"^__main__$")
try:
    warnings.warn("mod-match")
    print("mod-match FAIL")
except UserWarning as e:
    print("mod-match caught:", str(e))

warnings.resetwarnings()
warnings.filterwarnings("error", module=r"^other_module$")
with warnings.catch_warnings(record=True) as w:
    warnings.warn("mod-miss")
print("mod-miss recorded:", len(w))

# lineno filter
warnings.resetwarnings()
target_line = current_line() + 3
warnings.filterwarnings("error", lineno=target_line)
try:
    warnings.warn("on-target-line")
    print("lineno FAIL")
except UserWarning as e:
    print("lineno matched:", str(e))
with warnings.catch_warnings(record=True) as w:
    warnings.warn("off-target-line")
print("lineno non-match recorded:", len(w))

# append=True vs append=False priority
warnings.resetwarnings()
warnings.filterwarnings("ignore", message="dup")
warnings.filterwarnings("error", message="dup", append=True)
with warnings.catch_warnings(record=True) as w:
    warnings.warn("dup test")
print("append=True lower priority:", len(w))

warnings.filterwarnings("error", message="dup", append=False)
try:
    warnings.warn("dup test")
    print("append=False FAIL")
except UserWarning as e:
    print("append=False higher priority:", str(e))

# "once" and "module" actions
warnings.resetwarnings()
with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter("once")
    warnings.warn("only-once")
    warnings.warn("only-once")
print("once count:", len(w))

warnings.resetwarnings()
with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter("module")
    warnings.warn("per-module")
    warnings.warn("per-module")
print("module count:", len(w))

# filterwarnings validation errors
for label, fn in [
    ("bad-action", lambda: warnings.filterwarnings("bogus")),
    ("bad-msg", lambda: warnings.filterwarnings("ignore", message=123)),
    ("bad-mod", lambda: warnings.filterwarnings("ignore", module=123)),
    ("neg-lineno", lambda: warnings.filterwarnings("ignore", lineno=-1)),
    ("bad-lineno", lambda: warnings.filterwarnings("ignore", lineno="1")),
]:
    try:
        fn()
        print(label, "NO ERROR")
    except AssertionError:
        print(label, "AssertionError")


# ── 4. formatwarning, showwarning, warn_explicit, monkey-patching ─────────────
warnings.resetwarnings()
print("formatwarning explicit line:", repr(warnings.formatwarning("oops", UserWarning, "demo.py", 12, line="  y = 1  ")))
print("formatwarning empty line:", repr(warnings.formatwarning("oops", UserWarning, "demo.py", 12, line="")))
print("formatwarning sys:", repr(warnings.formatwarning("oops", UserWarning, "sys", 1)))

buf = io.StringIO()
warnings.showwarning("hello", DeprecationWarning, "mod.py", 7, file=buf, line="x = 2")
print("showwarning buf:", repr(buf.getvalue()))

# Monkey-patching warnings.formatwarning affects warnings.showwarning
orig_fw = warnings.formatwarning
try:
    warnings.formatwarning = lambda msg, cat, fn, ln, line=None: f"[{cat.__name__}] {fn}:{ln} {msg}\n"
    buf2 = io.StringIO()
    warnings.showwarning("patched-fw", UserWarning, "f.py", 3, file=buf2)
    print("patched formatwarning:", repr(buf2.getvalue()))
finally:
    warnings.formatwarning = orig_fw

# Monkey-patching warnings.showwarning intercepts warn() and warn_explicit()
seen = []
orig_sw = warnings.showwarning
try:
    def custom_sw(message, category, filename, lineno, file=None, line=None):
        seen.append((str(message), category.__name__, filename, lineno))

    warnings.showwarning = custom_sw
    warnings.simplefilter("always")
    warnings.warn("via-custom-sw", RuntimeWarning)
    warnings.warn_explicit("explicit-msg", DeprecationWarning, "explicit.py", 99, module="explicit_mod")

    # catch_warnings(record=True) temporarily restores default recording sink
    # and restores custom_sw on exit.
    with warnings.catch_warnings(record=True) as w:
        warnings.warn("inside-record", UserWarning)
    warnings.warn("after-record", UserWarning)
finally:
    warnings.showwarning = orig_sw
    warnings.resetwarnings()

print("custom_sw count:", len(seen))
print("custom_sw[0]:", seen[0][0], seen[0][1], seen[0][2].endswith("test_warnings_hardening.py"))
print("custom_sw[1]:", seen[1])
print("inside-record count:", len(w), str(w[0].message))
print("custom_sw[2]:", seen[2][0], seen[2][1])

# warn_explicit with module filter and explicit registry
warnings.resetwarnings()
warnings.filterwarnings("error", module="my_pkg")
try:
    warnings.warn_explicit("boom", UserWarning, "my_pkg.py", 5)
    print("warn_explicit module FAIL")
except UserWarning as e:
    print("warn_explicit module caught:", str(e))

warnings.resetwarnings()
reg = {}
with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter("default")
    warnings.warn_explicit("dup-explicit", UserWarning, "a.py", 10, module="a", registry=reg)
    warnings.warn_explicit("dup-explicit", UserWarning, "a.py", 10, module="a", registry=reg)
print("warn_explicit registry dedup:", len(w))

# WarningMessage construction and str()
wm = warnings.WarningMessage(UserWarning("hi"), UserWarning, "x.py", 4, line="pass")
print("WarningMessage attrs:", str(wm.message), wm.category.__name__, wm.filename, wm.lineno, wm.line)
print("WarningMessage str:", str(wm))

print("warnings hardening ok")
