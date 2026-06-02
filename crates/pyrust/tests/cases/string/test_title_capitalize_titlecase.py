# Parity fixture for #2040: str.title() / str.capitalize() must use the Unicode
# *titlecase* mapping for the first cased character of each word, not the
# uppercase mapping. The Lt digraphs (ǆ ǳ ǉ ǌ …) titlecase to ǅ ǲ ǈ ǋ, and
# SpecialCasing entries (ß→Ss, ﬀ→Ff) apply too.

# Lt digraphs titlecase (not uppercase)
print("ǆ".title())  # ǆ -> ǅ
print("ǳ".title())  # ǳ -> ǲ
print("ǉ".title())  # ǉ -> ǈ
print("ǌ".title())  # ǌ -> ǋ
print("ǆenix".title())  # ǆenix -> ǅenix
print("ǆabc".capitalize())  # ǆabc -> ǅabc

# upper() still uppercases the whole digraph
print("ǆ".upper())  # -> Ǆ

# SpecialCasing titlecase entries
print("ßabc".capitalize())  # ßabc -> Ssabc
print("ﬀx".title())  # ﬀx -> Ffx

# ASCII / common cases unchanged
print("dz".title())
print("hello world".title())
print("HELLO".capitalize())
print("123abc def".title())
print("".title())
print("".capitalize())

assert "ǆ".title() == "ǅ"
assert "ǆ".upper() == "Ǆ"
assert "ǆabc".capitalize() == "ǅabc"
assert "dz".title() == "Dz"
assert "hello world".title() == "Hello World"
