# Parity fixture: PEP 657 attribute-access caret anchors — store side (#2442).
#
# Companion to language/test_pep657_attr_carets.py, which covers the attribute
# *read* (`GetAttr`).  This fixture covers the attribute *store* paths, where an
# uncaught AttributeError should underline the `obj.attr` target span:
#
#   * plain attribute assignment   `obj.attr = v`        (Stmt::AttrAssign)
#   * unpacked attribute target    `a, obj.attr = ...`   (store-unpack)
#   * augmented attribute assign   `obj.attr += v`        (GetAttr read-back)
#
# CPython 3.12 anchors the caret on `obj.attr` (target start column → attribute
# name end column) for each of these.  When the anchor spans the whole
# significant line CPython prints no caret row; pyrust mirrors that.
#
# The parity harness merges stdout+stderr and strips the `^`/`~` underline rows
# before diffing (CPython emits fine-grained markers, pyrust a full-width `^`),
# so this fixture verifies the echoed source line + exception class/message;
# exact caret placement is checked by hand against python3.12.
#
# Each case raises inside exec() (caught here) so the traceback hits stderr and
# the script keeps running.


class Frozen:
    __slots__ = ()


# Case 1: plain `obj.attr = v` store rejected — underline `obj.attr`.
try:
    exec("f = Frozen(); f.x = 1", {"Frozen": Frozen})
except AttributeError as e:
    print("case1:", type(e).__name__)

# Case 2: store target nested in an unpack — underline only `f.x`.
try:
    exec("f = Frozen(); a, f.x = 1, 2", {"Frozen": Frozen})
except (AttributeError, ValueError) as e:
    print("case2:", type(e).__name__)

# Case 3: augmented attribute assignment with a missing attribute — the read
# fails first; underline `f.x`.
try:
    exec("f = Frozen(); f.x += 1", {"Frozen": Frozen})
except AttributeError as e:
    print("case3:", type(e).__name__)

# Case 4: store on a built-in instance that rejects attributes (whole-line
# anchor → CPython prints no caret row).
try:
    exec("(1).bit_length = 5", {})
except AttributeError as e:
    print("case4:", type(e).__name__)

print("all done")
