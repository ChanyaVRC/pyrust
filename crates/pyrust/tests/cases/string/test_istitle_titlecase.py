# Parity fixture for #2105: str.istitle() must treat Unicode titlecase (Lt)
# characters like ǅ (U+01C5), ǈ, ǋ, ǲ as uppercase/word-start characters, the
# same way CPython's unicode_istitle does. Distinct from #2040 (title() output).

# A single Lt char is titlecased.
print("ǅ".istitle())  # True
# Lt starts a word, followed by lowercase.
print("ǅabc".istitle())  # True
# Lt after lowercase breaks titlecasing (must follow a non-cased char).
print("Abǅ".istitle())  # False
# Two Lt chars in a row: second follows a cased char -> not titlecased.
print("ǅǅ".istitle())  # False
# Lt chars separated by a space each start their own word.
print("ǅ ǅ".istitle())  # True
# The lowercase digraph (Ll) is not titlecased on its own.
print("ǆ".istitle())  # False
# Uppercase digraph (Lu) is titlecased as a single-letter word.
print("Ǆ".istitle())  # True
# title() output round-trips through istitle().
print("dz x".title().istitle())  # True
print("ǆenix".title().istitle())  # True

# Other Lt digraphs.
print("ǲ".istitle())  # True
print("ǈ".istitle())  # True
print("ǋ".istitle())  # True
print("ǳ".istitle())  # False (Ll)

# ASCII behaviour unchanged.
print("Hello World".istitle())  # True
print("hello".istitle())  # False
print("HELLO".istitle())  # False
print("Ab Cd".istitle())  # True
print("AbC".istitle())  # False

# Edge cases.
print("".istitle())  # False
print("A".istitle())  # True
print("a".istitle())  # False
print("123".istitle())  # False
print("A1b".istitle())  # False
print("Title Case With Numbers 123".istitle())  # True
