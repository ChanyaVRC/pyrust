# Parity fixture for #2080: str.is* classification is pinned to Unicode 15.0
# (the database CPython 3.12 ships).
#
# pyrust's `general_category` / `char::is_*case` predicates track a newer Unicode
# database, which over-counted ~9.5k codepoints assigned in Unicode 16.0/17.0
# (all Cn / Unassigned in 15.0) as letters/digits/cased characters. Each block
# below must print byte-identical to python3.12.

METHODS = (
    "isalpha",
    "isupper",
    "islower",
    "istitle",
    "isalnum",
    "isnumeric",
    "isdecimal",
    "isdigit",
    "isidentifier",
)


def classify(cp):
    c = chr(cp)
    return [getattr(c, m)() for m in METHODS]


# Codepoints assigned in Unicode 16.0/17.0, unassigned (Cn) in 15.0.
# python3.12 classifies every one as False across all is* methods.
post_15 = [
    0x88F,  # Arabic-script addition (16.0)
    0xC5C,  # Telugu letter (16.0)
    0x1C89,  # Cyrillic (16.0, was over-counted upper/title)
    0x1C8A,  # Cyrillic (16.0, was over-counted lower)
    0xA7CB,  # Latin Extended-D (16.0)
    0xA7F1,  # Latin modifier letter (16.0)
    0x105C0,  # Todhri (16.0)
    0x10D40,  # Garay digit (16.0) — was over-counted decimal/digit
    0x10D70,  # Garay letter (16.0) — was over-counted lower
    0x11380,  # Tulu-Tigalari (16.0)
    0x116D0,  # Myanmar digit (16.0)
    0x16100,  # Gurung Khema (16.0)
    0x16D40,  # Kirat Rai (16.0)
    0x1E5D0,  # Ol Onal (16.0)
    0x1E6C0,  # Sidetic (17.0)
    0x2EBF0,  # CJK Ext I (16.0)
    0x323B0,  # CJK Ext J (17.0)
]
for cp in post_15:
    print(hex(cp), classify(cp))

# Common ranges that are unchanged across Unicode versions: every result here
# must match python3.12 exactly (regression guard for #2040/#2074/#2105).
unchanged = [
    ord("A"),  # Lu
    ord("a"),  # Ll
    ord("5"),  # Nd
    ord("_"),
    0x00B2,  # superscript two (No, Numeric_Type=Digit)
    0x00DF,  # ß
    0x01C5,  # ǅ titlecase letter (Lt)
    0x0391,  # Greek capital alpha
    0x03B1,  # Greek small alpha
    0x0660,  # Arabic-Indic digit zero (Nd)
    0x4E00,  # CJK 一
    0x2167,  # Ⅷ roman numeral (Nl)
    0x2460,  # ① circled digit one (No, Numeric_Type=Digit)
    0x1F600,  # 😀 emoji (So)
    0x10000,  # Linear B syllable (Lo)
]
for cp in unchanged:
    print(hex(cp), classify(cp))

# Whole-string behaviour over mixed content.
print("HÉLLO".isupper())  # True
print("héllo".islower())  # True
print("Hello World".istitle())  # True
print("Ⅷ123".isnumeric())  # True
print(("a" + chr(0x88F)).isalpha())  # False: post-15.0 codepoint is not a letter
print(("a" + chr(0x10D40)).isalnum())  # False: post-15.0 digit is Cn in 15.0
