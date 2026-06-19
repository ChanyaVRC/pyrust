ba = bytearray(b'abc')

# BigInt -> ValueError
try:
    _ = (1 << 70) in ba
    print('WRONG')
except ValueError as e:
    print('ok', str(e) == "byte must be in range(0, 256)")

# Negative BigInt -> ValueError
try:
    _ = (-(1 << 70)) in ba
    print('WRONG')
except ValueError as e:
    print('ok')

# Sanity: in-range values still work
print(97 in ba)     # True  (ord('a'))
print(128 in ba)    # False
print(0 in ba)      # False

# Out-of-range plain int still ValueError (regression check)
try:
    _ = 256 in ba
    print('WRONG')
except ValueError:
    print('ok 256')

# Sibling search methods: BigInt -> ValueError (same wording), for both
# bytearray and bytes (they share the int-or-bytes argument helper).
for obj in (bytearray(b'abc'), b'abc'):
    for meth in ('count', 'index', 'find', 'rfind', 'rindex'):
        try:
            getattr(obj, meth)(1 << 70)
            print('WRONG', type(obj).__name__, meth)
        except ValueError as e:
            print(type(obj).__name__, meth, str(e) == "byte must be in range(0, 256)")
        except Exception as e:
            print('WRONGTYPE', type(obj).__name__, meth, type(e).__name__)
