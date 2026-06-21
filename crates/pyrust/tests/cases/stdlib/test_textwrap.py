import textwrap

# -- wrap / fill -----------------------------------------------------------
print(textwrap.wrap("The quick brown fox jumped over the lazy dog.", width=20))
print(textwrap.fill("The quick brown fox jumped over the lazy dog.", width=20))
print(textwrap.fill("The quick brown fox.", width=15))
print(textwrap.wrap(""))
print(textwrap.wrap("   "))
print(textwrap.fill("single", width=70))

# -- hyphen / em-dash splitting (default break_on_hyphens=True) -------------
print(textwrap.wrap("Look, goof-ball -- use the -b option!", width=12))
print(textwrap.wrap("well-being and self-aware-ness", width=10))
print(textwrap.wrap("e-mail x-y 1-2-3 co-op", width=8))
print(textwrap.wrap("this--that foo---bar", width=20))

# break_on_hyphens=False keeps hyphenated words intact
print(textwrap.wrap("Look, goof-ball -- use the -b option!",
                    width=12, break_on_hyphens=False))

# -- long word breaking ----------------------------------------------------
print(textwrap.wrap("supercalifragilistic", width=10, break_long_words=True))
print(textwrap.wrap("supercalifragilistic", width=10, break_long_words=False))
print(textwrap.wrap("aaaaa-bbbbb-ccccc", width=8))

# -- dedent ----------------------------------------------------------------
t = "    line 1\n    line 2\n    line 3\n"
print(repr(textwrap.dedent(t)))
t2 = "   line1\n   line2\n   line3\n"
print(repr(textwrap.dedent(t2)))
# mixed indentation: common margin only
print(repr(textwrap.dedent("    a\n      b\n    c\n")))
# blank lines normalized
print(repr(textwrap.dedent("  a\n\n  b\n")))
# tabs and spaces are not equal -> no common margin
print(repr(textwrap.dedent("  spaces\n\ttab\n")))
print(repr(textwrap.dedent("")))

# -- indent ----------------------------------------------------------------
print(textwrap.indent("line1\nline2\nline3", "> "))
print(repr(textwrap.indent("line1\n\nline3", "> ",
                           predicate=lambda line: line.strip())))
# default predicate skips whitespace-only lines
print(repr(textwrap.indent("hello\n   \nworld\n", "+ ")))
print(repr(textwrap.indent("a\nb", "x")))

# -- shorten ---------------------------------------------------------------
print(textwrap.shorten("Hello world! This is a long string.", width=20))
print(textwrap.shorten("Hello  world!", width=12))
print(textwrap.shorten("Hello  world!", width=11))
print(textwrap.shorten("Hello world", width=8))
print(textwrap.shorten("short", width=20))

# -- TextWrapper class -----------------------------------------------------
wrapper = textwrap.TextWrapper(width=15, initial_indent="  ",
                               subsequent_indent="    ")
print(repr(wrapper.fill("Hello world how are you doing")))

w2 = textwrap.TextWrapper(width=20, initial_indent="  ",
                          subsequent_indent="    ")
print(w2.fill("The quick brown fox jumped over the lazy dog."))

# max_lines + placeholder
w3 = textwrap.TextWrapper(width=20, max_lines=2)
print(w3.wrap("The quick brown fox jumped over the lazy dog."))

# fix_sentence_endings
w4 = textwrap.TextWrapper(width=70, fix_sentence_endings=True)
print(repr(w4.fill("A foo. Bar baz.")))

# defaults expose CPython attribute values
w5 = textwrap.TextWrapper()
print(w5.width, repr(w5.placeholder), w5.break_on_hyphens, w5.max_lines)

# -- error path ------------------------------------------------------------
try:
    textwrap.wrap("hi", width=0)
except ValueError as e:
    print("ValueError:", e)

try:
    textwrap.shorten("word", width=4)
except ValueError as e:
    print("ValueError:", e)

# -- import surface --------------------------------------------------------
from textwrap import wrap, fill, indent, dedent, shorten, TextWrapper
print(callable(wrap), callable(fill), callable(indent),
      callable(dedent), callable(shorten), isinstance(TextWrapper, type))

print("textwrap ok")
