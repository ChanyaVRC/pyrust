# str.format_map: named-field substitution via a mapping.

# Basic dict mapping
print('{x}'.format_map({'x': 'hello'}))
print('{x} {y}'.format_map({'x': 1, 'y': 2}))

# Conversion flags
print('{name!r}'.format_map({'name': 'world'}))

# Format spec
print('{x:>10}'.format_map({'x': 'test'}))

# Literal braces pass through
print('{{literal}}'.format_map({'x': 1}))

# KeyError on missing key
try:
    '{missing}'.format_map({})
except KeyError as e:
    print(e)

# TypeError on wrong number of args (0)
try:
    '{x}'.format_map()
except TypeError as e:
    print(e)

# TypeError on wrong number of args (2)
try:
    '{x}'.format_map({}, {})
except TypeError as e:
    print(e)

# Positional fields are forbidden with format_map
try:
    '{}'.format_map({'0': 'x'})
except ValueError as e:
    print(e)

try:
    '{0}'.format_map({'0': 'x'})
except ValueError as e:
    print(e)

# Non-mapping: no error if no fields are accessed
print('no fields'.format_map(42))

# Non-mapping: TypeError when a field is accessed
try:
    '{x}'.format_map(42)
except TypeError as e:
    print(e)

# Custom mapping via __getitem__
class M:
    def __getitem__(self, key):
        return f'<{key}>'

print('{x}'.format_map(M()))
print('{a} and {b}'.format_map(M()))
