# Tests for ascii() and the !a f-string conversion dispatching user __repr__.
# Issue #1197: both were calling Value::repr() directly, bypassing __repr__.

class Foo:
    def __repr__(self):
        return "Foo(custom)"

# Use ord() references rather than raw non-ASCII literals so the fixture file
# stays ASCII-clean and the expected output is unambiguous.
class Bar:
    def __repr__(self):
        # chr(233) == '\xe9' (LATIN SMALL LETTER E WITH ACUTE)
        return "Bar(" + chr(233) + ")"

class Wide:
    def __repr__(self):
        # chr(0x1F600) == U+1F600 GRINNING FACE (> U+FFFF)
        return "Wide(" + chr(0x1F600) + ")"

class NonStrRepr:
    def __repr__(self):
        return 42

class NoRepr:
    pass

# --- ascii() builtin ---

# User __repr__ is called; ASCII-only result passes through unchanged.
print(ascii(Foo()))

# User __repr__ is called; non-ASCII chars are escaped.
print(ascii(Bar()))

# Codepoint above U+FFFF uses \UNNNNNNNN form.
print(ascii(Wide()))

# No __repr__ defined — falls back to default object repr (contains only ASCII
# in practice, but we just confirm it does not raise).
result = ascii(NoRepr())
print(result.startswith("<") and result.endswith(">"))

# Non-string __repr__ raises TypeError.
try:
    ascii(NonStrRepr())
except TypeError as e:
    print("TypeError raised")

# Plain built-in types still work.
print(ascii("hello " + chr(233) + " world"))
print(ascii(42))
print(ascii([1, 2, 3]))

# --- f-string !a conversion ---

f = Foo()
b = Bar()
w = Wide()

# ASCII-only custom repr passes through.
print(f"{f!a}")

# Non-ASCII chars in repr are escaped.
print(f"{b!a}")

# Wide codepoint.
print(f"{w!a}")

# Plain int via !a.
print(f"{42!a}")
