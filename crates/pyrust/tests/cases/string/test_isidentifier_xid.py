# Parity fixture for #2047: str.isidentifier() must use the Unicode
# XID_Start / XID_Continue properties (plus '_'), not is_alphabetic /
# is_alphanumeric. Combining marks are XID_Continue but not alphanumeric;
# superscripts are alphanumeric but not XID_Continue.

# a + combining acute accent: XID_Continue, so identifier
print(("á").isidentifier())  # True
# superscript two: alphanumeric but NOT XID_Continue
print("a²".isidentifier())  # False
# precomposed / non-Latin letters
print("naïve".isidentifier())  # True
print("π".isidentifier())  # True
print("変数".isidentifier())  # True
# Arabic-Indic digit as continue char
print("a٠".isidentifier())  # True
# leading digit / space / empty
print("1abc".isidentifier())  # False
print(" a".isidentifier())  # False
print("".isidentifier())  # False
# underscore start and middle
print("_x".isidentifier())  # True
print("__init__".isidentifier())  # True
# a combining mark cannot START an identifier
print("́a".isidentifier())  # False
# ASCII common cases
print("a1".isidentifier())  # True
print("foo_bar".isidentifier())  # True

assert ("á").isidentifier() is True
assert "a²".isidentifier() is False
assert "π".isidentifier() is True
assert " a".isidentifier() is False
