# Comprehensive CPython 3.12 parity tests for `unicodedata` (issue #3203).

import unicodedata
import unicodedata as ud

# 1. Version & UCD
print("version:", ud.unidata_version)
print("ucd_3_2_0:", isinstance(ud.ucd_3_2_0, ud.UCD), ud.ucd_3_2_0.unidata_version)

# 2. category, bidirectional, combining, east_asian_width, mirrored, decomposition
sample_chars = [
    "\x00",        # Cc, BN
    " ",           # Zs, WS
    "A",           # Lu, L, Na
    "z",           # Ll, L, Na
    "\u01c5",      # Lt (Dz titlecase)
    "\u02b0",      # Lm
    "\u4e00",      # Lo, W (CJK)
    "\uac00",      # Lo, W (Hangul GA)
    "\u0301",      # Mn, NSM, combining=230
    "\u0327",      # Mn, NSM, combining=202
    "5",           # Nd, EN
    "\u0665",      # Arabic-Indic digit 5
    "\u2164",      # Nl (Roman numeral V)
    "\u00bd",      # No (1/2)
    "(",           # Ps, ON, mirrored=1
    ")",           # Pe, ON, mirrored=1
    "[",           # Ps, mirrored=1
    "/",           # Po, CS, mirrored=0
    "\u05d0",      # Hebrew Alef (R)
    "\u0627",      # Arabic Alef (AL)
    "\u200e",      # LRM (Cf, L)
    "\uff21",      # Fullwidth A (F, <wide> 0041)
    "\uff71",      # Halfwidth Katakana A (H, <narrow> 30A2)
    "\u00e9",      # e with acute (0065 0301)
    "\ufb01",      # fi ligature (<compat> 0066 0069)
    "\u00b2",      # superscript 2 (<super> 0032)
    "\u212b",      # Angstrom sign (00C5)
    "\U0001f600",  # Emoji (So, W)
    "\ud800",      # Lone surrogate (Cs)
    "\ue000",      # Private use (Co)
    "\uffff",      # Noncharacter (Cn)
    "\U0010ffff",  # Max codepoint (Cn)
]

for ch in sample_chars:
    print(
        f"U+{ord(ch):04X}:",
        ud.category(ch),
        repr(ud.bidirectional(ch)),
        ud.combining(ch),
        ud.east_asian_width(ch),
        ud.mirrored(ch),
        type(ud.mirrored(ch)) is int,
        repr(ud.decomposition(ch)),
    )

# 3. decimal, digit, numeric (with and without defaults)
num_chars = [
    "0",
    "9",
    "\u0667",      # Arabic-Indic 7 (dec=7, dig=7, num=7.0)
    "\u096b",      # Devanagari 5
    "\u00b2",      # Superscript 2 (no dec, dig=2, num=2.0)
    "\u2460",      # Circled 1 (no dec, dig=1, num=1.0)
    "\u00bd",      # 1/2 (no dec, no dig, num=0.5)
    "\u2153",      # 1/3
    "\u216b",      # Roman XII (num=12.0)
    "\u4e00",      # CJK one (num=1.0)
    "\u5104",      # CJK 100 million (num=100000000.0)
    "A",           # Non-numeric
]
for ch in num_chars:
    print(
        f"num U+{ord(ch):04X}:",
        ud.decimal(ch, -1),
        ud.digit(ch, -1),
        ud.numeric(ch, -1),
    )

for fn in (ud.decimal, ud.digit, ud.numeric):
    try:
        fn("A")
    except ValueError as e:
        print(fn.__name__, "ValueError:", str(e))

# 4. name and lookup
named_chars = [
    " ",
    "A",
    "\u00e9",
    "\u2603",
    "\u4e00",
    "\u9fff",
    "\U00020000",
    "\U00030000",
    "\uac00",
    "\ud7a3",
    "\ud55c",
    "\uae00",
    "\U0001f600",
    "\uf900",
]
for ch in named_chars:
    nm = ud.name(ch)
    back = ud.lookup(nm)
    print(f"name U+{ord(ch):04X}:", nm, back == ch)

# Case-insensitivity on standard names vs exact uppercase on algorithmic names
print("lookup lower std:", ud.lookup("latin small letter a") == "a")
for q in ["cjk unified ideograph-4e00", "CJK UNIFIED IDEOGRAPH-4e00", "hangul syllable ga", "HANGUL SYLLABLE ga"]:
    try:
        ud.lookup(q)
        print("UNEXPECTED:", q)
    except KeyError as e:
        print("algo case KeyError:", str(e))

# Unnamed chars with and without default
for ch in ["\x00", "\n", "\ud800", "\ue000", "\uffff"]:
    print(f"unnamed U+{ord(ch):04X}:", repr(ud.name(ch, None)), repr(ud.name(ch, "NONE")))
    try:
        ud.name(ch)
    except ValueError as e:
        print("  err:", str(e))

# Name aliases and strict lookup rejections
for alias in ["LINE FEED", "LF", "NL", "NUL", "ALERT", "BELL", "LATIN CAPITAL LETTER GHA"]:
    got = ud.lookup(alias)
    print(f"alias {alias!r}:", hex(ord(got)))

print("lookup bytes:", repr(ud.lookup(b"LATIN CAPITAL LETTER A")))

for bad_name in [
    "",
    "NO SUCH CHARACTER",
    "latinsmalllettera",
    "LATIN_SMALL_LETTER_A",
    "LOW-LINE",
    "CJK UNIFIED IDEOGRAPH-2A6FF",
]:
    try:
        ud.lookup(bad_name)
        print("UNEXPECTED HIT:", bad_name)
    except KeyError as e:
        print("lookup KeyError:", str(e))

# 5. normalize and is_normalized
norm_cases = [
    "",
    "hello world 123",
    "caf\u00e9",
    "cafe\u0301",
    "\u212b",                      # Angstrom -> A-ring -> A + ring
    "\ufb01",                      # fi ligature
    "\u00bd",                      # 1/2
    "\u00b2",                      # superscript 2
    "\uff21\uff22\uff23",          # Fullwidth ABC
    "\uac00\ud55c\uae00",          # Hangul syllables (NFC)
    "\u1100\u1161\u1112\u1161\u11ab\u1100\u1173\u11af",  # Hangul jamo (NFD)
    "a\u0301\u0327",               # Multiple combining marks (ccc 230, 202 -> reordered)
    "a\u0327\u0301",               # Already canonically ordered (ccc 202, 230)
    "\u1e69",                      # s with dot below and dot above (multi-step compose)
    "s\u0307\u0323",               # Reorder + compose -> U+1E69
    "\u03d3",                      # Greek Upsilon with acute and hook symbol
    "\uf900",                      # CJK compatibility ideograph -> U+8C48 (composition exclusion)
    "x\ud800y",                    # Lone surrogate preserved
]

for form in ("NFC", "NFD", "NFKC", "NFKD"):
    for text in norm_cases:
        res = ud.normalize(form, text)
        is_n = ud.is_normalized(form, text)
        is_n_res = ud.is_normalized(form, res)
        print(
            form,
            [hex(ord(c)) for c in text],
            "->",
            [hex(ord(c)) for c in res],
            is_n,
            is_n_res,
        )

# 6. Error handling & type checks
for fn in (ud.category, ud.bidirectional, ud.combining, ud.east_asian_width, ud.mirrored, ud.decomposition):
    for bad in ("", "ab", 123):
        try:
            fn(bad)
        except TypeError as e:
            print(fn.__name__, type(bad).__name__, "TypeError:", str(e))

for bad_form in ("nfc", "INVALID", ""):
    for fn in (ud.normalize, ud.is_normalized):
        try:
            fn(bad_form, "abc")
        except ValueError as e:
            print(fn.__name__, repr(bad_form), "ValueError:", str(e))

try:
    ud.normalize(123, "abc")
except TypeError as e:
    print("normalize arg1 TypeError:", str(e))

try:
    ud.normalize("NFC", 123)
except TypeError as e:
    print("normalize arg2 TypeError:", str(e))

try:
    ud.lookup(123)
except TypeError as e:
    print("lookup TypeError:", str(e))
