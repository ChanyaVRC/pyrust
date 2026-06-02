# Parity fixture for #2048: str.casefold() must apply the CaseFolding.txt
# mappings where the fold target differs from the lowercase mapping, e.g.
# MICRO SIGN µ (U+00B5) -> GREEK SMALL LETTER MU μ (U+03BC) and
# GREEK SMALL LETTER FINAL SIGMA ς (U+03C2) -> GREEK SMALL LETTER SIGMA σ.

print("µ".casefold() == "μ")  # micro sign -> mu
print("ς".casefold() == "σ")  # final sigma -> sigma
print("µ".casefold())  # µ -> μ
print("ς".casefold())  # ς -> σ

# Existing multi-char foldings still work
print("ß".casefold())  # ss
print("ﬆ".casefold())  # st
print("ẞ".casefold())  # capital sharp s -> ss
print("Ω".casefold())  # ohm sign -> omega

# A whole word using both
print("ΣΣµ".casefold())  # -> σσμ

# ASCII unchanged
print("Hello".casefold())

assert "µ".casefold() == "μ"
assert "ς".casefold() == "σ"
assert "ß".casefold() == "ss"
assert "Ω".casefold() == "ω"
