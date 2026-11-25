# Comprehensive CPython 3.12 parity tests for `stringprep` (RFC 3454) (issue #3250).

import stringprep

# 1. Table A.1 (unassigned code points in Unicode 3.2)
for cp in [0x0041, 0x0221, 0x0234, 0x0378, 0x0380, 0x0530, 0xFDD0, 0xFDEF, 0xFFFE, 0xFFFF, 0x1F600, 0x10FFFF]:
    print("a1", hex(cp), stringprep.in_table_a1(chr(cp)))

# 2. Table B.1 (commonly mapped to nothing)
for cp in [0x0041, 0x00AD, 0x034F, 0x1806, 0x180B, 0x200B, 0x200C, 0x200D, 0x2060, 0xFE00, 0xFE0F, 0xFEFF]:
    print("b1", hex(cp), stringprep.in_table_b1(chr(cp)))

# 3. Table B.2 and B.3 (case folding mappings)
for cp in [
    0x0041, 0x0061, 0x00B5, 0x00DF, 0x0130, 0x0149, 0x017F, 0x0390,
    0x03C2, 0x03F9, 0x1E9E, 0x1F80, 0x2122, 0xFB00, 0xFB03, 0x1D400, 0x1D6A8,
]:
    c = chr(cp)
    print("b2b3", hex(cp), repr(stringprep.map_table_b2(c)), repr(stringprep.map_table_b3(c)))

# 4. Tables C.1.1, C.1.2, C.1.1+C.1.2 (spaces)
for cp in [0x0020, 0x0041, 0x00A0, 0x1680, 0x2000, 0x200A, 0x202F, 0x205F, 0x3000]:
    c = chr(cp)
    print("c1", hex(cp), stringprep.in_table_c11(c), stringprep.in_table_c12(c), stringprep.in_table_c11_c12(c))
print("c11 non-space:", stringprep.in_table_c11(""), stringprep.in_table_c11("  "), stringprep.in_table_c11(123))

# 5. Tables C.2.1, C.2.2, C.2.1+C.2.2 (control characters)
for cp in [0x0000, 0x001F, 0x0020, 0x007F, 0x0080, 0x009F, 0x06DD, 0x070F, 0x180E, 0x200C, 0x2028, 0x2060, 0xFEFF, 0xFFF9, 0x1D173]:
    c = chr(cp)
    print("c2", hex(cp), stringprep.in_table_c21(c), stringprep.in_table_c22(c), stringprep.in_table_c21_c22(c))

# 6. Tables C.3..C.9 (private use, non-character, surrogate, inappropriate, bidi, tagging)
for cp in [0x0041, 0xE000, 0xF8FF, 0xF0000, 0x10FFFF, 0xFDD0, 0xFDEF, 0xFFFE, 0xD800, 0xDFFF]:
    c = chr(cp)
    print("c345", hex(cp), stringprep.in_table_c3(c), stringprep.in_table_c4(c), stringprep.in_table_c5(c))

for cp in [0x0041, 0xFFF9, 0xFFFD, 0x2FF0, 0x2FFB, 0x0340, 0x200E, 0x202A, 0x206A, 0xE0001, 0xE0020, 0xE007F]:
    c = chr(cp)
    print(
        "c6789",
        hex(cp),
        stringprep.in_table_c6(c),
        stringprep.in_table_c7(c),
        stringprep.in_table_c8(c),
        stringprep.in_table_c9(c),
    )

# 7. Tables D.1 and D.2 (BiDi R/AL and L)
for cp in [0x0041, 0x0030, 0x05BE, 0x05D0, 0x0627, 0x0660, 0x200F, 0xFB1D, 0xFE70]:
    c = chr(cp)
    print("d12", hex(cp), stringprep.in_table_d1(c), stringprep.in_table_d2(c))

# 8. Error handling for non-single-character inputs
for fn_name in ["in_table_a1", "in_table_b1", "in_table_c5", "in_table_d1", "map_table_b2", "map_table_b3"]:
    fn = getattr(stringprep, fn_name)
    for bad in ["", "ab", 42]:
        try:
            fn(bad)
            print("UNEXPECTED:", fn_name, repr(bad))
        except TypeError:
            print("typeerror ok:", fn_name, type(bad).__name__)
