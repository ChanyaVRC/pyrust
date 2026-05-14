# Parity tests for the typed-dialect migration of `format` (#400).
#
# Covers:
#   - happy path: int / float-with-spec / str / str-with-spec
#   - non-string format_spec — TypeError (parity-bug fix: legacy raised
#     RuntimeError, CPython raises TypeError; the typed dialect now
#     matches CPython on the *class*, while the wording is the standard
#     typed-prelude divergence)
#   - missing required positional arg
#   - extra positional arg
#   - keyword arg (both params are positional-only — must be rejected)
#
# The TypeError wording from the typed-dialect prelude does not match
# CPython byte-for-byte, so error-path branches only print the caught
# exception's class name, which is identical across both runtimes.

# ── happy path ───────────────────────────────────────────────────────────────
print("format-int", format(42))
print("format-float-2f", format(3.14, ".2f"))
print("format-str", format("hi"))
print("format-str-right10", format("hi", ">10"))

# Custom __format__ still dispatches (sanity).
class WithFormat:
    def __format__(self, spec):
        return "custom-" + spec

print("format-user-empty", format(WithFormat()))
print("format-user-spec", format(WithFormat(), "abc"))

# ── parity-bug fix: non-string format_spec is TypeError, not RuntimeError ────
try:
    format("x", 123)
except TypeError:
    print("format-nonstr-spec", True)

# ── missing required arg ─────────────────────────────────────────────────────
try:
    format()
except TypeError:
    print("format-missing", True)

# ── extra positional arg ─────────────────────────────────────────────────────
try:
    format("x", "y", "z")
except TypeError:
    print("format-extra", True)

# ── positional-only: keyword arg is rejected ─────────────────────────────────
try:
    format("x", format_spec=">5")
except TypeError:
    print("format-kw", True)
