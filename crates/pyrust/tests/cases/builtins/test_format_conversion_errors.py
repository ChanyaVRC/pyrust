# Parity test for issue #2484: str.format / str.format_map must raise
# ValueError (not KeyError) for a malformed conversion flag.
#
# A replacement field's conversion is a single char after '!' that must be
# followed by the field end or ':'. CPython 3.12 wording:
#   - bare '!' (no conversion char)        -> "unmatched '{' in format spec"
#   - more than one char after '!'          -> "expected ':' after conversion specifier"
#   - a recognised-but-unknown conversion   -> "Unknown conversion specifier <c>"
# Previously pyrust folded the malformed conversion back into the field name,
# so "{x!}".format(x=1) looked up the key "x!" and raised KeyError.


def show(label, fn):
    try:
        print(label, "->", repr(fn()))
    except Exception as e:
        print(label, "->", type(e).__name__ + ":", e)


# --- malformed conversions: str.format ---
show("{x!}", lambda: "{x!}".format(x=1))
show("{x!ab}", lambda: "{x!ab}".format(x=1))
show("{x!r!s}", lambda: "{x!r!s}".format(x=1))
show("{x!z}", lambda: "{x!z}".format(x=1))

# --- malformed conversions: str.format_map ---
show("map {x!}", lambda: "{x!}".format_map({"x": 1}))
show("map {x!ab}", lambda: "{x!ab}".format_map({"x": 1}))
show("map {x!z}", lambda: "{x!z}".format_map({"x": 1}))

# --- unknown conversion char renders CPython-style: printable ASCII literal,
#     everything else as '\\x' + minimal lowercase hex (matches %c) ---
show("{x! }", lambda: "{x! }".format(x=1))  # space -> \x20
show("{x!\xf1}", lambda: "{x!\xf1}".format(x=1))  # ñ -> \xf1
show("{x!€}", lambda: "{x!€}".format(x=1))  # € -> \x20ac
show("{x!\U0001f600}", lambda: "{x!\U0001f600}".format(x=1))  # 😀 -> \x1f600

# --- error ordering: an earlier complete field renders/raises first ---
show("{a} {x!}", lambda: "{a} {x!}".format(a=1, x=2))
show("{missing} {x!}", lambda: "{missing} {x!}".format(x=2))

# --- valid conversions are unchanged ---
show("{x!r}", lambda: "{x!r}".format(x=[1, 2]))
show("{x!s}", lambda: "{x!s}".format(x=42))
show("{x!a}", lambda: "{x!a}".format(x="ñ"))
show("{0!r:>6}", lambda: "{0!r:>6}".format("hi"))
show("{!r}", lambda: "{!r}".format("hi"))
show("{x!r:d}", lambda: "{x!r:d}".format(x=255))
show("{x!s:{w}}", lambda: "{x!s:{w}}".format(x=42, w=5))

# --- '!' inside an accessor subscript stays part of the field name ---
show("{d[a!b]}", lambda: "{d[a!b]}".format(d={"a!b": 9}))
show("{d[a!]}", lambda: "{d[a!]}".format(d={"a!": 7}))
show("{d[!]}", lambda: "{d[!]}".format(d={"!": 5}))

# --- ':' takes precedence: '!' after the spec colon is literal spec text ---
show("{x:!}", lambda: "{x:!}".format(x=1))

# --- no-conversion happy paths (no regression) ---
show("{}", lambda: "{}".format(1))
show("{x}", lambda: "{x}".format(x="ok"))
