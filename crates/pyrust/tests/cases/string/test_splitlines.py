# str.splitlines() parity — keepends coercion via standard truth protocol
# CPython 3.12 accepts any value for keepends and coerces it via bool().

text = 'foo\nbar\nbaz'

# No argument (default: keepends=False)
print(text.splitlines())

# bool
print(text.splitlines(True))
print(text.splitlines(False))

# int
print(text.splitlines(1))
print(text.splitlines(0))

# float — truthy/falsy
print(text.splitlines(1.0))
print(text.splitlines(0.0))

# None — always falsy
print(text.splitlines(None))

# str — empty is falsy, non-empty is truthy
print(text.splitlines(''))
print(text.splitlines('x'))

# list — empty is falsy, non-empty is truthy
print(text.splitlines([]))
print(text.splitlines([1]))

# tuple — empty is falsy, non-empty is truthy
print(text.splitlines(()))
print(text.splitlines((1,)))

# bytes — empty is falsy, non-empty is truthy
print(text.splitlines(b''))
print(text.splitlines(b'x'))

# dict — empty is falsy, non-empty is truthy
print(text.splitlines({}))
print(text.splitlines({1: 2}))

# set — empty is falsy, non-empty is truthy
print(text.splitlines(set()))
print(text.splitlines({1}))

# range — empty is falsy, non-empty is truthy
print(text.splitlines(range(0)))
print(text.splitlines(range(3)))

# complex — 0+0j is falsy, anything else is truthy
print(text.splitlines(complex(0, 0)))
print(text.splitlines(complex(1, 0)))
print(text.splitlines(complex(0, 1)))

# big int
print(text.splitlines(10 ** 20))

# Various line-ending styles (keepends=True)
print('a\r\nb'.splitlines(True))
print('a\rb'.splitlines(True))
print('a\nb'.splitlines(True))
print('a\x0bb'.splitlines(True))
print('a\x0cb'.splitlines(True))
