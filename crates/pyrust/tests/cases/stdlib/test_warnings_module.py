# Test the `warnings` module stub: warn(), filterwarnings(), simplefilter(),
# resetwarnings(), catch_warnings(), and the category class exports.
#
# warn() normally writes to stderr; we suppress that output here so the
# parity harness sees only stdout.  All tests print "True"/"False"/"ok" on
# stdout to allow exact diff.

import warnings

# ── warn() with simplefilter("ignore") does not raise ────────────────────────

warnings.simplefilter("ignore")
warnings.warn("test", UserWarning, stacklevel=1)
warnings.warn("deprecated", DeprecationWarning)
warnings.warn("runtime", RuntimeWarning)
warnings.resetwarnings()
print("warn-ok")  # warn-ok

# ── filterwarnings("ignore") suppresses by category ──────────────────────────

warnings.filterwarnings("ignore", category=DeprecationWarning)
warnings.warn("dep-ignored", DeprecationWarning)
warnings.resetwarnings()
print("filterwarnings-ok")  # filterwarnings-ok

# ── filterwarnings("error") turns a warning into an exception ────────────────

warnings.filterwarnings("error", category=UserWarning)
try:
    warnings.warn("will-raise", UserWarning)
    print("FAIL")
except UserWarning as e:
    print(str(e))  # will-raise
warnings.resetwarnings()

# ── simplefilter("error") same ────────────────────────────────────────────────

warnings.simplefilter("error")
try:
    warnings.warn("also-raises", RuntimeWarning)
    print("FAIL")
except RuntimeWarning as e:
    print(str(e))  # also-raises
warnings.resetwarnings()

# ── catch_warnings() restores filter state ───────────────────────────────────

warnings.simplefilter("ignore")
with warnings.catch_warnings():
    warnings.simplefilter("error")
    # Inside the block the filter is "error".
    try:
        warnings.warn("inside", UserWarning)
        print("FAIL-inside")
    except UserWarning:
        print("error-inside")  # error-inside
# After the block the filter is restored to "ignore".
warnings.warn("after-restore", UserWarning)  # should not raise
print("restore-ok")  # restore-ok
warnings.resetwarnings()

# ── catch_warnings(record=True) collects WarningMessage objects ───────────────

with warnings.catch_warnings(record=True) as w:
    warnings.simplefilter("always")
    warnings.warn("msg1", UserWarning)
    warnings.warn("msg2", DeprecationWarning)

print(len(w))                          # 2
print(str(w[0].message))              # msg1
print(w[0].category.__name__)         # UserWarning
print(str(w[1].message))              # msg2
print(w[1].category.__name__)         # DeprecationWarning

# ── resetwarnings() clears all filters ───────────────────────────────────────

warnings.simplefilter("ignore")
warnings.resetwarnings()
# After reset, filters list is empty; "default" action applies.
# We only check that warn() doesn't raise here (stderr suppressed by test).
with warnings.catch_warnings(record=True) as w2:
    warnings.simplefilter("always")
    warnings.warn("after-reset", RuntimeWarning)
print(len(w2))   # 1
print(w2[0].category.__name__)  # RuntimeWarning
