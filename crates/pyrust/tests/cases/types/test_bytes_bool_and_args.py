# bytes(bool) — bool is a subclass of int; True==1, False==0
print(bytes(True))          # b'\x00'  (1 zero byte)
print(bytes(False))         # b''      (0 bytes)
print(bytes([True, False]))  # b'\x01\x00'
print(bytes((True, False)))  # b'\x01\x00'

# Too many args must raise TypeError, not RuntimeError
try:
    bytes(1, 2, 3, 4)
except TypeError:
    print("TypeError too many args")
