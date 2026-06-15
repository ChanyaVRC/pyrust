# Parity fixture for issue #2489: an invalid conversion character after `!` in
# an f-string replacement field must raise SyntaxError with CPython 3.12's exact
# wording.
#
# CPython 3.12 distinguishes two cases:
#   - a real char that isn't s/r/a:
#       f-string: invalid conversion character 'X': expected 's', 'r', or 'a'
#   - the conversion is absent (terminator `}`/`:` or whitespace follows `!`):
#       f-string: missing conversion character
#
# `str(e)` carries a trailing " (<file>, line N)" location annotation in CPython
# that pyrust does not emit, so we compare the leading message only.


def msg(src):
    try:
        compile(src, "<test>", "eval")
        return "NO ERROR"
    except SyntaxError as e:
        return str(e).split(" (")[0]


# --- invalid conversion character ---
print(msg('f"{x!z}"'))
print(msg('f"{x:{y!z}}"'))  # nested replacement field inside a format spec
print(msg('f"{x=!z}"'))  # debug (`=`) form
print(msg('f"{x!ñ}"'))  # non-ASCII char printed literally

# --- missing conversion character ---
print(msg('f"{x!}"'))  # `!` immediately closed
print(msg('f"{x!:>5}"'))  # `!` followed by a format spec
print(msg('f"{x! }"'))  # `!` followed by whitespace
print(msg('f"{x=!}"'))  # debug form, no conversion
print(msg('f"{x:{y!}}"'))  # nested field, no conversion

# --- valid conversions still parse and evaluate ---
x = 42
print(f"{x!s}")
print(f"{x!r}")
print(f"{x!a}")
print(f"{x=!r}")
