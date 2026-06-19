import sys
# BigInt replacement -> ValueError
try:
    'abc'.translate({ord('a'): 10**30})
    print('WRONG')
except ValueError as e:
    print('ok', 'range' in str(e))

# Negative BigInt -> ValueError
try:
    'abc'.translate({ord('a'): -(10**30)})
    print('WRONG')
except ValueError as e:
    print('ok')

# In-range plain int still works
print('abc'.translate({ord('a'): ord('X')}))  # Xbc

# Out-of-range plain int -> ValueError (sanity check)
try:
    'abc'.translate({ord('a'): 0x110000})
    print('WRONG')
except ValueError as e:
    print('ok')
