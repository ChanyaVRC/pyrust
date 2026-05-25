# Parity fixture for str.__repr__ quote-selection logic.
# CPython prefers single-quote wrapping; switches to double-quote when the
# string contains a single quote but no double quote (avoids backslash escapes).

print(repr("hello"))           # 'hello'
print(repr("it's"))            # "it's"
print(repr('"world"'))         # '"world"'
print(repr("'single'"))        # "'single'"
print(repr("both ' and \""))   # 'both \' and "'
print(repr(""))                # ''
print(repr("\n"))              # '\n'
print(repr("tab\there"))       # 'tab\there'
