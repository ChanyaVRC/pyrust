# Parity fixture for slice dispatch via __getitem__ (issue #825).
#
# CPython passes a slice object to __getitem__ when a slice subscript is used
# on a user-defined class.  pyrust previously raised RuntimeError instead.

# --- __getitem__ with slice: delegates to underlying list ---
class MySeq:
    def __init__(self, data):
        self.data = data

    def __getitem__(self, index):
        return self.data[index]


s = MySeq([0, 1, 2, 3, 4, 5])
print(s[1:3])     # [1, 2]
print(s[::2])     # [0, 2, 4]
print(s[::-1])    # [5, 4, 3, 2, 1, 0]
print(s[1:4:2])   # [1, 3]

# --- __getitem__ with int: normal integer indexing ---
print(s[0])       # 0
print(s[-1])      # 5

# --- __getitem__ receives a slice object with correct attributes ---
class Inspector:
    def __getitem__(self, idx):
        if isinstance(idx, slice):
            return (idx.start, idx.stop, idx.step)
        return idx


ins = Inspector()
print(ins[1:3])       # (1, 3, None)
print(ins[1:5:2])     # (1, 5, 2)
print(ins[::])        # (None, None, None)
print(ins[0])         # 0

# --- class without __getitem__ + slice -> TypeError ---
class Plain:
    pass


p = Plain()
try:
    _ = p[1:2]
except TypeError as e:
    print(type(e).__name__)   # TypeError

# --- slice() constructor and isinstance ---
sl = slice(1, 3)
print(sl.start, sl.stop, sl.step)   # 1 3 None
sl2 = slice(5)
print(sl2.start, sl2.stop, sl2.step)  # None 5 None
sl3 = slice(1, 10, 2)
print(sl3.start, sl3.stop, sl3.step)  # 1 10 2
print(isinstance(sl, slice))   # True
print(repr(sl))                # slice(1, 3, None)

# --- tuple slicing ---
t = (10, 20, 30, 40, 50)
print(t[1:3])    # (20, 30)
print(t[::-1])   # (50, 40, 30, 20, 10)

# --- bytes slicing ---
b = b"hello"
print(b[1:4])    # b'ell'
print(b[::2])    # b'hlo'
