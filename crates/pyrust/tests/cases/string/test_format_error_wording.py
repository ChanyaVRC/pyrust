# Parity fixture for format-spec / format-template error wording vs CPython
# 3.12 (#2355, #2373, #2378, #2379).  Each branch prints the message CPython
# raises; the harness diffs it against pyrust byte-for-byte.

# ── #2355: invalid format specifier names the spec + the value's type ─────────
# Reaches the error through all three entry points (f-string, format(),
# str.format) and for several receiver types, including a subclass and bool
# (CPython names the *actual* type, e.g. 'I' / 'bool').
for value in (5, 5.0, "x", 1j, True):
    # format() builtin
    try:
        format(value, ".2f.3")
    except ValueError as e:
        print(e)
    # str.format
    try:
        "{:.2f.3}".format(value)
    except ValueError as e:
        print(e)

# f-string path (literal spec)
try:
    f"{5:.2f.3}"
except ValueError as e:
    print(e)


class I(int):
    pass


try:
    format(I(5), ".2f.3")
except ValueError as e:
    print(e)

# ── #2373a: grouping separator combined with the 's' presentation type ────────
# CPython names the *actual* separator that was supplied.
try:
    format("x", ",s")
except ValueError as e:
    print(e)
try:
    format("x", "_s")
except ValueError as e:
    print(e)
try:
    "{:,s}".format("abc")
except ValueError as e:
    print(e)

# ── #2373b: unknown format codes — non-ASCII / astral code points escaped ─────
# Code points in 0x20..=0x7f (note: DEL, 0x7f) print literally; control
# characters (< 0x20) and non-ASCII / astral code points (>= 0x80) are escaped
# as the lowercase, non-zero-padded \xHEX of the code point.
for code in ("Q", "\U0001d11e", "é", "Δ", "\x01", "\x7f", "\x80", "Ā"):
    try:
        format(3.14, ">>" + code)
    except ValueError as e:
        print(e)
# Same escaping on int / str receivers.
try:
    format(5, "Q")
except ValueError as e:
    print(e)
try:
    format("x", "Q")
except ValueError as e:
    print(e)
try:
    format(3.14, "\U0001d11e")
except ValueError as e:
    print(e)

# ── #2378: unterminated accessor in a format field ('[' with a closing '}'
# but no ']') reports CPython's "expected '}' before end of string".
try:
    "{a[}".format(a=1)
except ValueError as e:
    print(e)
try:
    "{0[}".format(1)
except ValueError as e:
    print(e)

# ── #2379: str.format_map() rejects keyword arguments ─────────────────────────
# Plain str receiver.
try:
    "{a}".format_map({"a": 1}, b=2)
except TypeError as e:
    print(e)
# Subclass receiver (previously dropped the kwarg silently).
class S(str):
    pass


try:
    S("{a}").format_map({"a": 1}, b=2)
except TypeError as e:
    print(e)
# Wrong positional count still reports the count, not the kwarg message.
try:
    "{a}".format_map({"a": 1}, {"b": 2})
except TypeError as e:
    print(e)
try:
    "{a}".format_map()
except TypeError as e:
    print(e)

# Grouping/type compatibility precedes the per-value unknown-code check
# (pinned against python3.12): ',' allows only d/e/E/f/F/g/G/%; '_' adds
# b/o/x/X; doubled or paired separators name themselves.
for v, spec in [("s", ",,"), ("s", "_,"), ("s", ",_"), ("s", ",q"), ("s", "_q"),
                ("s", ",d"), (1, ",,"), (1.5, ",,"), (1, ",c"), (1, ",b"),
                (1, ",o"), (255, "_x"), (5, "_b"), (1, "_%"), (1, "_n"),
                ("t", ",s"), (1.5, ",s"), (1.5, ",€")]:
    try:
        print(repr(spec), "OK", format(v, spec))
    except ValueError as e:
        print(repr(spec), "VE", e)
