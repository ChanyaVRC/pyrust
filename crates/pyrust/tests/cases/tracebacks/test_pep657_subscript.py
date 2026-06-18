# PEP 657 caret placement for chained subscript expressions (issue #2570).
#
# A chained subscript `d['a']['b']['c']` whose last key is missing must
# underline the *failing* subscript precisely.  CPython 3.12:
#
#     d['a']['b']['c']
#     ~~~~~~~~~~~^^^^^
#     KeyError: 'c'
#
# The object being indexed (`d['a']['b']`) is underlined with `~`, the failing
# `['c']` subscript with `^`.  Likewise for an out-of-range index chain:
#
#     a[0][1][2]
#     ~~~~^^^
#     IndexError: list index out of range
#
# Before the fix the optimizer marked the inner `GetItem`s "ambiguous" (their
# register operands collapse to identical forms after copy-prop) and dropped
# every caret past the first, so chained subscripts printed no caret at all.
#
# NOTE: the parity harness strips the `^`/`~` underline rows before diffing
# (see crates/pyrust/tests/parity_compare.rs::normalize_pythonish_output), so
# this fixture pins the **exception message + echoed source line** half; the
# precise caret columns are verified byte-for-byte against `python3.12`
# manually (and in the implementing PR's description).

# --- chained dict subscript: missing key on the third `[...]` ---
try:
    d = {"a": {"b": {}}}
    d["a"]["b"]["c"]
except KeyError as e:
    print("chained key:", type(e).__name__, e)


# --- chained list index: out of range on an inner `[...]` ---
try:
    a = [[0], [1]]
    a[0][1][2]
except IndexError as e:
    print("chained index:", type(e).__name__, e)


# --- single subscript still anchors correctly ---
try:
    {}["missing"]
except KeyError as e:
    print("single:", type(e).__name__, e)


# --- mixed attribute + subscript chain ---
try:
    d = {"a": {"b": []}}
    d["a"]["b"][5]
except IndexError as e:
    print("mixed:", type(e).__name__, e)
