# str.format accessible as an instance method (#709)
# hasattr/getattr/call all work on str instances.

# Keyword substitution
s = '{x}'
print(s.format(x=42))

# Positional substitution
print('{0}'.format('hello'))

# Format spec
print('{:.2f}'.format(3.14159))

# Conversion flag
print('{!r}'.format('hi'))

# hasattr / in dir
print(hasattr('', 'format'))
print('format' in dir(''))

# getattr then call
fmt = getattr('{key}', 'format')
print(fmt(key='value'))

# class-level call is unaffected
print(str.format('{a} {b}', a='one', b='two'))

# Mixed positional + keyword
print('{0} and {name}'.format('alpha', name='beta'))

# Repeated positional field
print('{0} {0}'.format('echo'))
