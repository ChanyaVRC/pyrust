# str %-formatting %a conversion (ascii repr), like the ascii() builtin (#2073).
#
# The bytes %-format path already implemented %a; the str path was missing the
# arm and raised "unsupported format character 'a'".  %a mirrors %r but escapes
# non-ASCII characters, and must honour width / precision exactly like %r.
print('%a' % 'café')        # "'caf\\xe9'"
print('%a' % 123)
print('%a' % [1, 2])
print('%a' % {'k': 1})
print('%a' % None)
print('%a' % b'hi')
print('%a' % '')
print('%10a' % 'hi')             # width padding
print('%-10a|' % 'hi')           # left align
print('%.3a' % 'hello')          # precision truncation -> "'he"

# %r / %s neighbours must be unaffected: %r keeps non-ASCII as-is.
print('%s %r %a' % ('é', 'é', 'é'))


class C:
    def __repr__(self):
        return 'Café<é>'


print('%a' % C())
print('%r' % C())
