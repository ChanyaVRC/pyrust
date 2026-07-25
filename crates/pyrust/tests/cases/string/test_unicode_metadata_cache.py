# Non-ASCII strings cache their Python codepoint length and sparse byte offsets.
text = "日本語🙂" * 40
print(len(text), len(text), text[0], text[57], text[-1])
print(text[31:37], text[80:96], text[-9:-2])

# A long substring uses the slice descriptor layout and owns an independent
# cache while sharing its immutable bytes with the root.
view = text[13:109]
print(len(view), len(view), view[0], view[63], view[-1], view[7:42])

# In-place append must invalidate metadata built for the old byte sequence.
grown = "αβγδ" * 20
print(len(grown), grown[-1])
grown += "界🙂"
print(len(grown), len(grown), grown[-2], grown[-1])

# Lone surrogates use pyrust's CESU-8 representation and still count/index as
# one Python codepoint apiece.
surrogates = "\ud800a\udfff" * 8
print(len(surrogates), ord(surrogates[0]), ord(surrogates[2]), repr(surrogates[1:8]))
