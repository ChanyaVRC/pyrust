# Parity fixture for #2051: isspace / isnumeric / isalnum edge cases.

# isspace: CPython's whitespace set includes the C0 information separators
# \x1c-\x1f and NEL \x85, which Rust's char::is_whitespace omits.
print("\x1c".isspace())  # True
print("\x1d".isspace())  # True
print("\x1e".isspace())  # True
print("\x1f".isspace())  # True
print("\x85".isspace())  # True
print(" ".isspace())  # True
print("\t\n\r".isspace())  # True
print(" ".isspace())  # True (no-break space)
print("a".isspace())  # False
print("".isspace())  # False

# isnumeric: includes CJK ideographic numerals (category Lo, Numeric_Type=Numeric)
print("一".isnumeric())  # True
print("二十百五万参".isnumeric())  # True
print("Ⅷ".isnumeric())  # True (roman numeral, Nl)
print("½".isnumeric())  # True (vulgar fraction, No)
print("²".isnumeric())  # True (superscript two)
print("123".isnumeric())  # True
print("12a".isnumeric())  # False
print("".isnumeric())  # False

# isalnum: circled letters (category So) are neither alpha nor numeric -> False
print("Ⓐ".isalnum())  # False
print("Ⓐ".isalpha(), "Ⓐ".isnumeric(), "Ⓐ".isdigit(), "Ⓐ".isdecimal())
print("一".isalnum())  # True (numeric)
print("abc123".isalnum())  # True
print("naïve".isalnum())  # True
print("a b".isalnum())  # False
print("".isalnum())  # False

assert "\x1c".isspace() is True
assert "一".isnumeric() is True
assert "Ⓐ".isalnum() is False
assert " ".isspace() is True
assert "abc".isalnum() is True
