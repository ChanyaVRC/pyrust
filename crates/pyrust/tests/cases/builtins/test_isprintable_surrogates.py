# str.isprintable() with surrogate codepoints (0xD800-0xDFFF) must return False,
# matching CPython. Surrogates appear in pyrust strings via str.translate().

# Basic printable ASCII
print("".isprintable())           # True
print("hello".isprintable())      # True
print(" ".isprintable())          # True (ASCII space is printable)
print("\t".isprintable())         # False (tab is control)
print("\x00".isprintable())       # False (null is control)

# Non-ASCII printable
print("é".isprintable())     # True (é)
print("中".isprintable())     # True (CJK)

# Surrogate codepoints via translate -- must be False, not panic
s_d800 = "a".translate({ord("a"): 0xD800})
print(s_d800.isprintable())       # False

s_dfff = "a".translate({ord("a"): 0xDFFF})
print(s_dfff.isprintable())       # False

s_d900 = "a".translate({ord("a"): 0xD900})
print(s_d900.isprintable())       # False

# String with surrogate mixed with printable char
s_mixed = "a".translate({ord("a"): 0xD800}) + "b"
print(s_mixed.isprintable())      # False (surrogate makes whole string non-printable)

# Just below surrogate range (Cn -- unassigned, not printable)
print(chr(0xD7FF).isprintable())  # False

# Just above surrogate range (Co -- private use, not printable)
print(chr(0xE000).isprintable())  # False
