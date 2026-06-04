# Parity fixture for the built-in `__format__` method (issue #2191).
# `obj.__format__(spec)` on a built-in value must produce the same result as
# `format(obj, spec)` / f"{obj:spec}".  Previously the exposed method was a stub
# that returned the format-spec verbatim (int/float/bool/None/bytes) or raised
# RuntimeError (str/list).

# --- int / bool ---
print(repr((5).__format__('')))
print(repr((5).__format__('d')))
print(repr((255).__format__('x')))
print(repr((5).__format__('03d')))
print(repr((1000000).__format__(',')))
print(repr(True.__format__('')))
print(repr(True.__format__('d')))
print(repr(False.__format__('05d')))

# --- BigInt ---
print(repr((10 ** 30).__format__('x')))
print(repr((10 ** 30).__format__(',')))

# --- float ---
print(repr((3.14).__format__('')))
print(repr((3.14).__format__('.1f')))
print(repr((3.14159).__format__('.2f')))
print(repr((3.14).__format__('e')))

# --- complex ---
print(repr((1 + 2j).__format__('')))
print(repr((1.5 + 2j).__format__('.2f')))

# --- str ---
print(repr('hi'.__format__('')))
print(repr('hi'.__format__('>5')))
print(repr('hi'.__format__('^6')))

# --- bytes / None / list (inherit object.__format__: empty spec only) ---
print(repr(b'x'.__format__('')))
print(repr(None.__format__('')))
print(repr([1, 2].__format__('')))
print(repr((1, 2).__format__('')))
print(repr({'a': 1}.__format__('')))

# --- bound-method-as-value form ---
m = 'hi'.__format__
print(repr(m('>5')))
n = (255).__format__
print(repr(n('x')))

# --- error: non-empty spec on a type inheriting object.__format__ ---
for value in [None, [1, 2], (1, 2), b'x', {'a': 1}]:
    try:
        value.__format__('x')
    except TypeError as e:
        print('TypeError:', e)

# --- error: wrong argument type ---
try:
    (5).__format__(5)
except TypeError as e:
    print('TypeError:', e)

# --- error: too many arguments (owner is the formatting type) ---
try:
    (5).__format__('a', 'b')
except TypeError as e:
    print('TypeError:', e)
try:
    [1].__format__('a', 'b')
except TypeError as e:
    print('TypeError:', e)

# --- super().__format__('') on a pure user class delegates to str(self) ---
class C:
    def __format__(self, spec):
        return super().__format__(spec)
    def __str__(self):
        return 'custom-str'

print(repr(C().__format__('')))
print(repr(format(C(), '')))
