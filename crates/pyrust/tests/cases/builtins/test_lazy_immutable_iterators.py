# Immutable bytes/string iterators advance from the source without eagerly
# allocating one Value per element.

data = bytes([0, 127, 128, 255])
it = iter(data)
print(type(it).__name__, next(it), list(it))

text = "Aé😀Z"
it = iter(text)
print(type(it).__name__, next(it), list(it))

# PyRust stores lone surrogates as CESU-8 internally; iteration must preserve
# them as one Python codepoint without asking Rust's `char` decoder to accept
# the surrogate.
surrogate = chr(0xD800) + "x"
print([hex(ord(ch)) for ch in surrogate])

# Iterator aliases share one cursor, as required for iterator objects.
it = iter("éab")
alias = it
print(next(it), next(alias), list(it))
