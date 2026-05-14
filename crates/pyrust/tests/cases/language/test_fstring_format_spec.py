# f-string format-spec mini-language: fill / align / sign / # / 0 / width /
# grouping / .precision / type.  Regression for issue #388 where zero-pad,
# alignment, and fill characters were parsed but silently dropped.

# --- Sign ---
assert f"{42:+}" == "+42"
assert f"{42: }" == " 42"
assert f"{42:-}" == "42"
assert f"{-42:+}" == "-42"
assert f"{-42: }" == "-42"
assert f"{3.14:+}" == "+3.14"

# --- Zero-pad (the headline bug) ---
assert f"{42:05}" == "00042"
assert f"{42:05d}" == "00042"
assert f"{-42:05}" == "-0042"
assert f"{-42:+05}" == "-0042"
assert f"{42:+05}" == "+0042"
assert f"{3.14:08.2f}" == "00003.14"
assert f"{-3.14:08.2f}" == "-0003.14"
assert f"{3.14:+08.2f}" == "+0003.14"

# --- Explicit fill / align (with and without zero-pad) ---
assert f"{42:>10}" == "        42"
assert f"{42:<10}|" == "42        |"
assert f"{42:^10}|" == "    42    |"
assert f"{42:*>10}" == "********42"
assert f"{42:>05}" == "00042"
assert f"{42:<05}" == "42000"
assert f"{42:^05}" == "04200"
assert f"{42:*>05}" == "***42"
assert f"{42:0>5}" == "00042"
assert f"{-42:>05}" == "00-42"

# --- Alternate form (#) ---
assert f"{255:#x}" == "0xff"
assert f"{255:#X}" == "0XFF"
assert f"{255:#o}" == "0o377"
assert f"{255:#b}" == "0b11111111"
assert f"{255:#08x}" == "0x0000ff"
assert f"{0:#x}" == "0x0"

# --- Grouping (',' and '_') ---
assert f"{1234567:_}" == "1_234_567"
assert f"{1234567:,}" == "1,234,567"
assert f"{-12345:,}" == "-12,345"
assert f"{-12345:08,}" == "-012,345"
assert f"{1234.5:,.2f}" == "1,234.50"
assert f"{12345.6789:,.4f}" == "12,345.6789"
assert f"{255:_x}" == "ff"          # underscore allowed on hex
assert f"{0xdeadbeef:_x}" == "dead_beef"

# --- Width + precision ---
assert f"{3.14159:.3f}" == "3.142"
assert f"{3.14159:10.3f}" == "     3.142"
assert f"{3.14159:<10.3f}|" == "3.142     |"
assert f"{42:5d}" == "   42"

# --- String formatting ---
assert f"{'hello':*^15}" == "*****hello*****"
assert f"{'hello':.3}" == "hel"
assert f"{'hello':10}" == "hello     "
assert f"{'hello':>10}" == "     hello"

# --- Integer type codes ---
assert f"{255:b}" == "11111111"
assert f"{255:o}" == "377"
assert f"{255:x}" == "ff"
assert f"{255:X}" == "FF"
assert f"{65:c}" == "A"
assert f"{True:d}" == "1"
assert f"{True:5}" == "    1"
assert f"{False:>5}" == "    0"

# --- Float type codes ---
assert f"{1.5e-5:.2e}" == "1.50e-05"
assert f"{1234.5:e}" == "1.234500e+03"
assert f"{0.5:.0%}" == "50%"
assert f"{42:%}" == "4200.000000%"

# --- Empty spec / no spec ---
assert f"{42:}" == "42"
assert f"{42}" == "42"
assert f"{3.14}" == "3.14"
assert f"{'hi'}" == "hi"

# --- Errors: type mismatches ---
try:
    s = f"{42:s}"
    raise AssertionError("expected ValueError on 42:s")
except ValueError:
    pass

try:
    s = f"{3.14:d}"
    raise AssertionError("expected ValueError on 3.14:d")
except ValueError:
    pass

try:
    s = f"{'x':d}"
    raise AssertionError("expected ValueError on 'x':d")
except ValueError:
    pass

print("fstring format spec OK")
