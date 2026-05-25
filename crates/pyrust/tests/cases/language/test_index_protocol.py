# CPython 3.12 parity: sequence subscripting calls __index__ on non-int indices
# (list/tuple/str/bytes read, list write, list delete).

class MyIndex:
    def __index__(self): return 1

class NegIndex:
    def __index__(self): return -1

class ZeroIndex:
    def __index__(self): return 0

class BadReturn:
    def __index__(self): return 'hello'

class NoIndex:
    pass


# --- list read ---
lst = [10, 20, 30]
print(lst[MyIndex()])      # 20
print(lst[NegIndex()])     # 30
print(lst[ZeroIndex()])    # 10

# --- tuple read ---
tup = (10, 20, 30)
print(tup[MyIndex()])      # 20
print(tup[NegIndex()])     # 30
print(tup[ZeroIndex()])    # 10

# --- str read ---
s = 'abc'
print(s[MyIndex()])        # b
print(s[NegIndex()])       # c
print(s[ZeroIndex()])      # a

# --- bytes read ---
b = b'abc'
print(b[MyIndex()])        # 98 (ord('b'))
print(b[NegIndex()])       # 99 (ord('c'))
print(b[ZeroIndex()])      # 97 (ord('a'))

# --- list write ---
lst2 = [10, 20, 30]
lst2[MyIndex()] = 99
print(lst2)                # [10, 99, 30]

lst2[NegIndex()] = 77
print(lst2)                # [10, 99, 77]

# --- list delete ---
lst3 = [10, 20, 30]
del lst3[MyIndex()]
print(lst3)                # [10, 30]

# --- normal int indexing is unaffected ---
print([1, 2, 3][0])        # 1
print((1, 2, 3)[2])        # 3
print('xyz'[1])            # y

# --- __index__ returning non-int raises TypeError ---
try:
    [1, 2, 3][BadReturn()]
except TypeError as e:
    print(e)               # __index__ returned non-int (type str)

# --- object without __index__ raises TypeError ---
try:
    [1, 2, 3][NoIndex()]
except TypeError as e:
    print(e)               # list indices must be integers or slices, not NoIndex

try:
    (1, 2, 3)[NoIndex()]
except TypeError as e:
    print(e)               # tuple indices must be integers or slices, not NoIndex

try:
    'abc'[NoIndex()]
except TypeError as e:
    print(e)               # string indices must be integers or slices, not NoIndex

# --- bool (subtype of int) still works ---
print(lst[True])           # 20 (True == 1)
print(lst[False])          # 10 (False == 0)
