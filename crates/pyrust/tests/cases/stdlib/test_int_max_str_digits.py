# Parity fixture for issue #2001: CPython 3.11+'s integer string-conversion
# length limit (sys.get/set_int_max_str_digits, default 4300 digits, gh-95778).
#
# Covers: the limit on int(str) parsing and int->str rendering
# (str/repr/format/f-string/%/print), the power-of-two-base and arithmetic
# exemptions, and the sys.get_int_max_str_digits / set_int_max_str_digits API.
import sys


def show(label, fn):
    try:
        fn()
        print(label, "OK")
    except ValueError as e:
        print(label, "ValueError:", e)
    except TypeError as e:
        print(label, "TypeError:", e)


# Default limit.
print("default:", sys.get_int_max_str_digits())

# --- int(str) parse side (message includes the digit count) ---
show("parse-4301", lambda: int("9" * 4301))
show("parse-4300", lambda: int("9" * 4300))   # exactly at the limit: OK
show("parse-base3", lambda: int("1" * 5000, 3))   # non-power-of-two base: limited
show("parse-hex", lambda: int("f" * 5000, 16))    # power-of-two base: exempt
show("parse-bin", lambda: int("1" * 5000, 2))      # exempt

# --- int -> str render side (message has no digit count) ---
big = 10 ** 5000
show("str", lambda: str(big))
show("repr", lambda: repr(big))
show("fstring", lambda: f"{big}")
show("percent-d", lambda: "%d" % big)
show("percent-s", lambda: "%s" % big)
show("percent-x", lambda: "%x" % big)              # hex render: exempt
show("format-empty", lambda: format(big, ""))
show("format-d", lambda: format(big, "d"))
show("format-x", lambda: format(big, "x"))         # exempt
show("brace-format", lambda: "{}".format(big))
show("nested-list-repr", lambda: repr([big]))
show("frozenset-repr", lambda: repr(frozenset({big})))   # frozenset element
show("nested-frozenset", lambda: repr([frozenset({big})]))
show("hex-builtin", lambda: hex(big))              # exempt
show("arith", lambda: big + 1 > 0)                  # arithmetic: exempt

# Small ints are always fine.
print("small:", str(123), repr(-45), int("678"), f"{9}", "%d" % 7, format(42, "d"))

# --- sys.set_int_max_str_digits rules ---
show("set-639", lambda: sys.set_int_max_str_digits(639))   # ValueError (< 640)
show("set-str", lambda: sys.set_int_max_str_digits("x"))   # TypeError

sys.set_int_max_str_digits(640)
print("after-640:", sys.get_int_max_str_digits())

sys.set_int_max_str_digits(0)   # 0 disables the limit
print("disabled-len:", len(str(10 ** 5000)))

sys.set_int_max_str_digits(700)
show("limit-700", lambda: str(10 ** 800))

# Restore default.
sys.set_int_max_str_digits(4300)
print("restored:", sys.get_int_max_str_digits())
