# Basic positional substitution
assert "Hello, {}!".format("world") == "Hello, world!"
assert "{0} + {1} = {2}".format(1, 2, 3) == "1 + 2 = 3"
assert "{1} {0}".format("a", "b") == "b a"

# Auto-numbered placeholders
assert "{} {} {}".format(1, 2, 3) == "1 2 3"

# Keyword substitution
assert "{name} is {age}".format(name="Alice", age=30) == "Alice is 30"
assert "{a}-{b}-{a}".format(a="x", b="y") == "x-y-x"

# Literal braces
assert "{{literal}}".format() == "{literal}"
assert "{{{}}}".format(42) == "{42}"

# Format specs: width and alignment
assert "{:>10}".format("right") == "     right"
assert "{:<10}".format("left") == "left      "
assert "{:^10}".format("ctr") == "   ctr    "
assert "{:*^10}".format("ctr") == "***ctr****"

# Numeric format specs
assert "{:.2f}".format(3.14159) == "3.14"
assert "{:.0f}".format(2.5) == "2"  # banker's rounding may differ; pick a safe value
assert "{:05d}".format(42) == "00042"
assert "{:+d}".format(7) == "+7"
assert "{:+d}".format(-7) == "-7"
assert "{:x}".format(255) == "ff"
assert "{:X}".format(255) == "FF"
assert "{:o}".format(8) == "10"
assert "{:b}".format(5) == "101"

# Conversion flags
assert "{!r}".format("hi") == "'hi'"
assert "{!s}".format(42) == "42"

# Mix of positional and keyword
assert "{0} {x}".format("hello", x="world") == "hello world"

# Attribute and item access on positional args
class Pt:
    def __init__(self, x, y):
        self.x = x
        self.y = y

p = Pt(3, 4)
assert "({0.x}, {0.y})".format(p) == "(3, 4)"

lst = [10, 20, 30]
assert "{0[1]}".format(lst) == "20"

# Empty format string
assert "".format() == ""
assert "no placeholders".format() == "no placeholders"

# IndexError on missing positional
try:
    "{2}".format("a", "b")
    print("FAIL: expected IndexError")
except IndexError:
    pass

# KeyError on missing kwarg
try:
    "{missing}".format(x=1)
    print("FAIL: expected KeyError")
except KeyError:
    pass

print("str.format OK")
