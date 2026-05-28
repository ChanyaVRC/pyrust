# Test str.isdigit() against the full Unicode Numeric_Type=Digit set (CPython 3.12 parity).

# ASCII digits — always True
print("0".isdigit())   # True
print("9".isdigit())   # True

# Superscript digits (were already handled)
print("²".isdigit())  # ² superscript 2 → True
print("³".isdigit())  # ³ superscript 3 → True
print("¹".isdigit())  # ¹ superscript 1 → True
print("⁰".isdigit())  # ⁰ superscript 0 → True
print("⁴".isdigit())  # ⁴ superscript 4 → True
print("⁹".isdigit())  # ⁹ superscript 9 → True

# Subscript digits (were already handled)
print("₀".isdigit())  # ₀ subscript 0 → True
print("₉".isdigit())  # ₉ subscript 9 → True

# Circled digits 1–9 (U+2460–U+2468) — the primary bug report
print("①".isdigit())  # ① circled digit 1 → True
print("⑤".isdigit())  # ⑤ circled digit 5 → True
print("⑨".isdigit())  # ⑨ circled digit 9 → True

# Parenthesized digits 1–9 (U+2474–U+247C)
print("⑴".isdigit())  # ⑴ → True
print("⑼".isdigit())  # ⑼ → True

# Digit full-stop 1–9 (U+2488–U+2490)
print("⒈".isdigit())  # ⒈ → True
print("⒐".isdigit())  # ⒐ → True

# Circled digit 0 (U+24EA)
print("⓪".isdigit())  # ⓪ → True

# Double circled digits 1–9 (U+24F5–U+24FD)
print("⓵".isdigit())  # ❵ → True
print("⓽".isdigit())  # → True

# Negative circled digit 0 (U+24FF)
print("⓿".isdigit())  # ⓿ → True

# Dingbat negative circled digits 1–9 (U+2776–U+277E)
print("❶".isdigit())  # ❶ → True
print("❾".isdigit())  # ❾ → True

# Dingbat circled sans-serif digits 1–9 (U+2780–U+2788)
print("➀".isdigit())  # ➀ → True
print("➈".isdigit())  # ➈ → True

# Dingbat negative circled sans-serif digits 1–9 (U+278A–U+2792)
print("➊".isdigit())  # ➊ → True
print("➒".isdigit())  # ➒ → True

# Ethiopic digits 1–9 (U+1369–U+1371)
print("፩".isdigit())  # ፩ → True
print("፱".isdigit())  # ፱ → True

# New Tai Lue Tham Digit One (U+19DA)
print("᧚".isdigit())  # ᧚ → True

# Kharoshthi digits 1–4 (U+10A40–U+10A43)
print("\U00010a40".isdigit())  # → True
print("\U00010a43".isdigit())  # → True

# Rumi digits 1–9 (U+10E60–U+10E68)
print("\U00010e60".isdigit())  # → True
print("\U00010e68".isdigit())  # → True

# Brahmi numbers 1–9 (U+11052–U+1105A)
print("\U00011052".isdigit())  # → True
print("\U0001105a".isdigit())  # → True

# Digit comma/full-stop (U+1F100–U+1F10A)
print("\U0001f100".isdigit())  # 🄀 → True
print("\U0001f10a".isdigit())  # → True

# Non-digit characters — must stay False
print("a".isdigit())   # False
print("".isdigit())    # False
print(" ".isdigit())   # False
print("½".isdigit())  # ½ fraction (No, but not Digit) → False
print("A".isdigit())  # A → False

# Mixed string — False if any non-digit
print("1a".isdigit())  # False
print("12".isdigit())  # True
