# Parity fixture for issue #1370:
# When __format__ returns a non-str, a TypeError must be raised regardless
# of how __format__ is invoked: format(), str.format(), str.format_map(),
# and f-strings must all validate the return type.


class BadInt:
    """Returns an int instead of str from __format__."""

    def __format__(self, spec):
        return 42


class BadNone:
    """Returns None instead of str from __format__."""

    def __format__(self, spec):
        return None


class GoodStr:
    """Returns a proper str from __format__."""

    def __format__(self, spec):
        return "formatted:" + spec


class GoodStrNoFormat:
    """No custom __format__ — falls back to object.__format__."""

    def __str__(self):
        return "GoodStrNoFormat()"


# ── format() builtin ────────────────────────────────────────────────────────

try:
    format(BadInt())
except TypeError as e:
    print("format(BadInt()):", e)

try:
    format(BadInt(), "d")
except TypeError as e:
    print("format(BadInt(), 'd'):", e)

try:
    format(BadNone())
except TypeError as e:
    print("format(BadNone()):", e)

# Correct return: no error
print(format(GoodStr(), "x"))
print(format(GoodStr(), ""))

# ── str.format() ─────────────────────────────────────────────────────────────

try:
    "{}".format(BadInt())
except TypeError as e:
    print("'{}' % BadInt():", e)

try:
    "{:}".format(BadInt())
except TypeError as e:
    print("'{:}' % BadInt():", e)

try:
    "{:d}".format(BadInt())
except TypeError as e:
    print("'{:d}' % BadInt():", e)

try:
    "{0}".format(BadNone())
except TypeError as e:
    print("'{0}' % BadNone():", e)

print("{0}".format(GoodStr()))
print("{0:x}".format(GoodStr()))

# ── str.format_map() ─────────────────────────────────────────────────────────

try:
    "{x}".format_map({"x": BadInt()})
except TypeError as e:
    print("format_map BadInt:", e)

try:
    "{x:d}".format_map({"x": BadInt()})
except TypeError as e:
    print("format_map BadInt spec:", e)

print("{x}".format_map({"x": GoodStr()}))

# ── f-strings ────────────────────────────────────────────────────────────────

b = BadInt()
try:
    _ = f"{b}"
except TypeError as e:
    print("f-string BadInt:", e)

try:
    _ = f"{b:d}"
except TypeError as e:
    print("f-string BadInt spec:", e)

g = GoodStr()
print(f"{g}")
print(f"{g:x}")

# ── class with no custom __format__ (fallback to object.__format__) ──────────

# Empty spec: should call __str__
result = "{}".format(GoodStrNoFormat())
print("no __format__ empty spec:", result)

# Non-empty spec via str.format: TypeError
try:
    "{:d}".format(GoodStrNoFormat())
except TypeError as e:
    print("no __format__ non-empty spec:", e)
