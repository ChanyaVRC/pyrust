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

# --- multi-char NAME conversion segment (issue #2496): the whole segment is
# lexed as a NAME and quoted in the message ---
print(msg('f"{x!sr}"'))  # starts with a valid char then more
print(msg('f"{x!ra}"'))
print(msg('f"{x!sa}"'))  # two valid chars, still invalid as a segment
print(msg('f"{x!zzz}"'))
print(msg('f"{x=!sr}"'))  # debug form, multi-char
print(msg('f"{x:{y!sr}}"'))  # nested field, multi-char
print(msg('f"{x!_}"'))  # `_` is a NAME-start char, so it is quoted

# --- non-name-start conversion segment (issue #2496): nothing is captured as a
# NAME, so the message carries no quoted char ---
print(msg('f"{x!5}"'))
print(msg('f"{x!.}"'))
print(msg('f"{x=!5}"'))  # debug form
print(msg('f"{x:{y!5}}"'))  # nested field

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
